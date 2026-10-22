use super::*;
use dbide_data::{Cell, Column, CsvOptions, CsvScalarMode, CsvStreamWriter, FormulaMode, Limits};
use dbide_sqlite::{ExportErrorKind, ExportEvent, ExportLimits, ExportValue};
use tempfile::NamedTempFile;

#[derive(Clone, Copy, ValueEnum)]
enum ExportFormat {
    Csv,
}

#[derive(Args)]
#[command(
    after_help = "Runs the supplied SQL once on an independent read-only SQLite file connection. Does not continue or replay a displayed result, share temporary tables, or see another session's uncommitted changes. Only CSV is supported. Requires a fresh --output path; no --force or stdout data stream. Publishes only after complete execution, encoding and sync. Statistics are JSON on stdout. SQL <=64 KiB; all limits are finite. No PostgreSQL/MySQL streaming support."
)]
pub(super) struct Export {
    /// Exact profile ID; omitted uses the explicitly selected profile.
    #[arg(long)]
    profile: Option<String>,
    #[command(flatten)]
    input: SqlInput,
    /// Fresh file path; its parent must already exist. Existing files are never replaced.
    #[arg(long)]
    output: PathBuf,
    #[arg(long, value_enum, default_value = "csv")]
    format: ExportFormat,
    #[arg(long)]
    expect_sql_sha256: Option<String>,
    #[arg(long)]
    expect_target_fingerprint: Option<String>,
    #[arg(long, default_value = "\\N")]
    null_marker: String,
    /// Prefix spreadsheet formulas with apostrophe; explicitly changes text values.
    #[arg(long)]
    escape_formulas: bool,
    #[arg(long, default_value_t = 1_000_000, value_parser = clap::value_parser!(u64).range(1..=1_000_000_000))]
    max_rows: u64,
    /// Caps both cumulative source payload and encoded file bytes.
    #[arg(long, default_value_t = 67_108_864, value_parser = clap::value_parser!(u64).range(1..=1_099_511_627_776))]
    max_bytes: u64,
    /// Caps each source row/header before application-owned text conversion.
    #[arg(long, default_value_t = 1_048_576, value_parser = clap::value_parser!(u64).range(1..=67_108_864))]
    max_row_bytes: u64,
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=600))]
    timeout_secs: u64,
}

pub(super) fn run(store: &ProfileStore, args: Export, cancelled: Arc<AtomicBool>) -> Result<()> {
    let statistics = stream(store, args, cancelled).map_err(|mut failure| {
        failure
            .message
            .push_str("; export output was not published");
        failure
    })?;
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &statistics).map_err(|error| {
        Failure::new(
            4,
            format!("export file was published; statistics output failed: {error}"),
        )
    })?;
    stdout
        .write_all(b"\n")
        .and_then(|()| stdout.flush())
        .map_err(|error| {
            Failure::new(
                4,
                format!("export file was published; statistics output failed: {error}"),
            )
        })
}

fn stream(
    store: &ProfileStore,
    args: Export,
    cancelled: Arc<AtomicBool>,
) -> Result<serde_json::Value> {
    let ExportFormat::Csv = args.format;
    let sql = sql(args.input)?;
    let sql_sha256 = dbide_project::digest_sql(&sql);
    if args
        .expect_sql_sha256
        .as_deref()
        .is_some_and(|expected| expected != sql_sha256)
    {
        return Err(config("SQL SHA-256 changed; nothing submitted"));
    }
    if args.null_marker.is_empty() {
        return Err(config("CSV NULL marker must not be empty"));
    }
    let state = store.load().map_err(config)?;
    let p = profile(&state, args.profile.as_deref())?;
    if p.engine != DatabaseEngine::Sqlite {
        return Err(config(
            "streaming export currently supports SQLite files only",
        ));
    }
    workspace::validate_target_profile(&p)?;
    if p.sqlite_path.as_deref() == Some(":memory:") {
        return Err(config(
            "streaming export requires a SQLite file; :memory: is a different database on a separate connection",
        ));
    }
    let target = StoredExecutionTarget::capture(&p, "main").map_err(config)?;
    if args
        .expect_target_fingerprint
        .as_deref()
        .is_some_and(|expected| expected != target.endpoint_fingerprint)
    {
        return Err(config("target fingerprint changed; nothing submitted"));
    }
    // Freeze a configured symlink's resolution once. The captured target still
    // identifies the original profile endpoint, not a snapshot or file digest.
    let source = fs::canonicalize(p.sqlite_path.as_deref().expect("validated SQLite path"))
        .map_err(|error| Failure::new(3, error))?;
    if !source.is_file() {
        return Err(config("SQLite export source must be an existing file"));
    }
    let parent = args
        .output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = fs::canonicalize(parent).map_err(config)?;
    let file_name = args
        .output
        .file_name()
        .ok_or_else(|| config("output must name a new file"))?;
    let output_path = parent.join(file_name);
    // Statistics encode this exact absolute path as a JSON string. Refuse an
    // unrepresentable destination before creating any temp file or running SQL.
    let output_name = output_path
        .to_str()
        .ok_or_else(|| config("export output path must be UTF-8"))?
        .to_owned();
    output::check_fresh(&output_path)?;
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut reserved = source.as_os_str().to_os_string();
        reserved.push(suffix);
        // Conservatively include ASCII case aliases on macOS filesystems even
        // when the sidecar does not exist yet and cannot be canonicalized.
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
    let options = CsvOptions {
        null_marker: Some(args.null_marker),
        scalar_mode: CsvScalarMode::TextualValues,
        formula_mode: if args.escape_formulas {
            FormulaMode::Escape
        } else {
            FormulaMode::Preserve
        },
        ..CsvOptions::default()
    };
    let limits = Limits {
        max_rows: usize::try_from(args.max_rows).map_err(config)?,
        max_bytes: usize::try_from(args.max_bytes).map_err(config)?,
    };
    // Validate options before connecting, even if the selected query later fails.
    options.validate().map_err(config)?;
    let mut temporary = NamedTempFile::new_in(&parent).map_err(config)?;
    let mut file = Some(temporary.reopen().map_err(config)?);
    let mut encoder: Option<CsvStreamWriter<fs::File>> = None;
    let started = Instant::now();
    let timeout = Duration::from_secs(args.timeout_secs);
    let summary = dbide_sqlite::export_read_only(
        &source,
        &sql,
        ExportLimits {
            max_rows: args.max_rows,
            max_row_bytes: usize::try_from(args.max_row_bytes).map_err(config)?,
            max_payload_bytes: args.max_bytes,
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
                            &options,
                            limits,
                        )
                        .map_err(|error| error.to_string())?,
                    );
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
                }
            }
            Ok(())
        },
    )
    .map_err(|error| {
        Failure::new(
            match error.kind {
                ExportErrorKind::Configuration => 2,
                ExportErrorKind::Connection => 3,
                ExportErrorKind::Cancelled => 130,
                _ => 4,
            },
            error,
        )
    })?;
    let check = || {
        if cancelled.load(Ordering::Acquire) {
            Err(Failure::new(130, "export cancelled"))
        } else if started.elapsed() >= timeout {
            Err(Failure::new(4, "export deadline exceeded"))
        } else {
            Ok(())
        }
    };
    check()?;
    encoder
        .take()
        .ok_or_else(|| Failure::new(4, "missing export header"))?
        .finish()
        .map_err(|error| Failure::new(4, error))?;
    check()?;
    temporary
        .as_file_mut()
        .sync_all()
        .map_err(|error| Failure::new(4, error))?;
    check()?;
    let bytes = temporary.as_file().metadata().map_err(config)?.len();
    let statistics = serde_json::json!({
        "schema_version": 1, "ok": true, "format": "csv", "rows": summary.rows,
        "encoded_bytes": bytes, "payload_bytes": summary.payload_bytes,
        "output": output_name, "execution_target": target, "sql_sha256": sql_sha256,
        "read_only": true, "independent_connection": true,
    });
    // Atomic no-clobber still arbitrates races after the preflight check.
    temporary.persist_noclobber(&output_path).map_err(config)?;
    Ok(statistics)
}
