//! Native CLI: shared profiles and bridge, explicit credentials, no SQL replay.
mod export;
mod output;
mod workspace;
use clap::{Args, Parser, Subcommand, ValueEnum};
use dbide_bridge::{BridgeEvent, DatabaseBridge, OperationId, Outcome};
use dbide_credentials::{CredentialStore, MacKeychainStore, Secret};
use dbide_profiles::{
    ConnectionProfile, ConnectionRequest, DatabaseEngine, HistoryEntry, ProfileStore,
    StoredExecutionTarget, TlsMode, WorkspaceState,
};
use dbide_settings::{Locale, SettingsStore};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, Failure>;
struct Failure {
    code: u8,
    message: String,
    machine_code: &'static str,
}
impl Failure {
    fn new(code: u8, message: impl ToString) -> Self {
        Self {
            code,
            message: message.to_string(),
            machine_code: "configuration_error",
        }
    }
}
fn config(e: impl ToString) -> Failure {
    Failure::new(2, e)
}

#[derive(Parser)]
#[command(
    name = "dbide",
    version,
    about = "Native database profiles, SQL execution and schema inspection",
    after_help = "Examples:\n  dbide --config ./profiles.json profiles add --id demo --name Demo --engine sqlite --sqlite-path ./demo.db --sqlite-create\n  dbide --config ./profiles.json query --profile demo --sql 'SELECT 1' --format json\n  dbide --config ./profiles.json config set persist_history true"
)]
struct Cli {
    /// Shared GUI/CLI password-free profiles file. Default is the platform application settings path.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Explicit folder workspace operations. JSON output; never connects to a database.
    Workspace(workspace::Workspace),
    /// Offline machine-readable CLI discovery.
    Agent {
        #[command(subcommand)]
        command: workspace::AgentCommand,
    },
    /// Manage password-free connection profiles. Add/edit never connect.
    Profiles {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    /// Inspect or change shared local settings.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Execute SQL once through the shared native bridge. No retry after dispatch.
    Query(Query),
    /// Stream one explicit supported read-only query to a fresh CSV file.
    Export(export::Export),
    /// Read server catalogs as JSON using a temporary connection.
    Schema(Schema),
    /// Read saved catalog metadata from disk. No database connection or authentication.
    #[command(
        after_help = "Examples:\n  dbide --config ./profiles.json catalog list\n  dbide --config ./profiles.json catalog show SHARD_ID\n\nCopy an exact shard_id from list. Reads only historical files in the selected profiles directory's catalog-cache; it never refreshes the database."
    )]
    Catalog {
        #[command(subcommand)]
        command: CatalogCommand,
    },
    /// Offline SQL tools; no database connection or profile access.
    Sql {
        #[command(subcommand)]
        command: SqlCommand,
    },
}
#[derive(Subcommand)]
enum CatalogCommand {
    /// Print bounded manifest summaries without reading shard payloads.
    List,
    /// Print one manifest-listed historical snapshot; use an exact ID from catalog list.
    Show { shard_id: String },
}
#[derive(Subcommand)]
enum ProfileCommand {
    /// Print password-free profiles and the selected profile ID as JSON.
    List,
    Add(ProfileFields),
    Edit {
        profile_id: String,
        #[command(flatten)]
        fields: ProfileFields,
    },
    /// Back up the original settings before removal. Keychain credentials are retained.
    Remove {
        id: String,
    },
    Select {
        id: String,
    },
    /// Explicitly open and close a test connection; SQLite creation follows the profile setting.
    Test {
        id: String,
        #[command(flatten)]
        auth: Auth,
        #[arg(long, default_value_t=30, value_parser=clap::value_parser!(u64).range(1..=600))]
        timeout_secs: u64,
    },
}
#[derive(Args, Default)]
struct ProfileFields {
    #[arg(long)]
    id: Option<String>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long, value_enum)]
    engine: Option<Engine>,
    #[arg(long)]
    sqlite_path: Option<String>,
    #[arg(long, num_args=0..=1, default_missing_value="true")]
    sqlite_create: Option<bool>,
    #[arg(long, num_args=0..=1, default_missing_value="true")]
    read_only: Option<bool>,
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long)]
    database: Option<String>,
    #[arg(long)]
    username: Option<String>,
    #[arg(long, value_enum)]
    tls: Option<Tls>,
    #[arg(long)]
    root_cert_path: Option<String>,
    #[arg(long)]
    connect_timeout_secs: Option<u64>,
}
#[derive(Clone, Copy, ValueEnum)]
enum Engine {
    Postgres,
    Sqlite,
    Mysql,
}
#[derive(Clone, Copy, ValueEnum)]
enum Tls {
    Disable,
    Require,
    VerifyFull,
}
#[derive(Subcommand)]
enum ConfigCommand {
    Show,
    Set {
        #[arg(value_parser=["persist_history"])]
        key: String,
        #[arg(action=clap::ArgAction::Set)]
        value: bool,
    },
    /// Read or change the shared GUI language without opening profiles or databases.
    Language {
        #[command(subcommand)]
        command: LanguageCommand,
    },
}
#[derive(Subcommand)]
enum LanguageCommand {
    /// Print ko or en. Missing settings default to ko without creating a file.
    Get,
    /// Atomically change only locale in app-settings.json beside the selected profiles file.
    Set {
        #[arg(value_enum)]
        locale: Language,
    },
}
#[derive(Clone, Copy, ValueEnum)]
enum Language {
    Ko,
    En,
}
#[derive(Args, Default)]
#[group(id = "password_source", multiple = false)]
struct Auth {
    /// Read this explicitly named environment variable; never infer credentials.
    #[arg(long)]
    password_env: Option<String>,
    /// Read a UTF-8 password from stdin (one trailing LF/CRLF removed).
    #[arg(long)]
    password_stdin: bool,
    /// Load the exact selected profile ID from the app's macOS Keychain service.
    #[arg(long)]
    keychain: bool,
}
#[derive(Args)]
#[group(id = "sql_source", required = true, multiple = false)]
struct SqlInput {
    #[arg(long, allow_hyphen_values = true)]
    sql: Option<String>,
    #[arg(long)]
    file: Option<PathBuf>,
    #[arg(long)]
    stdin: bool,
}
#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Table,
    Json,
    Csv,
}
#[derive(Args)]
struct Destination {
    /// Atomically publish to a fresh file after execution/encoding succeeds.
    #[arg(long)]
    output: Option<PathBuf>,
    /// Explicitly replace an existing output only after successful encoding.
    #[arg(long, requires = "output")]
    force: bool,
}
#[derive(Args)]
#[command(
    after_help = "Examples:\n  dbide query --profile dev --file ./query.sql --format json\n  dbide query --profile dev --sql 'SELECT 1;' --format csv --output ./result.csv\n  dbide query --profile dev --stdin --password-env MY_DB_PASSWORD --format json\n\nProgress/notices go to stderr. JSON includes all retained result sets; CSV needs --result-index when multiple sets occur. SQL is limited to 64 KiB; driver retention is 512 KiB / 4096 events (truncation exits 5)."
)]
struct Query {
    /// Exact profile ID; omitted uses the explicitly selected profile.
    #[arg(long)]
    profile: Option<String>,
    /// Explicit database override (SQLite accepts only main).
    #[arg(long)]
    database: Option<String>,
    /// Refuse changed SQL before authentication or connection.
    #[arg(long)]
    expect_sql_sha256: Option<String>,
    /// Refuse changed profile endpoint before authentication or connection.
    #[arg(long)]
    expect_target_fingerprint: Option<String>,
    #[command(flatten)]
    input: SqlInput,
    #[command(flatten)]
    auth: Auth,
    #[arg(long, value_enum, default_value = "table")]
    format: Format,
    #[command(flatten)]
    destination: Destination,
    #[arg(long, default_value_t=30, value_parser=clap::value_parser!(u64).range(1..=600))]
    timeout_secs: u64,
    /// Enable the adapter's session read-only setting for this invocation.
    #[arg(long)]
    read_only: bool,
    /// Zero-based result-set index. Required for CSV when there are multiple sets.
    #[arg(long)]
    result_index: Option<usize>,
    /// Explicit NULL marker; collision with a real text value is rejected.
    #[arg(long, default_value = "\\N")]
    null_marker: String,
    /// Display at most this many rows per result set in table format.
    #[arg(long, default_value_t=100, value_parser=clap::value_parser!(u64).range(1..=4096))]
    max_rows: u64,
}
#[derive(Args)]
struct Schema {
    #[arg(long)]
    profile: Option<String>,
    #[command(flatten)]
    auth: Auth,
    #[arg(long, default_value_t=30, value_parser=clap::value_parser!(u64).range(1..=600))]
    timeout_secs: u64,
    #[command(flatten)]
    destination: Destination,
}
#[derive(Subcommand)]
enum SqlCommand {
    Format {
        #[command(flatten)]
        input: SqlInput,
        #[arg(long, value_enum, default_value = "postgres")]
        dialect: Engine,
        #[command(flatten)]
        destination: Destination,
    },
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let json_errors = matches!(cli.command, Command::Workspace(_) | Command::Agent { .. });
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    if let Err(e) = ctrlc::set_handler(move || {
        signal.store(true, Ordering::Relaxed);
    }) {
        eprintln!("dbide: signal handler: {e}");
        return 2.into();
    }
    match run(cli, &cancelled) {
        Ok(()) => 0.into(),
        Err(e) => {
            if json_errors {
                println!(
                    "{}",
                    serde_json::json!({"schema_version":1,"ok":false,"error":{"code":e.machine_code,"message":e.message}})
                );
            } else {
                eprintln!("dbide: {}", e.message);
            }
            e.code.into()
        }
    }
}
fn store(path: Option<PathBuf>) -> Result<ProfileStore> {
    match path {
        Some(path) => ProfileStore::open(path),
        None => ProfileStore::open_default(),
    }
    .map_err(config)
}
fn read_bounded(reader: impl Read, limit: usize) -> Result<String> {
    let mut bytes = vec![];
    reader
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(config)?;
    if bytes.len() > limit {
        return Err(config(format!(
            "input exceeds {limit} byte limit; nothing submitted"
        )));
    }
    String::from_utf8(bytes).map_err(|_| config("input must be UTF-8"))
}
fn sql(input: SqlInput) -> Result<String> {
    let value = if let Some(value) = input.sql {
        value
    } else if let Some(path) = input.file {
        let mut options = fs::OpenOptions::new();
        options.read(true);
        // Opening a replaced FIFO must not block before the descriptor's type check.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = options.open(path).map_err(config)?;
        if !file.metadata().map_err(config)?.is_file() {
            return Err(config(
                "SQL --file input must be a regular file; nothing submitted",
            ));
        }
        read_bounded(file, dbide_profiles::MAX_SQL_BYTES)?
    } else {
        read_bounded(io::stdin().lock(), dbide_profiles::MAX_SQL_BYTES)?
    };
    if value.len() > dbide_profiles::MAX_SQL_BYTES || value.trim().is_empty() {
        return Err(config(
            "SQL must be nonempty and at most 65536 UTF-8 bytes; nothing submitted",
        ));
    }
    Ok(value)
}
fn password(auth: Auth, id: &str) -> Result<Secret> {
    if let Some(name) = auth.password_env {
        let value = std::env::var(name)
            .map_err(|_| config("explicit password environment variable missing or not UTF-8"))?;
        if value.len() > dbide_credentials::MAX_SECRET_BYTES {
            return Err(config("password exceeds 64 KiB"));
        }
        Ok(Secret::new(value))
    } else if auth.password_stdin {
        let mut value = read_bounded(io::stdin().lock(), dbide_credentials::MAX_SECRET_BYTES)?;
        if value.ends_with('\n') {
            value.pop();
            if value.ends_with('\r') {
                value.pop();
            }
        }
        Ok(Secret::new(value))
    } else if auth.keychain {
        MacKeychainStore
            .load(id)
            .map_err(config)?
            .ok_or_else(|| config("no credential stored for this exact profile ID"))
    } else {
        Ok(Secret::new(Vec::new()))
    }
}
fn profile(state: &WorkspaceState, id: Option<&str>) -> Result<ConnectionProfile> {
    let id = id
        .or(state.selected_profile_id.as_deref())
        .ok_or_else(|| config("provide --profile ID or use profiles select ID"))?;
    state
        .profiles
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .ok_or_else(|| config("profile ID does not exist"))
}
fn request(p: ConnectionProfile, auth: Auth) -> Result<ConnectionRequest> {
    let secret = password(auth, &p.id)?;
    Ok(ConnectionRequest {
        profile: p,
        password: secret.expose_str().map_err(config)?.into(),
    })
}
fn update(
    store: &ProfileStore,
    action: impl FnOnce(&mut WorkspaceState) -> Result<()>,
) -> Result<()> {
    // GUI and CLI share this lock-scoped read/modify/write transaction.
    store
        .update(|state| {
            action(state).map_err(|error| dbide_profiles::StoreError::InvalidState(error.message))
        })
        .map_err(config)?;
    Ok(())
}
fn apply(fields: ProfileFields, p: &mut ConnectionProfile, editing: bool) -> Result<()> {
    if editing && fields.id.is_some() {
        return Err(config("profile ID is immutable; omit --id while editing"));
    }
    if let Some(v) = fields.engine {
        p.engine = match v {
            Engine::Postgres => DatabaseEngine::PostgreSql,
            Engine::Sqlite => DatabaseEngine::Sqlite,
            Engine::Mysql => DatabaseEngine::MySql,
        };
        if !editing && fields.name.is_none() {
            p.name = match v {
                Engine::Postgres => "Local PostgreSQL",
                Engine::Sqlite => "Local SQLite",
                Engine::Mysql => "Local MySQL",
            }
            .into();
        }
        if !editing && matches!(v, Engine::Mysql) {
            p.port = 3306;
            p.database = "mysql".into();
            p.username = "root".into();
        }
    }
    if let Some(v) = fields.id {
        p.id = v;
    }
    if let Some(v) = fields.name {
        p.name = v;
    }
    if let Some(v) = fields.sqlite_path {
        // Make profiles independent of the shell/UI working directory.
        p.sqlite_path = Some(if v != ":memory:" && Path::new(&v).is_relative() {
            std::env::current_dir()
                .map_err(config)?
                .join(v)
                .to_str()
                .ok_or_else(|| config("SQLite path must be UTF-8"))?
                .to_owned()
        } else {
            v
        });
    }
    if let Some(v) = fields.sqlite_create {
        p.sqlite_create = v;
    }
    if let Some(v) = fields.read_only {
        p.read_only = v;
    }
    if let Some(v) = fields.host {
        p.host = v;
    }
    if let Some(v) = fields.port {
        p.port = v;
    }
    if let Some(v) = fields.database {
        p.database = v;
    }
    if let Some(v) = fields.username {
        p.username = v;
    }
    if let Some(v) = fields.tls {
        p.tls_mode = match v {
            Tls::Disable => TlsMode::Disable,
            Tls::Require => TlsMode::Require,
            Tls::VerifyFull => TlsMode::VerifyFull,
        };
    }
    if let Some(v) = fields.root_cert_path {
        p.root_cert_path = if v.is_empty() { None } else { Some(v) };
    }
    if let Some(v) = fields.connect_timeout_secs {
        p.connect_timeout_secs = v;
    }
    p.validate().map_err(config)
}
fn run(cli: Cli, cancelled: &Arc<AtomicBool>) -> Result<()> {
    if let Command::Workspace(command) = cli.command {
        return workspace::run(command, cli.config);
    }
    if let Command::Agent { command } = cli.command {
        return workspace::agent(command);
    }
    if let Command::Sql {
        command:
            SqlCommand::Format {
                input,
                dialect,
                destination,
            },
    } = cli.command
    {
        output::check(&destination)?;
        let dialect = match dialect {
            Engine::Postgres => dbide_sql::Dialect::PostgreSql,
            Engine::Sqlite => dbide_sql::Dialect::SQLite,
            Engine::Mysql => dbide_sql::Dialect::MySql,
        };
        let formatted = dbide_sql::format_sql(&sql(input)?, dialect).map_err(config)?;
        return output::publish(&destination, formatted.as_bytes());
    }
    if let Command::Config {
        command: ConfigCommand::Language { command },
    } = cli.command
    {
        let profile_path = match cli.config {
            Some(path) => path,
            None => ProfileStore::default_path().map_err(config)?,
        };
        let settings = SettingsStore::for_profile_path(profile_path);
        let value = match command {
            LanguageCommand::Get => settings.load().map_err(config)?,
            LanguageCommand::Set { locale } => settings
                .set_locale(match locale {
                    Language::Ko => Locale::Ko,
                    Language::En => Locale::En,
                })
                .map_err(config)?,
        };
        println!("{}", value.locale.code());
        return Ok(());
    }
    if let Command::Catalog { command } = cli.command {
        let profile_path = match cli.config {
            Some(path) => path,
            None => ProfileStore::default_path().map_err(config)?,
        };
        return catalog(profile_path, command);
    }
    let store = store(cli.config)?;
    match cli.command {
        Command::Profiles { command } => match command {
            ProfileCommand::List => {
                let state = store.load().map_err(config)?;
                println!("{}",serde_json::to_string_pretty(&serde_json::json!({"profiles":state.profiles,"selected_profile_id":state.selected_profile_id})).map_err(config)?);
                Ok(())
            }
            ProfileCommand::Add(fields) => {
                let mut p = ConnectionProfile::new();
                apply(fields, &mut p, false)?;
                let id = p.id.clone();
                update(&store, move |state| {
                    if state.profiles.iter().any(|v| v.id == p.id) {
                        return Err(config("profile ID already exists; use profiles edit"));
                    }
                    state.upsert(p).map_err(config)
                })?;
                println!("{id}");
                Ok(())
            }
            ProfileCommand::Edit {
                profile_id: id,
                fields,
            } => update(&store, move |state| {
                let mut p = profile(state, Some(&id))?;
                apply(fields, &mut p, true)?;
                state.upsert(p).map_err(config)
            }),
            ProfileCommand::Select { id } => {
                update(&store, |s| s.select(Some(&id)).map_err(config))
            }
            ProfileCommand::Remove { id } => update(&store, |s| {
                profile(s, Some(&id))?;
                let backup = output::backup(store.path())?;
                eprintln!("Profile backup: {}", backup.display());
                s.remove(&id);
                Ok(())
            }),
            ProfileCommand::Test {
                id,
                auth,
                timeout_secs,
            } => {
                let p = profile(&store.load().map_err(config)?, Some(&id))?;
                let mut bridge = DatabaseBridge::new().map_err(config)?;
                let op = bridge.test_connection(request(p, auth)?).map_err(submit)?;
                let result = wait(&mut bridge, op, timeout_secs, cancelled);
                cleanup(&mut bridge);
                match result? {
                    BridgeEvent::ConnectionTested { .. } => {
                        println!("Connection successful");
                        Ok(())
                    }
                    _ => Err(Failure::new(3, "connection test ended unexpectedly")),
                }
            }
        },
        Command::Config { command } => match command {
            ConfigCommand::Show => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&store.load().map_err(config)?).map_err(config)?
                );
                Ok(())
            }
            ConfigCommand::Set { key: _, value } => update(&store, |s| {
                s.persist_history = value;
                if !value {
                    s.history.clear();
                }
                Ok(())
            }),
            ConfigCommand::Language { .. } => unreachable!(),
        },
        Command::Query(q) => query(&store, q, cancelled),
        Command::Export(e) => export::run(&store, e, cancelled.clone()),
        Command::Schema(s) => schema(&store, s, cancelled),
        Command::Catalog { .. } => unreachable!(),
        Command::Sql { .. } => unreachable!(),
        Command::Workspace(_) | Command::Agent { .. } => unreachable!(),
    }
}
fn catalog(profile_path: PathBuf, command: CatalogCommand) -> Result<()> {
    let parent = profile_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut store = dbide_catalog_store::Store::new(parent.join("catalog-cache"));
    let value = match command {
        CatalogCommand::List => {
            serde_json::to_value(store.list().map_err(config)?).map_err(config)?
        }
        CatalogCommand::Show { shard_id } => {
            let valid = shard_id.strip_suffix(".json").is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
            if !valid {
                return Err(config(
                    "invalid catalog shard ID; use the exact ID from catalog list, not a path",
                ));
            }
            let (key, snapshot) =
                store
                    .read_by_id(&shard_id)
                    .map_err(config)?
                    .ok_or_else(|| {
                        config("catalog shard unavailable; use catalog list to check its exact ID")
                    })?;
            serde_json::json!({"shard_id":shard_id,"historical":true,"key":key,"snapshot":snapshot})
        }
    };
    let bytes = serde_json::to_vec_pretty(&value).map_err(config)?;
    output::publish(
        &Destination {
            output: None,
            force: false,
        },
        &bytes,
    )
}
fn submit(e: dbide_bridge::BridgeSubmitError) -> Failure {
    Failure::new(4, format!("bridge rejected request: {e:?}"))
}
fn wait(
    bridge: &mut DatabaseBridge,
    id: OperationId,
    seconds: u64,
    cancelled: &AtomicBool,
) -> Result<BridgeEvent> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        if cancelled.load(Ordering::Relaxed) || Instant::now() >= deadline {
            let interrupted = cancelled.load(Ordering::Relaxed);
            let _ = bridge.cancel(id);
            return Err(Failure::new(
                if interrupted { 130 } else { 4 },
                if interrupted {
                    "interrupted; targeted cancel requested; dispatched SQL is never replayed"
                } else {
                    "operation timed out; targeted cancel requested; dispatched SQL may have taken effect; inspect database before retrying"
                },
            ));
        }
        while let Some(event) = bridge.try_next() {
            match &event {
                BridgeEvent::Connecting { operation, .. } if *operation == id => {
                    eprintln!("Connecting…")
                }
                BridgeEvent::Executing { operation, .. } if *operation == id => {
                    eprintln!("Executing…")
                }
                BridgeEvent::ConnectionFailed {
                    operation,
                    diagnostic,
                } if *operation == id => {
                    return Err(Failure::new(
                        3,
                        format!("{}; {}", diagnostic.message, diagnostic.hint),
                    ));
                }
                BridgeEvent::Error { operation, message }
                    if operation.is_none_or(|op| op == id) =>
                {
                    return Err(Failure::new(4, message));
                }
                BridgeEvent::Configured { operation }
                | BridgeEvent::ConnectionTested { operation }
                | BridgeEvent::SchemaLoaded { operation, .. }
                | BridgeEvent::Finished { operation, .. }
                    if *operation == id =>
                {
                    return Ok(event);
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn cleanup(bridge: &mut DatabaseBridge) {
    if let Ok(id) = bridge.disconnect() {
        let deadline = Instant::now() + Duration::from_secs(9);
        while Instant::now() < deadline {
            while let Some(event) = bridge.try_next() {
                if matches!(event,BridgeEvent::Disconnected{operation:Some(op)} if op==id) {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        eprintln!(
            "Cleanup deadline reached; worker shutdown requested; SQL outcome may be unknown"
        );
    }
}
fn configure_bridge(
    bridge: &mut DatabaseBridge,
    request: ConnectionRequest,
    seconds: u64,
    cancelled: &AtomicBool,
) -> Result<()> {
    let id = bridge.configure(request).map_err(submit)?;
    wait(bridge, id, seconds, cancelled)?;
    Ok(())
}
fn query(store: &ProfileStore, q: Query, cancelled: &AtomicBool) -> Result<()> {
    if q.input.stdin && q.auth.password_stdin {
        return Err(config(
            "SQL stdin and password stdin are mutually exclusive",
        ));
    }
    if matches!(q.format, Format::Json) && q.result_index.is_some() {
        return Err(config(
            "--result-index is for table/csv; JSON always includes the full report",
        ));
    }
    if matches!(q.format, Format::Csv) && q.null_marker.is_empty() {
        return Err(config("CSV NULL marker must not be empty"));
    }
    let sql = sql(q.input)?;
    if q.expect_sql_sha256
        .as_deref()
        .is_some_and(|expected| expected != dbide_project::digest_sql(&sql))
    {
        return Err(config("SQL SHA-256 changed; nothing submitted"));
    }
    let state = store.load().map_err(config)?;
    let mut p = profile(&state, q.profile.as_deref())?;
    if q.expect_target_fingerprint.is_some() {
        workspace::validate_target_profile(&p)?;
    }
    if let Some(database) = q.database {
        if p.engine == DatabaseEngine::Sqlite && database != "main" {
            return Err(config("SQLite database must be main; nothing submitted"));
        }
        if p.engine != DatabaseEngine::Sqlite {
            p.database = database;
        }
    }
    let database = if p.engine == DatabaseEngine::Sqlite {
        "main"
    } else {
        &p.database
    };
    let original_target = StoredExecutionTarget::capture(&p, database).map_err(config)?;
    if q.expect_target_fingerprint
        .as_deref()
        .is_some_and(|expected| expected != original_target.endpoint_fingerprint)
    {
        return Err(config("target fingerprint changed; nothing submitted"));
    }
    output::check(&q.destination)?;
    p.read_only |= q.read_only;
    let id = p.id.clone();
    // Capture the exact submitted request before ownership passes to the worker;
    // later profile edits/selection must not change this execution's identity.
    let database = if p.engine == DatabaseEngine::Sqlite {
        "main"
    } else {
        &p.database
    };
    let execution_target = StoredExecutionTarget::capture(&p, database).map_err(config)?;
    let request = request(p, q.auth)?;
    let mut bridge = DatabaseBridge::new().map_err(config)?;
    configure_bridge(&mut bridge, request, q.timeout_secs, cancelled)?;
    let op = bridge.execute(sql.clone()).map_err(submit)?;
    let finished = wait(&mut bridge, op, q.timeout_secs, cancelled);
    cleanup(&mut bridge);
    let BridgeEvent::Finished { report, .. } = finished? else {
        return Err(Failure::new(4, "execution ended unexpectedly"));
    };
    for notice in &report.notices {
        eprintln!("{}: {}", notice.severity, notice.message);
    }
    if report.truncated {
        eprintln!(
            "WARNING: driver result truncated (retained cap: 512 KiB / 4096 events). Output is incomplete; exit 5."
        );
    }
    let encoded = output::report(
        &report,
        q.format,
        q.result_index,
        &q.null_marker,
        q.max_rows as usize,
    )?;
    // Failed SQL may expose partial retained data on stdout, but must preserve existing output files.
    let succeeded = report.outcome == Outcome::Completed && !report.truncated;
    if q.destination.output.is_none() || succeeded {
        output::publish(&q.destination, &encoded)?;
    }
    // Local history is ancillary: never replace a finished database outcome with
    // a settings error, or hide the report and invite an accidental SQL replay.
    if state.persist_history {
        let row_count = report
            .events
            .iter()
            .filter(|e| matches!(e, dbide_driver_api::Event::Row { .. }))
            .count() as u64;
        if let Err(error) = update(store, |state| {
            state
                .record_history(HistoryEntry {
                    profile_id: id,
                    sql,
                    executed_at_unix_ms: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(config)?
                        .as_millis() as u64,
                    outcome: format!("{:?}", report.outcome),
                    row_count: Some(row_count),
                    execution_target: Some(execution_target),
                })
                .map_err(config)
        }) {
            eprintln!(
                "WARNING: SQL already executed; history persistence failed: {}; original outcome {:?} is unchanged. Do not replay SQL to repair history.",
                error.message, report.outcome
            );
        }
    }
    match report.outcome {
        Outcome::Completed if report.truncated => Err(Failure::new(
            5,
            "query completed; retained result is incomplete",
        )),
        Outcome::Completed => Ok(()),
        Outcome::Cancelled => Err(Failure::new(130, "query cancelled")),
        _ => Err(Failure::new(
            4,
            report
                .terminal_error
                .map(|e| e.message)
                .unwrap_or_else(|| format!("query outcome: {:?}", report.outcome)),
        )),
    }
}
fn schema(store: &ProfileStore, s: Schema, cancelled: &AtomicBool) -> Result<()> {
    output::check(&s.destination)?;
    let p = profile(&store.load().map_err(config)?, s.profile.as_deref())?;
    let mut bridge = DatabaseBridge::new().map_err(config)?;
    configure_bridge(&mut bridge, request(p, s.auth)?, s.timeout_secs, cancelled)?;
    let op = bridge.inspect_schema().map_err(submit)?;
    let result = wait(&mut bridge, op, s.timeout_secs, cancelled);
    cleanup(&mut bridge);
    let BridgeEvent::SchemaLoaded {
        objects, truncated, ..
    } = result?
    else {
        return Err(Failure::new(4, "schema operation ended unexpectedly"));
    };
    let objects:Vec<_>=objects.into_iter().map(|o|serde_json::json!({"schema":o.schema,"name":o.name,"kind":o.kind,"columns":o.columns.into_iter().map(|c|serde_json::json!({"name":c.name,"data_type":c.data_type,"nullable":c.nullable})).collect::<Vec<_>>()})).collect();
    if !truncated || s.destination.output.is_none() {
        output::publish(
            &s.destination,
            &serde_json::to_vec_pretty(
                &serde_json::json!({"objects":objects,"truncated":truncated}),
            )
            .map_err(config)?,
        )?;
    }
    if truncated {
        Err(Failure::new(5, "schema results truncated"))
    } else {
        Ok(())
    }
}
