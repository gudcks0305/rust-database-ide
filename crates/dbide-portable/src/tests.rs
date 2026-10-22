use super::*;

fn sample(base: &Path) -> (WorkspaceState, Bundle) {
    let mut sqlite = ConnectionProfile::fixture();
    sqlite.id = "portable-sqlite".into();
    sqlite.engine = DatabaseEngine::Sqlite;
    sqlite.sqlite_path = Some(base.join("data/demo.db").to_str().unwrap().into());
    let mut network = ConnectionProfile::fixture();
    network.id = "portable-postgres".into();
    network.tls_mode = TlsMode::VerifyFull;
    network.root_cert_path = Some("certs/root.pem".into());
    let state = WorkspaceState {
        profiles: vec![sqlite, network],
        selected_profile_id: Some("portable-sqlite".into()),
        persist_history: true,
        history: vec![dbide_profiles::HistoryEntry {
            profile_id: "portable-sqlite".into(),
            sql: "SELECT 'history-secret-that-must-not-leak'".into(),
            executed_at_unix_ms: 1,
            outcome: "Completed".into(),
            row_count: Some(1),
            execution_target: None,
        }],
        ..Default::default()
    };
    let bundle = Bundle::capture(&state, AppSettings::default(), true, base).unwrap();
    (state, bundle)
}
fn write_json(dir: &Path, value: &serde_json::Value) -> PathBuf {
    let path = dir.join("input.json");
    fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
    path
}

#[test]
fn roundtrip_restores_configuration_only_and_completion_last() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let (_, bundle) = sample(&source);
    let exported = dir.path().join("bundle.json");
    bundle.write_new(&exported).unwrap();
    let bytes = fs::read_to_string(&exported).unwrap();
    for excluded in [
        "history-secret-that-must-not-leak",
        "\"history\"",
        "\"password\"",
        "\"persist_history\"",
        "\"sql\"",
        "\"project_root\"",
    ] {
        assert!(!bytes.contains(excluded), "unexpected {excluded}");
    }
    let bundle = Bundle::read(&exported).unwrap();
    let inspection = bundle.inspect();
    assert_eq!(inspection.profile_count, 2);
    assert_eq!(inspection.relative_path_count, 2);
    assert_eq!(inspection.external_path_count, 0);
    let target = dir.path().join("restored");
    let remap = dir.path().join("moved-files");
    let report = bundle.restore_new_directory(&target, Some(&remap)).unwrap();
    assert_eq!(report.profile_count, 2);
    assert!(report.warnings.iter().any(|w| w.contains("Keychain")));
    assert!(report.warnings.iter().any(|w| w.contains("missing")));
    let state = ProfileStore::open(&report.profile_path)
        .unwrap()
        .load()
        .unwrap();
    assert_eq!(
        state.selected_profile_id.as_deref(),
        Some("portable-sqlite")
    );
    assert_eq!(
        state.profiles[0].sqlite_path.as_deref(),
        remap.join("data/demo.db").to_str()
    );
    assert_eq!(
        state.profiles[1].root_cert_path.as_deref(),
        remap.join("certs/root.pem").to_str()
    );
    assert!(state.history.is_empty());
    assert!(!state.persist_history);
    assert_eq!(
        SettingsStore::for_profile_path(&report.profile_path)
            .load()
            .unwrap(),
        bundle.app_settings
    );
    assert!(read_recovery_enabled(&report.profile_path).unwrap());
    assert!(target.join("portable-complete.json").is_file());
    assert!(!target.join("workspace.json").exists());
    assert!(!target.join("data").exists());
    assert!(!remap.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for name in [
            "profiles.json",
            "app-settings.json",
            "editor-preferences.json",
            "portable-complete.json",
        ] {
            assert_eq!(
                fs::metadata(target.join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}

#[test]
fn empty_network_and_memory_configs_require_no_remap() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = WorkspaceState::default();
    let empty = Bundle::capture(&state, AppSettings::default(), false, dir.path()).unwrap();
    assert!(empty.plan(None).unwrap().state.profiles.is_empty());
    let mut sqlite = ConnectionProfile::fixture();
    sqlite.engine = DatabaseEngine::Sqlite;
    sqlite.sqlite_path = Some(":memory:".into());
    state.profiles.push(sqlite);
    let bundle = Bundle::capture(&state, AppSettings::default(), false, dir.path()).unwrap();
    assert_eq!(
        bundle.plan(None).unwrap().state.profiles[0]
            .sqlite_path
            .as_deref(),
        Some(":memory:")
    );
}

#[test]
fn relative_paths_need_explicit_absolute_base_and_restore_root() {
    let dir = tempfile::tempdir().unwrap();
    let (state, bundle) = sample(dir.path());
    assert!(Bundle::capture(&state, AppSettings::default(), false, Path::new("relative")).is_err());
    assert!(matches!(
        bundle.plan(None),
        Err(Error::RelativeRootRequired)
    ));
    assert!(bundle.plan(Some(Path::new("relative"))).is_err());
    assert!(bundle.plan(Some(&dir.path().join("../escape"))).is_err());
    assert!(!dir.path().join("restored").exists());
}

#[test]
fn traversal_rooted_and_ambiguous_relative_paths_rejected() {
    let dir = tempfile::tempdir().unwrap();
    for bad in [
        "../secret.db",
        "foo/../secret.db",
        "foo/./secret.db",
        "C:secret.db",
        "\\rooted.db",
        "foo//bar.db",
        "foo\\..\\bar.db",
        "foo\nbar.db",
    ] {
        let mut p = ConnectionProfile::fixture();
        p.engine = DatabaseEngine::Sqlite;
        p.sqlite_path = Some(bad.into());
        let state = WorkspaceState {
            profiles: vec![p],
            ..Default::default()
        };
        assert!(
            Bundle::capture(&state, AppSettings::default(), false, dir.path()).is_err(),
            "accepted {bad:?}"
        );
    }
    let (_, mut bundle) = sample(dir.path());
    bundle.profiles[0].sqlite_path = Some(PortablePath::Relative {
        components: vec!["..".into(), "escape".into()],
    });
    assert!(bundle.plan(Some(dir.path())).is_err());
    bundle.profiles[0].sqlite_path = Some(PortablePath::Relative {
        components: vec!["/escape".into()],
    });
    assert!(bundle.plan(Some(dir.path())).is_err());
}

#[cfg(unix)]
#[test]
fn unix_relative_backslash_filenames_are_rejected_instead_of_reinterpreted() {
    let dir = tempfile::tempdir().unwrap();
    for certificate in [false, true] {
        let mut profile = ConnectionProfile::fixture();
        if certificate {
            profile.root_cert_path = Some("foo\\bar.pem".into());
        } else {
            profile.engine = DatabaseEngine::Sqlite;
            profile.sqlite_path = Some("foo\\bar.db".into());
        }
        let state = WorkspaceState {
            profiles: vec![profile],
            ..Default::default()
        };
        assert!(matches!(
            Bundle::capture(&state, AppSettings::default(), false, dir.path()),
            Err(Error::Invalid(_))
        ));
    }
    // Ordinary native components still resolve to the same selected file.
    let path = PortablePath::capture("foo/bar.db", dir.path(), true).unwrap();
    assert_eq!(
        path.resolve(Some(dir.path()), &[]).unwrap(),
        dir.path().join("foo/bar.db").to_str().unwrap()
    );
}

#[test]
fn cross_platform_external_paths_require_exact_explicit_mapping() {
    let dir = tempfile::tempdir().unwrap();
    let foreign = if cfg!(windows) {
        "/old/data/demo.db"
    } else {
        "C:\\old\\data\\demo.db"
    };
    let mut p = ConnectionProfile::fixture();
    p.engine = DatabaseEngine::Sqlite;
    p.sqlite_path = Some(foreign.into());
    let state = WorkspaceState {
        profiles: vec![p],
        ..Default::default()
    };
    let bundle = Bundle::capture(&state, AppSettings::default(), false, dir.path()).unwrap();
    assert_eq!(bundle.inspect().external_path_count, 1);
    assert!(matches!(
        bundle.plan(Some(dir.path())),
        Err(Error::ForeignPathRequiresRemap)
    ));
    let mapped = dir.path().join("explicit.db");
    let plan = bundle
        .plan_with_mappings(
            None,
            &[ExternalPathMapping {
                original: foreign.into(),
                destination: mapped.clone(),
            }],
        )
        .unwrap();
    assert_eq!(
        plan.state.profiles[0].sqlite_path.as_deref(),
        mapped.to_str()
    );
    assert!(
        bundle
            .plan_with_mappings(
                None,
                &[ExternalPathMapping {
                    original: "unknown".into(),
                    destination: mapped
                }]
            )
            .is_err()
    );
}

#[test]
fn current_platform_external_paths_remain_absolute_and_warn_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let mut p = ConnectionProfile::fixture();
    p.root_cert_path = Some(other.path().join("missing.pem").to_str().unwrap().into());
    let state = WorkspaceState {
        profiles: vec![p],
        ..Default::default()
    };
    let bundle = Bundle::capture(&state, AppSettings::default(), false, dir.path()).unwrap();
    let plan = bundle.plan(None).unwrap();
    assert_eq!(
        plan.state.profiles[0].root_cert_path,
        state.profiles[0].root_cert_path
    );
    assert!(plan.warnings.iter().any(|w| w.contains("missing")));
}

#[test]
fn existing_export_and_destination_never_change() {
    let dir = tempfile::tempdir().unwrap();
    let (_, bundle) = sample(dir.path());
    let export = dir.path().join("existing.json");
    fs::write(&export, b"original malformed bytes").unwrap();
    assert!(matches!(
        bundle.write_new(&export),
        Err(Error::DestinationExists)
    ));
    assert_eq!(fs::read(&export).unwrap(), b"original malformed bytes");
    let target = dir.path().join("existing");
    fs::create_dir(&target).unwrap();
    assert!(matches!(
        bundle.restore_new_directory(&target, Some(dir.path())),
        Err(Error::DestinationExists)
    ));
    assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
    fs::write(target.join("profiles.json"), b"corrupt existing profiles").unwrap();
    assert!(matches!(
        bundle.restore_new_directory(&target, Some(dir.path())),
        Err(Error::DestinationExists)
    ));
    assert_eq!(
        fs::read(target.join("profiles.json")).unwrap(),
        b"corrupt existing profiles"
    );
}

#[test]
fn invalid_import_never_creates_or_modifies_destination() {
    let dir = tempfile::tempdir().unwrap();
    let (_, mut bundle) = sample(dir.path());
    let target = dir.path().join("new");
    bundle.profiles[0].id.clear();
    assert!(
        bundle
            .restore_new_directory(&target, Some(dir.path()))
            .is_err()
    );
    assert!(!target.exists());
    bundle = sample(dir.path()).1;
    bundle.app_settings.editor_font_size = 99;
    assert!(
        bundle
            .restore_new_directory(&target, Some(dir.path()))
            .is_err()
    );
    assert!(!target.exists());
}

#[test]
fn unknown_duplicate_and_future_fields_are_rejected_without_changes() {
    let dir = tempfile::tempdir().unwrap();
    let (_, bundle) = sample(dir.path());
    let original = serde_json::to_value(&bundle).unwrap();
    for key in ["history", "password", "project_references", "workspace"] {
        let mut value = original.clone();
        value[key] = serde_json::json!([]);
        let path = write_json(dir.path(), &value);
        assert!(matches!(Bundle::read(&path), Err(Error::Malformed)));
        assert_eq!(
            fs::read(&path).unwrap(),
            serde_json::to_vec(&value).unwrap()
        );
    }
    for location in ["settings", "profile", "path"] {
        let mut value = original.clone();
        match location {
            "settings" => value["app_settings"]["password"] = serde_json::json!("excluded"),
            "profile" => value["profiles"][0]["password"] = serde_json::json!("excluded"),
            _ => value["profiles"][0]["sqlite_path"]["password"] = serde_json::json!("excluded"),
        }
        assert!(matches!(
            Bundle::read(&write_json(dir.path(), &value)),
            Err(Error::Malformed)
        ));
    }
    for key in [
        "schema_version",
        "profiles_schema_version",
        "settings_schema_version",
    ] {
        let mut value = original.clone();
        value[key] = serde_json::json!(999);
        value["future_field"] = serde_json::json!(true);
        assert!(matches!(
            Bundle::read(&write_json(dir.path(), &value)),
            Err(Error::UnsupportedVersion)
        ));
    }
    let path = dir.path().join("duplicate.json");
    let text = serde_json::to_string(&bundle)
        .unwrap()
        .replacen('{', "{\"schema_version\":1,", 1);
    fs::write(&path, text).unwrap();
    assert!(matches!(Bundle::read(&path), Err(Error::Malformed)));
}

#[test]
fn size_counts_selection_and_typed_settings_are_validated() {
    let dir = tempfile::tempdir().unwrap();
    let (_, mut bundle) = sample(dir.path());
    let path = dir.path().join("large.json");
    fs::write(&path, vec![b' '; MAX_BUNDLE_BYTES + 1]).unwrap();
    assert!(matches!(Bundle::read(&path), Err(Error::TooLarge)));
    let (mut future_state, _) = sample(dir.path());
    future_state.schema_version += 1;
    assert!(matches!(
        Bundle::capture(&future_state, AppSettings::default(), false, dir.path()),
        Err(Error::UnsupportedVersion)
    ));
    bundle.selected_profile_id = Some("not-present".into());
    assert!(bundle.validate().is_err());
    bundle.selected_profile_id = None;
    bundle.profiles[1].id = bundle.profiles[0].id.clone();
    assert!(bundle.validate().is_err());
    bundle = sample(dir.path()).1;
    let profile = bundle.profiles[0].clone();
    bundle.profiles = (0..=dbide_profiles::MAX_PROFILES)
        .map(|i| {
            let mut p = profile.clone();
            p.id = format!("id-{i}");
            p
        })
        .collect();
    assert!(bundle.validate().is_err());
    let mut value = serde_json::to_value(sample(dir.path()).1).unwrap();
    value["app_settings"]["table_page_size"] = serde_json::json!(123);
    assert!(Bundle::read(&write_json(dir.path(), &value)).is_err());
    value["app_settings"]
        .as_object_mut()
        .unwrap()
        .remove("table_page_size");
    assert!(matches!(
        Bundle::read(&write_json(dir.path(), &value)),
        Err(Error::Malformed)
    ));
}

#[test]
fn recovery_parser_is_bounded_strict_and_missing_is_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let profile = dir.path().join("profiles.json");
    assert!(!read_recovery_enabled(&profile).unwrap());
    let path = dir.path().join("editor-preferences.json");
    fs::write(&path, br#"{"enabled":true}"#).unwrap();
    assert!(read_recovery_enabled(&profile).unwrap());
    fs::write(&path, br#"{"enabled":true,"secret":"no"}"#).unwrap();
    assert!(matches!(
        read_recovery_enabled(&profile),
        Err(Error::Malformed)
    ));
    fs::write(&path, vec![b' '; 4097]).unwrap();
    assert!(matches!(
        read_recovery_enabled(&profile),
        Err(Error::TooLarge)
    ));
}

#[cfg(unix)]
#[test]
fn symlinks_and_special_files_are_rejected_without_following() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let (_, bundle) = sample(dir.path());
    let original = dir.path().join("original.json");
    bundle.write_new(&original).unwrap();
    let link = dir.path().join("link.json");
    symlink(&original, &link).unwrap();
    assert!(matches!(Bundle::read(&link), Err(Error::NotRegularFile)));
    assert!(matches!(
        bundle.write_new(&link),
        Err(Error::DestinationExists)
    ));
    let target = dir.path().join("target");
    symlink(dir.path().join("not-created"), &target).unwrap();
    assert!(matches!(
        bundle.restore_new_directory(&target, Some(dir.path())),
        Err(Error::DestinationExists)
    ));
    assert!(
        fs::symlink_metadata(&target)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let pref = dir.path().join("editor-preferences.json");
    symlink(&original, &pref).unwrap();
    assert!(matches!(
        read_recovery_enabled(&dir.path().join("profiles.json")),
        Err(Error::NotRegularFile)
    ));
    assert!(matches!(
        Bundle::read(dir.path()),
        Err(Error::NotRegularFile)
    ));
    let fifo = dir.path().join("fifo");
    let name = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(matches!(Bundle::read(&fifo), Err(Error::NotRegularFile)));
}

#[test]
fn cleanup_removes_only_owned_known_files_and_never_preexisting_directories() {
    let dir = tempfile::tempdir().unwrap();
    let owned = dir.path().join("owned");
    fs::create_dir(&owned).unwrap();
    let metadata = fs::symlink_metadata(&owned).unwrap();
    fs::write(owned.join("profiles.json"), "partial").unwrap();
    cleanup_owned_directory(&owned, &metadata);
    assert!(!owned.exists());
    let other = dir.path().join("other");
    fs::create_dir(&other).unwrap();
    fs::write(other.join("profiles.json"), "preserve").unwrap();
    #[cfg(unix)]
    {
        cleanup_owned_directory(&other, &metadata);
        assert_eq!(
            fs::read_to_string(other.join("profiles.json")).unwrap(),
            "preserve"
        );
    }
}
