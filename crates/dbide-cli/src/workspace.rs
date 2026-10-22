//! Offline, explicit-root workspace commands and bounded agent discovery.
use super::{DatabaseEngine, Failure, Path, PathBuf, ProfileStore, Result, StoredExecutionTarget};
use clap::{Args, Subcommand};
use dbide_project::{Project, ProjectError};
use serde_json::{Value, json};

#[derive(Args)]
pub(crate) struct Workspace {
    /// Explicit existing project directory. No ancestor discovery.
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: WorkspaceCommand,
}

#[derive(Subcommand)]
enum WorkspaceCommand {
    Init {
        #[arg(long)]
        name: String,
    },
    Show,
    Files {
        #[arg(long, default_value = ".")]
        directory: PathBuf,
    },
    Read {
        #[arg(long)]
        file: PathBuf,
    },
    Write {
        #[arg(long)]
        file: PathBuf,
        #[arg(
            long,
            allow_hyphen_values = true,
            required_unless_present = "stdin",
            conflicts_with = "stdin"
        )]
        sql: Option<String>,
        #[arg(long)]
        stdin: bool,
        /// Omit for create-only; provide last read digest to replace a file.
        #[arg(long)]
        expect_sha256: Option<String>,
    },
    Mkdir {
        #[arg(long)]
        path: PathBuf,
    },
    Bind {
        /// Exact profile ID from the explicit --config or platform profiles file.
        #[arg(long)]
        profile: String,
        #[arg(long)]
        database: Option<String>,
        #[arg(long)]
        expect_revision: u64,
    },
    Context,
    /// Produce guarded query argv; never authenticates, connects, or executes.
    Plan {
        #[arg(long)]
        file: PathBuf,
    },
}

#[derive(Subcommand)]
pub(crate) enum AgentCommand {
    /// Print versioned commands, effects, limits, and exit codes without I/O.
    Capabilities,
}

fn failure(code: &'static str, message: impl ToString) -> Failure {
    Failure {
        code: 2,
        machine_code: code,
        message: message.to_string(),
    }
}

fn project_error(error: ProjectError) -> Failure {
    failure(error.code(), error)
}

fn profile_error(_: impl ToString) -> Failure {
    failure(
        "profile_configuration",
        "Cannot read the selected profiles configuration; no database connection attempted",
    )
}

fn emit(mut value: Value) -> Result<()> {
    value["schema_version"] = json!(1);
    value["ok"] = json!(true);
    println!("{}", serde_json::to_string(&value).map_err(super::config)?);
    Ok(())
}

fn config_path(config: Option<PathBuf>) -> Result<PathBuf> {
    let path = match config {
        Some(path) => path,
        None => ProfileStore::default_path().map_err(profile_error)?,
    };
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir().map_err(profile_error)?.join(path))
    }
}

fn exact_profile(path: &Path, id: &str) -> Result<super::ConnectionProfile> {
    let store = ProfileStore::open(path.to_path_buf()).map_err(profile_error)?;
    let state = store.load().map_err(profile_error)?;
    state
        .profiles
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| {
            failure(
                "profile_not_found",
                "Bound profile ID does not exist; explicitly bind an available profile",
            )
        })
}

fn resolve(path: &Path, target: &StoredExecutionTarget) -> Result<super::ConnectionProfile> {
    let profile = exact_profile(path, &target.profile_id)?;
    validate_target_profile(&profile)?;
    target.resolve(&profile).map_err(|_| {
        failure(
            "target_changed",
            "Bound profile endpoint changed or is invalid; explicitly bind the target again",
        )
    })
}

/// Relative SQLite paths can identify different databases when the caller's cwd changes.
/// Preserve legacy query behavior unless the caller explicitly requests target guarding.
pub(super) fn validate_target_profile(profile: &super::ConnectionProfile) -> Result<()> {
    if dbide_project::validate_target_profile(profile).is_err() {
        return Err(failure(
            "relative_sqlite_path",
            "Guarded SQLite targets require an absolute SQLite path or :memory:; update the profile explicitly",
        ));
    }
    Ok(())
}

fn limits() -> Value {
    json!({"sql_bytes":dbide_project::MAX_SQL_BYTES,"manifest_bytes":dbide_project::MAX_MANIFEST_BYTES,
        "directory_entries":dbide_project::MAX_LIST_ENTRIES,"directory_scan_entries":dbide_project::MAX_SCANNED_ENTRIES,
        "context_entries":128,"portable_bundle_bytes":dbide_portable::MAX_BUNDLE_BYTES,"query_retained_bytes":524288,"query_retained_events":4096})
}

pub(crate) fn agent(_: AgentCommand) -> Result<()> {
    emit(json!({"commands":[
        {"command":"agent capabilities","effect":"offline_discovery","database_connection":false},
        {"command":"workspace init","effect":"create_manifest_and_queries","database_connection":false,"required":["--root","--name"]},
        {"command":"workspace show","effect":"read_manifest","database_connection":false,"required":["--root"]},
        {"command":"workspace files","effect":"bounded_directory_listing","database_connection":false,"required":["--root"]},
        {"command":"workspace read","effect":"read_sql_content","database_connection":false,"required":["--root","--file"]},
        {"command":"workspace write","effect":"create_or_compare_and_swap_sql","database_connection":false,"required":["--root","--file","--sql or --stdin"]},
        {"command":"workspace mkdir","effect":"create_directory","database_connection":false,"required":["--root","--path"]},
        {"command":"workspace bind","effect":"compare_and_swap_target_reference","database_connection":false,"required":["--root","--profile","--expect-revision"]},
        {"command":"workspace context","effect":"bounded_metadata_only","database_connection":false,"required":["--root"]},
        {"command":"workspace plan","effect":"guarded_query_argv_only","database_connection":false,"required":["--root","--file"]},
        {"command":"profiles list/add/edit/remove/select","effect":"local_profiles_read_or_write","database_connection":false},
        {"command":"profiles test","effect":"explicit_connection_test","database_connection":true},
        {"command":"config","effect":"local_settings_read_or_write","database_connection":false},
        {"command":"config export","effect":"password_free_portable_config_to_fresh_json","database_connection":false,"required":["--output","--base-dir"],"source":"--profiles (alias of --config)","excludes":["credentials","history","sql_buffers","database_data","unknown_settings"]},
        {"command":"config inspect","effect":"bounded_portable_config_validation","database_connection":false,"required":["--file"],"changes_active_source":false},
        {"command":"config import","effect":"portable_config_to_fresh_directory","database_connection":false,"required":["--file","--path-root","--destination"],"optional":["--map ORIGINAL=DESTINATION (repeatable)"],"changes_active_source":false,"overwrite":false},
        {"command":"catalog list/show","effect":"read_saved_metadata","database_connection":false},
        {"command":"sql format","effect":"format_sql_and_optional_output_file","database_connection":false},
        {"command":"query","effect":"execute_arbitrary_sql_and_optional_output_or_history","database_connection":true,"guards":["--expect-sql-sha256","--expect-target-fingerprint"],"read_only":"adapter session setting; not a SQL safety proof"},
        {"command":"export","effect":"explicit_single_statement_stream_to_fresh_file","database_connection":true,"engines":["sqlite","postgres","mysql"],"formats":["csv"],"required":["--output","--sql or --file or --stdin"],"guards":["--expect-sql-sha256","--expect-target-fingerprint"],"read_only":"independent SQLite READ_ONLY+authorizer or network READ ONLY transaction; restricted SELECT subset; no replay or retained-result continuation","limits":["--max-rows","--max-row-bytes","--max-bytes","--timeout-secs"],"publication":"success-only atomic create; no overwrite"},
        {"command":"schema","effect":"read_server_catalog_and_optional_output_file","database_connection":true}
    ],"limits":limits(),"workspace_contract":{"stdout":"one JSON object; schema_version=1, ok, result fields or error{code,message}","errors_exit_code":2,"clap_argument_errors":"stderr usage text, exit 2","root_discovery":false,"default_write":"create_only","overwrite":"--expect-sha256 digest","execution":"plan never executes; invoke its argv explicitly"},
    "exit_codes":{"0":"success","2":"arguments/configuration/conflict/guard refusal","3":"connection failure","4":"execution/timeout/bridge failure","5":"truncated result","130":"interrupted/cancelled"}}))
}

pub(crate) fn run(args: Workspace, config: Option<PathBuf>) -> Result<()> {
    let root = args.root.ok_or_else(|| {
        failure(
            "root_required",
            "Provide an explicit --root PATH; ancestor discovery is disabled",
        )
    })?;
    let project = match &args.command {
        WorkspaceCommand::Init { name } => Project::init(&root, name),
        _ => Project::open(&root),
    }
    .map_err(project_error)?;
    let root = project.root();
    match args.command {
        WorkspaceCommand::Init { .. } | WorkspaceCommand::Show => {
            emit(json!({"root":root,"manifest":project.manifest().map_err(project_error)?}))
        }
        WorkspaceCommand::Files { directory } => {
            emit(json!({"root":root,"listing":project.list(&directory).map_err(project_error)?}))
        }
        WorkspaceCommand::Read { file } => {
            emit(json!({"root":root,"file":project.read_sql(&file).map_err(project_error)?}))
        }
        WorkspaceCommand::Write {
            file,
            sql,
            stdin: _,
            expect_sha256,
        } => {
            let sql = match sql {
                Some(sql) => sql,
                None => super::read_bounded(std::io::stdin().lock(), dbide_project::MAX_SQL_BYTES)?,
            };
            let saved = project
                .write_sql(&file, &sql, expect_sha256.as_deref())
                .map_err(project_error)?;
            emit(
                json!({"root":root,"file":{"relative_path":saved.relative_path,"sha256":saved.sha256,"bytes":saved.sql.len()}}),
            )
        }
        WorkspaceCommand::Mkdir { path } => {
            let created = project.mkdir(&path).map_err(project_error)?;
            emit(json!({"root":root,"directory":created}))
        }
        WorkspaceCommand::Bind {
            profile,
            database,
            expect_revision,
        } => {
            let path = config_path(config)?;
            let p = exact_profile(&path, &profile)?;
            validate_target_profile(&p)?;
            let database = database
                .as_deref()
                .unwrap_or(if p.engine == DatabaseEngine::Sqlite {
                    "main"
                } else {
                    &p.database
                });
            let target = StoredExecutionTarget::capture(&p, database).map_err(|_| {
                failure(
                    "invalid_target",
                    "Invalid explicit target; SQLite database must be main",
                )
            })?;
            let manifest = project
                .set_default_target(Some(target), expect_revision)
                .map_err(project_error)?;
            emit(json!({"root":root,"manifest":manifest}))
        }
        WorkspaceCommand::Context => {
            let manifest = project.manifest().map_err(project_error)?;
            let resolution = if let Some(target) = &manifest.default_target {
                match config_path(config).and_then(|path| resolve(&path, target)) {
                    Ok(_) => json!({"status":"resolved"}),
                    Err(error) => json!({"status":"unresolved","code":error.machine_code}),
                }
            } else {
                json!({"status":"unbound"})
            };
            let mut listing = project.list(Path::new(".")).map_err(project_error)?;
            if listing.entries.len() > 128 {
                listing.entries.truncate(128);
                listing.truncated = true;
            }
            emit(
                json!({"root":root,"manifest":manifest,"target_resolution":resolution,"files":listing,
                "capabilities":["list","read_sql","create_sql","cas_write_sql","mkdir","bind","plan"],"limits":limits(),
                "included":{"sql_content":false,"history":false,"database_data":false,"passwords":false},"database_connection":false}),
            )
        }
        WorkspaceCommand::Plan { file } => {
            let manifest = project.manifest().map_err(project_error)?;
            let target = manifest.default_target.ok_or_else(|| {
                failure(
                    "target_required",
                    "Workspace has no default target; explicitly bind a profile before planning",
                )
            })?;
            let path = config_path(config)?;
            let _effective = resolve(&path, &target)?;
            let path = std::fs::canonicalize(path).map_err(profile_error)?;
            let loaded = project.read_sql(&file).map_err(project_error)?;
            if loaded.sql.trim().is_empty() {
                return Err(failure(
                    "invalid_sql",
                    "SQL must be nonempty before planning execution",
                ));
            }
            let config = path
                .to_str()
                .ok_or_else(|| failure("invalid_path", "Configuration path must be UTF-8"))?;
            let file = loaded
                .absolute_path
                .to_str()
                .ok_or_else(|| failure("invalid_path", "SQL path must be UTF-8"))?;
            let argv = vec![
                "dbide",
                "--config",
                config,
                "query",
                "--profile",
                &target.profile_id,
                "--database",
                &target.database,
                "--file",
                file,
                "--expect-sql-sha256",
                &loaded.sha256,
                "--expect-target-fingerprint",
                &target.endpoint_fingerprint,
                "--read-only",
                "--format",
                "json",
            ];
            emit(
                json!({"root":root,"executed":false,"database_connection":false,"read_only":true,
                "intended_target":{"profile_id":target.profile_id,"database":target.database,"engine":target.engine,
                "endpoint_fingerprint":target.endpoint_fingerprint,"sql_sha256":loaded.sha256,"sql_bytes":loaded.sql.len()},
                "file":loaded.relative_path,"argv":argv}),
            )
        }
    }
}
