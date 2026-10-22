//! Shared GUI/CLI one-shot read-only streaming on independent native connections.
//! CSV publishes a fresh file only after native completion, cleanup, encoding and
//! sync succeed. No result-set retention, retries, console session reuse or secrets
//! in persisted requests. Values are textual representations, not typed interchange.

use dbide_data::{Cell, Column, CsvOptions, CsvScalarMode, CsvStreamWriter, FormulaMode, Limits};
use dbide_driver_api::stream::{
    StreamError, StreamErrorKind, StreamEvent, StreamLimits, StreamSummary,
};
use dbide_profiles::{ConnectionProfile, ConnectionRequest, DatabaseEngine, StoredExecutionTarget};
use dbide_sqlite::{ExportEvent, ExportLimits, ExportValue};
use serde::Serialize;
use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tempfile::NamedTempFile;

pub const MAX_ROWS: u64 = 1_000_000_000;
pub const MAX_BYTES: u64 = 1_099_511_627_776;
pub const MAX_ROW_BYTES: usize = 67_108_864;
pub const MAX_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Clone, Debug)]
pub struct ExportRequest {
    pub profile: ConnectionProfile,
    pub sql: String,
    pub output: PathBuf,
    pub options: ExportOptions,
}

/// Ephemeral worker input. Password belongs only to this explicit connection;
/// this request is neither serialized nor included in diagnostics.
#[derive(Clone)]
pub struct QueryRequest {
    pub connection: ConnectionRequest,
    pub sql: String,
    pub options: ExportOptions,
}

#[derive(Clone, Debug)]
pub struct QuerySummary {
    pub rows: u64,
    pub payload_bytes: u64,
    pub execution_target: StoredExecutionTarget,
    pub sql_sha256: String,
    pub execution_id: u64,
}

#[derive(Clone, Debug)]
pub struct ExportOptions {
    pub max_rows: u64,
    /// Independently caps cumulative native source payload and encoded CSV.
    pub max_bytes: u64,
    pub max_row_bytes: usize,
    pub timeout: Duration,
    pub null_marker: String,
    pub escape_formulas: bool,
    pub expect_sql_sha256: Option<String>,
    pub expect_target_fingerprint: Option<String>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            max_rows: 1_000_000,
            max_bytes: 67_108_864,
            max_row_bytes: 1_048_576,
            timeout: Duration::from_secs(30),
            null_marker: "\\N".into(),
            escape_formulas: false,
            expect_sql_sha256: None,
            expect_target_fingerprint: None,
        }
    }
}

/// One execution's counters, overwritten in place without queues or row retention.
/// Active counters approximate source bytes; successful terminal counters exactly
/// match the native summary (SQLite numeric payload differs from textual length).
#[derive(Debug, Default)]
pub struct ExportProgress {
    rows: AtomicU64,
    payload_bytes: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProgressSnapshot {
    pub rows: u64,
    pub payload_bytes: u64,
}

impl ExportProgress {
    /// Two atomic loads can span adjacent rows; terminal summary is authoritative.
    pub fn snapshot(&self) -> ProgressSnapshot {
        ProgressSnapshot {
            rows: self.rows.load(Ordering::Acquire),
            payload_bytes: self.payload_bytes.load(Ordering::Acquire),
        }
    }

    fn update(&self, rows: u64, payload_bytes: u64) {
        self.payload_bytes.store(payload_bytes, Ordering::Release);
        self.rows.store(rows, Ordering::Release);
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ExportSummary {
    pub schema_version: u32,
    pub ok: bool,
    pub format: String,
    pub rows: u64,
    pub encoded_bytes: u64,
    pub payload_bytes: u64,
    pub output: String,
    pub execution_target: StoredExecutionTarget,
    pub sql_sha256: String,
    pub read_only: bool,
    pub independent_connection: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportErrorKind {
    Configuration,
    Connection,
    Execution,
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct ExportError {
    pub kind: ExportErrorKind,
    pub message: String,
}

impl ExportError {
    fn new(kind: ExportErrorKind, message: impl ToString) -> Self {
        Self {
            kind,
            message: message.to_string(),
        }
    }
}
impl fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for ExportError {}
fn config(message: impl ToString) -> ExportError {
    ExportError::new(ExportErrorKind::Configuration, message)
}
fn execution(message: impl ToString) -> ExportError {
    ExportError::new(ExportErrorKind::Execution, message)
}

/// Synchronous worker entry. Network engines get a private current-thread Tokio
/// runtime; call outside an existing Tokio runtime. SQLite executes directly.
/// One deadline covers preflight and execution. Native connection retirement can
/// use an additional finite cleanup allowance of at most three seconds; it cannot
/// turn cancellation, timeout or partial completion into success. Synchronous
/// sinks and blocked operating-system operations cannot be preempted.
pub fn stream_query<F>(
    request: QueryRequest,
    cancelled: Arc<AtomicBool>,
    progress: Arc<ExportProgress>,
    sink: F,
) -> Result<QuerySummary, ExportError>
where
    F: for<'a> FnMut(StreamEvent<'a>) -> Result<(), String> + Send,
{
    let started = Instant::now();
    progress.update(0, 0);
    validate_options(&request.options)?;
    check_control(&cancelled, started, request.options.timeout)?;
    let prepared = prepare(request)?;
    run_prepared(prepared, started, cancelled, progress, sink)
}

struct PreparedQuery {
    request: QueryRequest,
    source: Option<PathBuf>,
    target: StoredExecutionTarget,
    sql_sha256: String,
}

fn prepare(request: QueryRequest) -> Result<PreparedQuery, ExportError> {
    validate_options(&request.options)?;
    if request.sql.len() > dbide_project::MAX_SQL_BYTES || request.sql.trim().is_empty() {
        return Err(config(
            "SQL must be nonempty and at most 65536 UTF-8 bytes; nothing submitted",
        ));
    }
    let sql_sha256 = dbide_project::digest_sql(&request.sql);
    if request
        .options
        .expect_sql_sha256
        .as_deref()
        .is_some_and(|expected| expected != sql_sha256)
    {
        return Err(config("SQL SHA-256 changed; nothing submitted"));
    }
    let profile = &request.connection.profile;
    dbide_project::validate_target_profile(profile).map_err(|_| {
        config("Invalid connection profile; guarded SQLite targets require an absolute SQLite path; nothing submitted")
    })?;
    let database = if profile.engine == DatabaseEngine::Sqlite {
        "main"
    } else {
        &profile.database
    };
    let target = StoredExecutionTarget::capture(profile, database).map_err(config)?;
    if request
        .options
        .expect_target_fingerprint
        .as_deref()
        .is_some_and(|expected| expected != target.endpoint_fingerprint)
    {
        return Err(config("target fingerprint changed; nothing submitted"));
    }
    let source = if profile.engine == DatabaseEngine::Sqlite {
        if profile.sqlite_path.as_deref() == Some(":memory:") {
            return Err(config(
                "streaming requires a SQLite file; :memory: is a different database on a separate connection",
            ));
        }
        // Capture target identity from configuration, then resolve a symlink only
        // once for both source execution and output sidecar guards.
        let source = fs::canonicalize(
            profile
                .sqlite_path
                .as_deref()
                .expect("validated SQLite path"),
        )
        .map_err(|error| ExportError::new(ExportErrorKind::Connection, error))?;
        if !source.is_file() || source.to_str().is_none() {
            return Err(config(
                "SQLite stream source must be an existing UTF-8 file",
            ));
        }
        Some(source)
    } else {
        None
    };
    Ok(PreparedQuery {
        request,
        source,
        target,
        sql_sha256,
    })
}

fn run_prepared<F>(
    prepared: PreparedQuery,
    started: Instant,
    cancelled: Arc<AtomicBool>,
    progress: Arc<ExportProgress>,
    mut sink: F,
) -> Result<QuerySummary, ExportError>
where
    F: for<'a> FnMut(StreamEvent<'a>) -> Result<(), String> + Send,
{
    let PreparedQuery {
        request,
        source,
        target,
        sql_sha256,
    } = prepared;
    let QueryRequest {
        connection,
        sql,
        options,
    } = request;
    check_control(&cancelled, started, options.timeout)?;
    let execution_id = dbide_driver_api::next_session_id();
    let mut rows = 0u64;
    let mut bytes = 0u64;
    let mut observe = |event: StreamEvent<'_>| {
        check_control(&cancelled, started, options.timeout).map_err(|error| error.to_string())?;
        match &event {
            StreamEvent::Columns(names) => {
                bytes = names.iter().map(|name| name.len().max(1) as u64).sum();
            }
            StreamEvent::Row(values) => {
                rows += 1;
                bytes = bytes.saturating_add(
                    values
                        .iter()
                        .map(|value| value.as_ref().map_or(1, |value| value.len().max(1) as u64))
                        .sum(),
                );
            }
        }
        sink(event)?;
        progress.update(rows, bytes);
        check_control(&cancelled, started, options.timeout).map_err(|error| error.to_string())
    };
    let limits = StreamLimits {
        max_rows: options.max_rows,
        max_row_bytes: options.max_row_bytes,
        max_payload_bytes: options.max_bytes,
        timeout: options.timeout.saturating_sub(started.elapsed()),
    };
    let result = match connection.profile.engine {
        DatabaseEngine::Sqlite => dbide_sqlite::export_read_only(
            source.as_deref().expect("prepared SQLite source"),
            &sql,
            ExportLimits {
                max_rows: limits.max_rows,
                max_row_bytes: limits.max_row_bytes,
                max_payload_bytes: limits.max_payload_bytes,
                timeout: limits.timeout,
            },
            cancelled.clone(),
            |event| match event {
                ExportEvent::Columns(names) => observe(StreamEvent::Columns(names)),
                ExportEvent::Row(values) => {
                    let values = values
                        .iter()
                        .map(|value| match value {
                            ExportValue::Null => None,
                            ExportValue::Integer(value) => Some(value.to_string()),
                            ExportValue::Real(value) => Some(value.to_string()),
                            ExportValue::Text(value) => Some((*value).to_owned()),
                        })
                        .collect::<Vec<_>>();
                    observe(StreamEvent::Row(&values))
                }
            },
        )
        .map(|summary| StreamSummary {
            rows: summary.rows,
            payload_bytes: summary.payload_bytes,
        })
        .map_err(|error| {
            ExportError::new(
                match error.kind {
                    dbide_sqlite::ExportErrorKind::Configuration => ExportErrorKind::Configuration,
                    dbide_sqlite::ExportErrorKind::Connection => ExportErrorKind::Connection,
                    dbide_sqlite::ExportErrorKind::Cancelled => ExportErrorKind::Cancelled,
                    _ => ExportErrorKind::Execution,
                },
                error,
            )
        }),
        engine => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(execution)?;
            // Runtime construction is preflight too; pass only remaining budget.
            let limits = StreamLimits {
                timeout: options.timeout.saturating_sub(started.elapsed()),
                ..limits
            };
            check_control(&cancelled, started, options.timeout)?;
            match engine {
                DatabaseEngine::PostgreSql => {
                    runtime.block_on(dbide_pg_spike::export::stream_read_only(
                        connection,
                        &sql,
                        limits,
                        cancelled.clone(),
                        &mut observe,
                    ))
                }
                DatabaseEngine::MySql => runtime.block_on(dbide_mysql::export::stream_read_only(
                    connection,
                    &sql,
                    limits,
                    cancelled.clone(),
                    &mut observe,
                )),
                DatabaseEngine::Sqlite => unreachable!(),
            }
            .map_err(map_stream_error)
        }
    };
    check_control(&cancelled, started, options.timeout)?;
    let summary = result?;
    progress.update(summary.rows, summary.payload_bytes);
    Ok(QuerySummary {
        rows: summary.rows,
        payload_bytes: summary.payload_bytes,
        execution_target: target,
        sql_sha256,
        execution_id,
    })
}

fn map_stream_error(error: StreamError) -> ExportError {
    ExportError::new(
        match error.kind {
            StreamErrorKind::Configuration => ExportErrorKind::Configuration,
            StreamErrorKind::Connection => ExportErrorKind::Connection,
            StreamErrorKind::Cancelled => ExportErrorKind::Cancelled,
            _ => ExportErrorKind::Execution,
        },
        error,
    )
}

/// Empty-password compatibility entry; use `export_csv_with_password` for an
/// explicitly supplied network credential. Error paths discard provisional files.
pub fn export_csv(
    request: ExportRequest,
    cancelled: Arc<AtomicBool>,
    progress: Arc<ExportProgress>,
) -> Result<ExportSummary, ExportError> {
    export_csv_with_password(request, String::new(), cancelled, progress)
}

pub fn export_csv_with_password(
    request: ExportRequest,
    password: String,
    cancelled: Arc<AtomicBool>,
    progress: Arc<ExportProgress>,
) -> Result<ExportSummary, ExportError> {
    let started = Instant::now();
    progress.update(0, 0);
    export_file(request, password, started, cancelled, progress).map_err(|mut error| {
        error.message.push_str("; export output was not published");
        error
    })
}

fn export_file(
    request: ExportRequest,
    password: String,
    started: Instant,
    cancelled: Arc<AtomicBool>,
    progress: Arc<ExportProgress>,
) -> Result<ExportSummary, ExportError> {
    let ExportRequest {
        profile,
        sql,
        output,
        options,
    } = request;
    // Cancellation wins over invalid/missing sources, while invalid limits are
    // still configuration errors rather than an already-expired zero deadline.
    if cancelled.load(Ordering::Acquire) {
        return Err(ExportError::new(
            ExportErrorKind::Cancelled,
            "export cancelled",
        ));
    }
    validate_options(&options)?;
    check_control(&cancelled, started, options.timeout)?;
    let timeout = options.timeout;
    let csv_options = csv_options(&options);
    let limits = Limits {
        max_rows: usize::try_from(options.max_rows).map_err(config)?,
        max_bytes: usize::try_from(options.max_bytes).map_err(config)?,
    };
    let prepared = prepare(QueryRequest {
        connection: ConnectionRequest { profile, password },
        sql,
        options,
    })?;
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = fs::canonicalize(parent).map_err(config)?;
    let file_name = output
        .file_name()
        .ok_or_else(|| config("output must name a new file"))?;
    let output_path = parent.join(file_name);
    let output_name = output_path
        .to_str()
        .ok_or_else(|| config("export output path must be UTF-8"))?
        .to_owned();
    check_fresh(&output_path)?;
    if let Some(source) = &prepared.source {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let mut reserved = source.as_os_str().to_os_string();
            reserved.push(suffix);
            if output_path
                .as_os_str()
                .as_encoded_bytes()
                .eq_ignore_ascii_case(reserved.as_os_str().as_encoded_bytes())
            {
                return Err(config(
                    "export output must not be the source database or its WAL/shared-memory/rollback-journal sidecar",
                ));
            }
        }
    }
    check_control(&cancelled, started, timeout)?;
    let mut temporary = NamedTempFile::new_in(&parent).map_err(config)?;
    let mut file = Some(temporary.reopen().map_err(config)?);
    let mut encoder: Option<CsvStreamWriter<fs::File>> = None;
    let summary = run_prepared(prepared, started, cancelled.clone(), progress, |event| {
        match event {
            StreamEvent::Columns(names) => {
                let columns = names
                    .iter()
                    .map(|name| Column { name: name.clone() })
                    .collect::<Vec<_>>();
                encoder = Some(
                    CsvStreamWriter::new(
                        file.take().ok_or("duplicate export header")?,
                        &columns,
                        &csv_options,
                        limits,
                    )
                    .map_err(|error| error.to_string())?,
                );
            }
            StreamEvent::Row(values) => {
                let row = values
                    .iter()
                    .map(|value| {
                        value
                            .as_ref()
                            .map_or(Cell::Null, |value| Cell::Text(value.clone()))
                    })
                    .collect::<Vec<_>>();
                encoder
                    .as_mut()
                    .ok_or("missing export header")?
                    .write_row(&row)
                    .map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    })?;
    check_control(&cancelled, started, timeout)?;
    encoder
        .take()
        .ok_or_else(|| execution("missing export header"))?
        .finish()
        .map_err(execution)?;
    check_control(&cancelled, started, timeout)?;
    temporary.as_file_mut().sync_all().map_err(execution)?;
    check_control(&cancelled, started, timeout)?;
    let bytes = temporary.as_file().metadata().map_err(config)?.len();
    let statistics = ExportSummary {
        schema_version: 1,
        ok: true,
        format: "csv".into(),
        rows: summary.rows,
        encoded_bytes: bytes,
        payload_bytes: summary.payload_bytes,
        output: output_name,
        execution_target: summary.execution_target,
        sql_sha256: summary.sql_sha256,
        read_only: true,
        independent_connection: true,
    };
    // Atomic no-clobber arbitrates destination races after preflight. The check
    // covers all preceding work; OS publication itself cannot be preempted.
    check_control(&cancelled, started, timeout)?;
    temporary.persist_noclobber(&output_path).map_err(config)?;
    Ok(statistics)
}

fn check_control(
    cancelled: &AtomicBool,
    started: Instant,
    timeout: Duration,
) -> Result<(), ExportError> {
    if cancelled.load(Ordering::Acquire) {
        Err(ExportError::new(
            ExportErrorKind::Cancelled,
            "export cancelled",
        ))
    } else if started.elapsed() >= timeout {
        Err(execution("export deadline exceeded"))
    } else {
        Ok(())
    }
}

fn csv_options(options: &ExportOptions) -> CsvOptions {
    CsvOptions {
        null_marker: Some(options.null_marker.clone()),
        scalar_mode: CsvScalarMode::TextualValues,
        formula_mode: if options.escape_formulas {
            FormulaMode::Escape
        } else {
            FormulaMode::Preserve
        },
        ..CsvOptions::default()
    }
}

fn validate_options(options: &ExportOptions) -> Result<(), ExportError> {
    if !(1..=MAX_ROWS).contains(&options.max_rows)
        || !(1..=MAX_BYTES).contains(&options.max_bytes)
        || !(1..=MAX_ROW_BYTES).contains(&options.max_row_bytes)
        || options.timeout.is_zero()
        || options.timeout > MAX_TIMEOUT
    {
        return Err(config(
            "export limits must be positive: max rows <=1000000000, max bytes <=1099511627776, max row bytes <=67108864, timeout <=600 seconds",
        ));
    }
    if options.null_marker.is_empty() {
        return Err(config("CSV NULL marker must not be empty"));
    }
    csv_options(options).validate().map_err(config)?;
    Ok(())
}

fn check_fresh(path: &Path) -> Result<(), ExportError> {
    match fs::symlink_metadata(path) {
        Ok(_) => return Err(config("output exists; export requires a fresh path")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(config(error)),
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !parent.is_dir() {
        return Err(config(
            "output parent directory must already exist; nothing submitted",
        ));
    }
    drop(NamedTempFile::new_in(parent).map_err(config)?);
    Ok(())
}
