//! Thin offline dispatch into the shared portable configuration service.
use super::*;
use dbide_portable::Bundle;

#[derive(Args)]
pub(super) struct Export {
    /// Fresh portable JSON file. Existing files are never replaced.
    #[arg(long)]
    output: PathBuf,
    /// Absolute root without traversal used to encode contained SQLite/CA paths as relative paths.
    #[arg(long)]
    base_dir: PathBuf,
}

#[derive(Args)]
pub(super) struct Inspect {
    /// Portable JSON file; validated under the shared byte/profile limits.
    #[arg(long)]
    file: PathBuf,
}

#[derive(Args)]
pub(super) struct Import {
    #[arg(long)]
    file: PathBuf,
    /// Absolute root without traversal used to resolve portable relative paths.
    #[arg(long)]
    path_root: PathBuf,
    /// Absolute fresh configuration directory. Existing directories are never replaced.
    #[arg(long)]
    destination: PathBuf,
    /// Exact external replacement with absolute destination, repeatable; ORIGINAL=DESTINATION.
    #[arg(long = "map", value_name = "ORIGINAL=DESTINATION")]
    mappings: Vec<String>,
}

fn emit(value: serde_json::Value) -> Result<()> {
    println!("{}", serde_json::to_string(&value).map_err(config)?);
    Ok(())
}

pub(super) fn export(source: Option<PathBuf>, args: Export) -> Result<()> {
    let source = match source {
        Some(path) => path,
        None => ProfileStore::default_path().map_err(config)?,
    };
    let state = ProfileStore::open(source.clone())
        .map_err(config)?
        .load()
        .map_err(config)?;
    let settings = SettingsStore::for_profile_path(&source)
        .load()
        .map_err(config)?;
    let recovery = dbide_portable::read_recovery_enabled(&source).map_err(config)?;
    let bundle = Bundle::capture(&state, settings, recovery, &args.base_dir).map_err(config)?;
    bundle.write_new(&args.output).map_err(config)?;
    emit(
        serde_json::json!({"schema_version":1,"ok":true,"output":args.output,"inspection":bundle.inspect()}),
    )
}

pub(super) fn inspect(args: Inspect) -> Result<()> {
    let bundle = Bundle::read(&args.file).map_err(config)?;
    emit(serde_json::json!({"schema_version":1,"ok":true,"inspection":bundle.inspect()}))
}

pub(super) fn import(args: Import) -> Result<()> {
    let bundle = Bundle::read(&args.file).map_err(config)?;
    let mappings = args
        .mappings
        .iter()
        .map(|mapping| {
            let (original, destination) = mapping
                .split_once('=')
                .ok_or_else(|| config("--map must use ORIGINAL=DESTINATION"))?;
            Ok(dbide_portable::ExternalPathMapping {
                original: original.into(),
                destination: destination.into(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let report = bundle
        .restore_new_directory_with_mappings(&args.destination, Some(&args.path_root), &mappings)
        .map_err(config)?;
    emit(serde_json::json!({"schema_version":1,"ok":true,"report":report}))
}
