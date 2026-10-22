use dbide_profiles::{HistoryEntry, ProfileStore};
use dbide_settings::{AppSettings, Locale, SettingsStore};
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use tempfile::TempDir;

fn run(source: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dbide"))
        .arg("--profiles")
        .arg(source)
        .args(args)
        .env_remove("PGPASSWORD")
        .env_remove("DATABASE_URL")
        .env_remove("MYSQL_PWD")
        .output()
        .unwrap()
}

fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn fixture(dir: &Path) -> std::path::PathBuf {
    let source = dir.join("profiles.json");
    let added = run(
        &source,
        &[
            "profiles",
            "add",
            "--id",
            "fixture",
            "--name",
            "Portable fixture",
            "--engine",
            "sqlite",
            "--sqlite-path",
            dir.join("fixture.db").to_str().unwrap(),
        ],
    );
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let store = ProfileStore::open(source.clone()).unwrap();
    store
        .update(|state| {
            state.select(Some("fixture"))?;
            state.persist_history = true;
            state.record_history(HistoryEntry {
                profile_id: "fixture".into(),
                sql: "SELECT 'HISTORY_SQL_SENTINEL';".into(),
                executed_at_unix_ms: 1,
                outcome: "Completed".into(),
                row_count: Some(1),
                execution_target: None,
            })
        })
        .unwrap();
    SettingsStore::for_profile_path(&source)
        .save(&AppSettings {
            locale: Locale::En,
            editor_font_size: 18,
            editor_tab_width: 2,
            ..AppSettings::default()
        })
        .unwrap();
    let settings_path = dir.join("app-settings.json");
    let mut settings: Value = serde_json::from_slice(&fs::read(&settings_path).unwrap()).unwrap();
    settings["unrelated_secret"] = Value::String("UNKNOWN_SETTINGS_SENTINEL".into());
    fs::write(settings_path, serde_json::to_vec(&settings).unwrap()).unwrap();
    fs::write(dir.join("editor-preferences.json"), br#"{"enabled":true}"#).unwrap();
    fs::write(
        dir.join("workspace.json"),
        b"RECOVERY_SQL_SENTINEL deliberately not parsed",
    )
    .unwrap();
    source
}

#[test]
fn portable_export_inspect_import_preserves_settings_and_omits_local_content() {
    let d = TempDir::new().unwrap();
    let source_dir = d.path().join("source");
    fs::create_dir(&source_dir).unwrap();
    let source = fixture(&source_dir);
    let source_before = fs::read(&source).unwrap();
    let bundle = d.path().join("portable.json");
    let exported = success(run(
        &source,
        &[
            "config",
            "export",
            "--output",
            bundle.to_str().unwrap(),
            "--base-dir",
            source_dir.to_str().unwrap(),
        ],
    ));
    assert_eq!(exported["ok"], true);
    assert_eq!(exported["inspection"]["profile_count"], 1);
    assert_eq!(exported["inspection"]["recovery_enabled"], true);
    let text = fs::read_to_string(&bundle).unwrap();
    for omitted in [
        "HISTORY_SQL_SENTINEL",
        "RECOVERY_SQL_SENTINEL",
        "UNKNOWN_SETTINGS_SENTINEL",
    ] {
        assert!(!text.contains(omitted), "portable file included {omitted}");
    }
    let unrelated = d.path().join("missing-source").join("profiles.json");
    let inspected = success(run(
        &unrelated,
        &["config", "inspect", "--file", bundle.to_str().unwrap()],
    ));
    assert_eq!(inspected["inspection"]["selected_profile_id"], "fixture");
    assert!(
        !unrelated.parent().unwrap().exists(),
        "inspect must not open/create the active source"
    );
    let remap = d.path().join("remapped");
    fs::create_dir(&remap).unwrap();
    let destination = d.path().join("restored");
    let imported = success(run(
        &source,
        &[
            "config",
            "import",
            "--file",
            bundle.to_str().unwrap(),
            "--path-root",
            remap.to_str().unwrap(),
            "--destination",
            destination.to_str().unwrap(),
        ],
    ));
    assert_eq!(imported["report"]["profile_count"], 1);
    let restored_profile = Path::new(imported["report"]["profile_path"].as_str().unwrap());
    let restored = ProfileStore::open(restored_profile)
        .unwrap()
        .load()
        .unwrap();
    assert_eq!(restored.selected_profile_id.as_deref(), Some("fixture"));
    assert!(restored.history.is_empty());
    assert_eq!(
        restored.profiles[0].sqlite_path.as_deref(),
        Some(remap.join("fixture.db").to_str().unwrap())
    );
    let settings = SettingsStore::for_profile_path(restored_profile)
        .load()
        .unwrap();
    assert_eq!(settings.locale, Locale::En);
    assert_eq!(settings.editor_font_size, 18);
    assert_eq!(settings.editor_tab_width, 2);
    assert!(dbide_portable::read_recovery_enabled(restored_profile).unwrap());
    assert!(!destination.join("workspace.json").exists());
    assert!(
        !remap.join("fixture.db").exists(),
        "import must not create a database"
    );
    assert_eq!(
        fs::read(&source).unwrap(),
        source_before,
        "import must not change the active source"
    );
    assert!(imported["report"]["warnings"].as_array().is_some());
}

#[test]
fn portable_commands_require_explicit_paths_and_preserve_existing_destinations() {
    let d = TempDir::new().unwrap();
    let source = fixture(d.path());
    let bundle = d.path().join("portable.json");
    for args in [
        vec!["config", "export", "--output", bundle.to_str().unwrap()],
        vec![
            "config",
            "import",
            "--file",
            "missing.json",
            "--destination",
            "new",
        ],
    ] {
        assert_eq!(run(&source, &args).status.code(), Some(2));
    }
    success(run(
        &source,
        &[
            "config",
            "export",
            "--output",
            bundle.to_str().unwrap(),
            "--base-dir",
            d.path().to_str().unwrap(),
        ],
    ));
    let original = fs::read(&bundle).unwrap();
    assert_eq!(
        run(
            &source,
            &[
                "config",
                "export",
                "--output",
                bundle.to_str().unwrap(),
                "--base-dir",
                d.path().to_str().unwrap()
            ]
        )
        .status
        .code(),
        Some(2)
    );
    assert_eq!(fs::read(&bundle).unwrap(), original);
    let destination = d.path().join("existing");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("sentinel"), b"preserve").unwrap();
    assert_eq!(
        run(
            &source,
            &[
                "config",
                "import",
                "--file",
                bundle.to_str().unwrap(),
                "--path-root",
                d.path().to_str().unwrap(),
                "--destination",
                destination.to_str().unwrap()
            ]
        )
        .status
        .code(),
        Some(2)
    );
    assert_eq!(fs::read(destination.join("sentinel")).unwrap(), b"preserve");
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);
    let malformed = d.path().join("malformed.json");
    fs::write(&malformed, b"MALFORMED_PRIVATE_SENTINEL").unwrap();
    let output = run(
        &source,
        &["config", "inspect", "--file", malformed.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("MALFORMED_PRIVATE_SENTINEL"));
    assert!(output.stdout.is_empty());
}

#[test]
fn import_maps_an_external_path_only_when_explicitly_named() {
    let d = TempDir::new().unwrap();
    let source_dir = d.path().join("source");
    fs::create_dir(&source_dir).unwrap();
    let source = fixture(&source_dir);
    let base = d.path().join("export-base");
    fs::create_dir(&base).unwrap();
    let bundle = d.path().join("portable.json");
    let exported = success(run(
        &source,
        &[
            "config",
            "export",
            "--output",
            bundle.to_str().unwrap(),
            "--base-dir",
            base.to_str().unwrap(),
        ],
    ));
    assert_eq!(exported["inspection"]["external_path_count"], 1);
    let mapped = d.path().join("mapped.db");
    let mapping = format!(
        "{}={}",
        source_dir.join("fixture.db").display(),
        mapped.display()
    );
    let destination = d.path().join("restored");
    let imported = success(run(
        &source,
        &[
            "config",
            "import",
            "--file",
            bundle.to_str().unwrap(),
            "--path-root",
            d.path().to_str().unwrap(),
            "--destination",
            destination.to_str().unwrap(),
            "--map",
            &mapping,
        ],
    ));
    let restored = ProfileStore::open(imported["report"]["profile_path"].as_str().unwrap())
        .unwrap()
        .load()
        .unwrap();
    assert_eq!(restored.profiles[0].sqlite_path.as_deref(), mapped.to_str());
    assert!(!mapped.exists());
}
