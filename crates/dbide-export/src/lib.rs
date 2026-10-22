//! Shared GUI/CLI success-only CSV file export. Captures one explicit profile
//! and SQL document, executes once on an independent native read-only SQLite
//! connection, and publishes a fresh destination only after encoding and sync.
//! It never resumes a displayed result or shares another console's session.

use dbide_data::{Cell, Column, CsvOptions, CsvScalarMode, CsvStreamWriter, FormulaMode, Limits};
use dbide_profiles::{ConnectionProfile, DatabaseEngine, StoredExecutionTarget};
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

/// Single-export progress, overwritten in place without queues or row retention.
/// These counters describe encoded provisional data, not a published output.
/// Use one instance per running export. Successful completion equals its summary.
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
    /// Approximate counters during execution, using two nonblocking atomic loads.
    /// The pair may span adjacent rows while the writer runs. After completion,
    /// use the terminal summary for exact published totals.
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

/// Error paths drop the private provisional file and never publish an output.
/// Cancellation and deadlines are checked during native stepping and before
/// publication; they do not bound a blocked OS file operation or SQLite's own
/// working set. No automatic retries, connection discovery, or SQL replay.
pub fn export_csv(
    request: ExportRequest,
    cancelled: Arc<AtomicBool>,
    progress: Arc<ExportProgress>,
) -> Result<ExportSummary, ExportError> {
    stream(request, cancelled, progress).map_err(|mut error| {
        error.message.push_str("; export output was not published");
        error
    })
}

fn stream(
    request: ExportRequest,
    cancelled: Arc<AtomicBool>,
    progress: Arc<ExportProgress>,
) -> Result<ExportSummary, ExportError> {
    let ExportRequest {
        profile,
        sql,
        output,
        options,
    } = request;
    progress.update(0, 0);
    if cancelled.load(Ordering::Acquire) {
        return Err(ExportError::new(
            ExportErrorKind::Cancelled,
            "export cancelled",
        ));
    }
    validate_options(&options)?;
    if sql.len() > dbide_project::MAX_SQL_BYTES || sql.trim().is_empty() {
        return Err(config(
            "SQL must be nonempty and at most 65536 UTF-8 bytes; nothing submitted",
        ));
    }
    let sql_sha256 = dbide_project::digest_sql(&sql);
    if options
        .expect_sql_sha256
        .as_deref()
        .is_some_and(|expected| expected != sql_sha256)
    {
        return Err(config("SQL SHA-256 changed; nothing submitted"));
    }
    if profile.engine != DatabaseEngine::Sqlite {
        return Err(config(
            "streaming export currently supports SQLite files only",
        ));
    }
    dbide_project::validate_target_profile(&profile).map_err(|_| {
        config("Guarded SQLite targets require an absolute SQLite path or :memory:; update the profile explicitly")
    })?;
    if profile.sqlite_path.as_deref() == Some(":memory:") {
        return Err(config(
            "streaming export requires a SQLite file; :memory: is a different database on a separate connection",
        ));
    }
    let target = StoredExecutionTarget::capture(&profile, "main").map_err(config)?;
    if options
        .expect_target_fingerprint
        .as_deref()
        .is_some_and(|expected| expected != target.endpoint_fingerprint)
    {
        return Err(config("target fingerprint changed; nothing submitted"));
    }
    // Resolve a configured symlink once. Target identity remains the original
    // endpoint configuration, not a content digest or a connection snapshot.
    let source = fs::canonicalize(
        profile
            .sqlite_path
            .as_deref()
            .expect("validated SQLite path"),
    )
    .map_err(|error| ExportError::new(ExportErrorKind::Connection, error))?;
    if !source.is_file() {
        return Err(config("SQLite export source must be an existing file"));
    }
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = fs::canonicalize(parent).map_err(config)?;
    let file_name = output
        .file_name()
        .ok_or_else(|| config("output must name a new file"))?;
    let output_path = parent.join(file_name);
    // Refuse an unrepresentable summary path before temp creation or SQL.
    let output_name = output_path
        .to_str()
        .ok_or_else(|| config("export output path must be UTF-8"))?
        .to_owned();
    check_fresh(&output_path)?;
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
    let csv_options = CsvOptions {
        null_marker: Some(options.null_marker),
        scalar_mode: CsvScalarMode::TextualValues,
        formula_mode: if options.escape_formulas {
            FormulaMode::Escape
        } else {
            FormulaMode::Preserve
        },
        ..CsvOptions::default()
    };
    let limits = Limits {
        max_rows: usize::try_from(options.max_rows).map_err(config)?,
        max_bytes: usize::try_from(options.max_bytes).map_err(config)?,
    };
    csv_options.validate().map_err(config)?;
    let mut temporary = NamedTempFile::new_in(&parent).map_err(config)?;
    let mut file = Some(temporary.reopen().map_err(config)?);
    let mut encoder: Option<CsvStreamWriter<fs::File>> = None;
    let mut rows = 0;
    let mut payload_bytes = 0;
    let started = Instant::now();
    let timeout = options.timeout;
    let summary = dbide_sqlite::export_read_only(
        &source,
        &sql,
        ExportLimits {
            max_rows: options.max_rows,
            max_row_bytes: options.max_row_bytes,
            max_payload_bytes: options.max_bytes,
            timeout,
        },
        cancelled.clone(),
        |event| {
            match event {
                ExportEvent::Columns(names) => {
                    let columns: Vec<Column> = names
                        .iter()
                        .map(|name| Column { name: name.clone() })
                        .collect();
                    encoder = Some(
                        CsvStreamWriter::new(
                            file.take().ok_or("duplicate export header")?,
                            &columns,
                            &csv_options,
                            limits,
                        )
                        .map_err(|error| error.to_string())?,
                    );
                    payload_bytes = names.iter().map(|name| name.len().max(1) as u64).sum();
                    progress.update(rows, payload_bytes);
                }
                ExportEvent::Row(values) => {
                    let row: Vec<Cell> = values
                        .iter()
                        .map(|value| match value {
                            ExportValue::Null => Cell::Null,
                            ExportValue::Integer(value) => Cell::Integer(*value),
                            ExportValue::Real(value) => Cell::Decimal(value.to_string()),
                            ExportValue::Text(value) => Cell::Text((*value).into()),
                        })
                        .collect();
                    encoder
                        .as_mut()
                        .ok_or("missing export header")?
                        .write_row(&row)
                        .map_err(|error| error.to_string())?;
                    // Match native source accounting, not textual CSV lengths.
                    // Native bounds already checked each row and cumulative sum.
                    payload_bytes += values
                        .iter()
                        .map(|value| match value {
                            ExportValue::Null => 1,
                            ExportValue::Integer(_) | ExportValue::Real(_) => 8,
                            ExportValue::Text(value) => value.len().max(1) as u64,
                        })
                        .sum::<u64>();
                    rows += 1;
                    progress.update(rows, payload_bytes);
                }
            }
            Ok(())
        },
    )
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
    })?;
    let check = || {
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
    };
    check()?;
    encoder
        .take()
        .ok_or_else(|| execution("missing export header"))?
        .finish()
        .map_err(execution)?;
    check()?;
    temporary.as_file_mut().sync_all().map_err(execution)?;
    check()?;
    let bytes = temporary.as_file().metadata().map_err(config)?.len();
    let statistics = ExportSummary {
        schema_version: 1,
        ok: true,
        format: "csv".into(),
        rows: summary.rows,
        encoded_bytes: bytes,
        payload_bytes: summary.payload_bytes,
        output: output_name,
        execution_target: target,
        sql_sha256,
        read_only: true,
        independent_connection: true,
    };
    // Atomic no-clobber still arbitrates destination races after preflight.
    check()?;
    temporary.persist_noclobber(&output_path).map_err(config)?;
    Ok(statistics)
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
    // Validate permissions without publishing anything or submitting SQL.
    drop(NamedTempFile::new_in(parent).map_err(config)?);
    Ok(())
}
