use super::*;
use dbide_export::{ExportErrorKind, ExportOptions, ExportProgress, ExportRequest, ExportSummary};

#[derive(Clone, Copy, ValueEnum)]
enum ExportFormat {
    Csv,
}

#[derive(Args)]
#[command(
    after_help = "Runs a supported query once on an independent read-only SQLite, PostgreSQL or MySQL connection. Does not continue or replay a displayed result, share temporary tables, or see another session's uncommitted changes. Only CSV is supported. Requires a fresh --output path; no --force or stdout data stream. Publishes only after complete execution, encoding and sync. Statistics are JSON on stdout. SQL <=64 KiB; all limits are finite. Network engines accept a bounded SELECT/CTE subset; native transaction restrictions and engine-specific type limits apply."
)]
pub(super) struct Export {
    /// Exact profile ID; omitted uses the explicitly selected profile.
    #[arg(long)]
    profile: Option<String>,
    /// Explicit database override (SQLite accepts only main).
    #[arg(long)]
    database: Option<String>,
    #[command(flatten)]
    auth: Auth,
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
        if !failure
            .message
            .ends_with("; export output was not published")
        {
            failure
                .message
                .push_str("; export output was not published");
        }
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

fn stream(store: &ProfileStore, args: Export, cancelled: Arc<AtomicBool>) -> Result<ExportSummary> {
    let ExportFormat::Csv = args.format;
    if args.input.stdin && args.auth.password_stdin {
        return Err(config(
            "SQL stdin and password stdin are mutually exclusive",
        ));
    }
    let sql = sql(args.input)?;
    // Retain CLI preflight order: changed SQL and invalid marker refuse before
    // loading profiles. The shared service repeats validation for every caller.
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
    let mut profile = profile(&state, args.profile.as_deref())?;
    workspace::validate_target_profile(&profile)?;
    if let Some(database) = args.database {
        if profile.engine == DatabaseEngine::Sqlite && database != "main" {
            return Err(config("SQLite database must be main; nothing submitted"));
        }
        if profile.engine != DatabaseEngine::Sqlite {
            profile.database = database;
        }
    }
    let database = if profile.engine == DatabaseEngine::Sqlite {
        "main"
    } else {
        &profile.database
    };
    let target = StoredExecutionTarget::capture(&profile, database).map_err(config)?;
    if args
        .expect_target_fingerprint
        .as_deref()
        .is_some_and(|expected| expected != target.endpoint_fingerprint)
    {
        return Err(config("target fingerprint changed; nothing submitted"));
    }
    if fs::symlink_metadata(&args.output).is_ok() {
        return Err(config("output exists; export requires a fresh path"));
    }
    let connection = request(profile.clone(), args.auth)?;
    let request = ExportRequest {
        profile,
        sql,
        output: args.output,
        options: ExportOptions {
            max_rows: args.max_rows,
            max_bytes: args.max_bytes,
            max_row_bytes: usize::try_from(args.max_row_bytes).map_err(config)?,
            timeout: Duration::from_secs(args.timeout_secs),
            null_marker: args.null_marker,
            escape_formulas: args.escape_formulas,
            expect_sql_sha256: args.expect_sql_sha256,
            expect_target_fingerprint: args.expect_target_fingerprint,
        },
    };
    dbide_export::export_csv_with_password(
        request,
        connection.password,
        cancelled,
        Arc::new(ExportProgress::default()),
    )
    .map_err(|error| {
        Failure::new(
            match &error.kind {
                ExportErrorKind::Configuration => 2,
                ExportErrorKind::Connection => 3,
                ExportErrorKind::Execution => 4,
                ExportErrorKind::Cancelled => 130,
            },
            error,
        )
    })
}
