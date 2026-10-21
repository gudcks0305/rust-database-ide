use super::*;

fn preview() -> ImportPreview {
    import_bytes(&serde_json::to_vec(&builtin_manifest()).unwrap()).unwrap()
}
fn registry() -> (tempfile::TempDir, Registry) {
    let temp = tempfile::tempdir().unwrap();
    let registry = Registry::open(temp.path()).unwrap();
    (temp, registry)
}

#[test]
fn strict_schema_and_limits() {
    let mut value = serde_json::to_value(builtin_manifest()).unwrap();
    value["run"] = "shell command".into();
    assert!(import_bytes(&serde_json::to_vec(&value).unwrap()).is_err());
    value.as_object_mut().unwrap().remove("run");
    value["schema_version"] = 2.into();
    assert!(import_bytes(&serde_json::to_vec(&value).unwrap()).is_err());
    assert!(import_bytes(&vec![b' '; MAX_MANIFEST_BYTES + 1]).is_err());
    for id in [
        "../escape",
        "/tmp/escape",
        "C:\\escape",
        "bad/name",
        "-bad",
        "A",
        "a--b",
    ] {
        let mut manifest = builtin_manifest();
        manifest.id = id.into();
        assert!(manifest.validate().is_err(), "{id}");
    }
    let mut manifest = builtin_manifest();
    manifest.snippets[0].sql = "a".repeat(MAX_SQL_BYTES + 1);
    assert!(manifest.validate().is_err());
    manifest = builtin_manifest();
    manifest.snippets = vec![manifest.snippets[0].clone(); MAX_SNIPPETS + 1];
    assert!(manifest.validate().is_err());
    manifest = builtin_manifest();
    manifest.snippets[1].id = manifest.snippets[0].id.clone();
    assert!(manifest.validate().is_err());
    manifest = builtin_manifest();
    manifest.version = "01.2.3".into();
    assert!(manifest.validate().is_err());
    manifest.version = "1.0.0".into();
    manifest.name = " ".into();
    assert!(manifest.validate().is_err());
}

#[test]
fn file_import_is_preview_only_and_bounded() {
    let (temp, registry) = registry();
    let source = temp.path().join("manifest.txt");
    fs::write(&source, serde_json::to_vec(&builtin_manifest()).unwrap()).unwrap();
    let imported = import_file(&source).unwrap();
    assert_eq!(imported.manifest().id, "builtin-sql");
    assert!(registry.list().unwrap().is_empty());
    fs::write(&source, vec![b' '; MAX_MANIFEST_BYTES + 1]).unwrap();
    assert!(import_file(&source).is_err());
}

#[test]
fn explicit_install_enable_and_dialect_scoped_editor_text() {
    let (_temp, registry) = registry();
    let preview = preview();
    registry.install(&preview, false).unwrap();
    assert!(
        registry
            .enabled_snippets(Dialect::Postgres)
            .unwrap()
            .is_empty()
    );
    assert!(
        registry
            .snippet_sql("builtin-sql", "pg-select", Dialect::Postgres)
            .is_err()
    );
    assert!(matches!(
        registry.install(&preview, false),
        Err(Error::AlreadyInstalled)
    ));
    registry.set_enabled("builtin-sql", true).unwrap();
    assert_eq!(
        registry.enabled_snippets(Dialect::Postgres).unwrap().len(),
        3
    );
    assert_eq!(registry.enabled_snippets(Dialect::Sqlite).unwrap().len(), 3);
    assert_eq!(registry.enabled_snippets(Dialect::Mysql).unwrap().len(), 3);
    assert_eq!(
        registry
            .snippet_sql("builtin-sql", "pg-select", Dialect::Postgres)
            .unwrap(),
        "SELECT 1 AS example;"
    );
    assert!(
        registry
            .snippet_sql("builtin-sql", "pg-select", Dialect::Mysql)
            .is_err()
    );
    registry.set_enabled("builtin-sql", false).unwrap();
    let reopened = Registry::open(registry.root()).unwrap();
    assert!(
        reopened
            .enabled_snippets(Dialect::Postgres)
            .unwrap()
            .is_empty()
    );
    for id in ["../outside", "/tmp/outside"] {
        assert!(registry.uninstall(id).is_err());
        assert!(registry.set_enabled(id, true).is_err());
    }
}

#[test]
fn replacement_backups_and_failed_replace_preserve_original() {
    let (_temp, registry) = registry();
    let original = registry.install(&preview(), false).unwrap().path;
    let bytes = fs::read(&original).unwrap();
    let mut invalid_preview = preview();
    invalid_preview.manifest.snippets[0].sql = "x".repeat(MAX_SQL_BYTES + 1);
    assert!(registry.install(&invalid_preview, true).is_err());
    assert_eq!(fs::read(&original).unwrap(), bytes);
    // A failed archive preparation must not damage the active entry.
    fs::write(registry.root().join("archive"), b"block archive directory").unwrap();
    assert!(registry.install(&preview(), true).is_err());
    assert_eq!(fs::read(&original).unwrap(), bytes);
    fs::remove_file(registry.root().join("archive")).unwrap();
    let mut changed = preview();
    changed.manifest.version = "2.0.0".into();
    changed.manifest.snippets[0].sql = "SELECT 2;".into();
    let result = registry.install(&changed, true).unwrap();
    assert_eq!(fs::read(result.backup.as_ref().unwrap()).unwrap(), bytes);
    assert_eq!(registry.list().unwrap()[0].manifest.version, "2.0.0");
    registry
        .restore_archive(result.backup.unwrap(), true)
        .unwrap();
    assert_eq!(registry.list().unwrap()[0].manifest.version, "1.0.0");
}

#[test]
fn uninstall_is_reversible_and_restore_is_disabled() {
    let (_temp, registry) = registry();
    registry.install(&preview(), false).unwrap();
    registry.set_enabled("builtin-sql", true).unwrap();
    let archive = registry.uninstall("builtin-sql").unwrap();
    assert!(registry.list().unwrap().is_empty());
    assert!(archive.exists());
    registry.restore_archive(&archive, false).unwrap();
    assert!(archive.exists());
    assert!(!registry.list().unwrap()[0].enabled);
    assert!(registry.restore_archive(&archive, false).is_err());
    let outside = tempfile::NamedTempFile::new().unwrap();
    assert!(registry.restore_archive(outside.path(), true).is_err());
}

#[test]
fn palettes_are_validated_and_only_enabled_themes_are_exposed() {
    let (_temp, registry) = registry();
    let mut manifest = builtin_manifest();
    manifest.snippets.clear();
    manifest.theme = Some(ThemePalette {
        background: "#001122".into(),
        foreground: "#aAbBcC".into(),
        accent: "#123456".into(),
        selection: "#445566".into(),
    });
    assert_eq!(manifest.capabilities(), vec!["theme-palette"]);
    let preview = import_bytes(&serde_json::to_vec(&manifest).unwrap()).unwrap();
    registry.install(&preview, false).unwrap();
    assert!(registry.enabled_themes().unwrap().is_empty());
    registry.set_enabled("builtin-sql", true).unwrap();
    assert_eq!(registry.enabled_themes().unwrap().len(), 1);
    manifest.theme.as_mut().unwrap().accent = "url(script)".into();
    assert!(manifest.validate().is_err());
    let mut value = serde_json::to_value(preview.manifest()).unwrap();
    value["theme"]["css"] = "bad".into();
    assert!(import_bytes(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[cfg(unix)]
#[test]
fn private_permissions_and_symlink_rejection() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (temp, registry) = registry();
    let result = registry.install(&preview(), false).unwrap();
    assert_eq!(
        fs::metadata(&result.path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let backup = registry
        .set_enabled("builtin-sql", true)
        .unwrap()
        .backup
        .unwrap();
    assert_eq!(
        fs::metadata(backup).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let outside = temp.path().join("outside.txt");
    fs::write(&outside, b"untouched").unwrap();
    symlink(&outside, registry.root().join("evil.json")).unwrap();
    assert!(registry.set_enabled("evil", true).is_err());
    assert_eq!(fs::read(outside).unwrap(), b"untouched");
}

#[test]
fn active_count_and_combined_manifest_bounds() {
    let (_temp, registry) = registry();
    for index in 0..MAX_EXTENSIONS {
        let mut manifest = builtin_manifest();
        manifest.id = format!("extension-{index}");
        let record = InstalledExtension {
            enabled: false,
            manifest,
        };
        fs::write(
            registry.root().join(format!("extension-{index}.json")),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
    }
    assert_eq!(registry.list().unwrap().len(), MAX_EXTENSIONS);
    assert!(registry.install(&preview(), false).is_err());
    assert!(!registry.root().join("builtin-sql.json").exists());
    let mut manifest = builtin_manifest();
    for snippet in &mut manifest.snippets {
        snippet.sql = "x".repeat(MAX_SQL_BYTES);
    }
    assert!(manifest.validate().is_err());
    let mut value = serde_json::to_value(builtin_manifest()).unwrap();
    value["snippets"][0]["execute"] = true.into();
    assert!(import_bytes(&serde_json::to_vec(&value).unwrap()).is_err());
}
