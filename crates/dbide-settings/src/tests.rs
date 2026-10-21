use super::*;
#[test]
fn default_no_side_effects_then_private_save_reload() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("new").join("app-settings.json");
    let store = SettingsStore::new(&path);
    assert_eq!(store.load().unwrap(), AppSettings::default());
    assert!(!path.parent().unwrap().exists());
    store.set_locale(Locale::En).unwrap();
    assert_eq!(store.load().unwrap().locale, Locale::En);
    assert_eq!(SettingsStore::new(&path).load().unwrap().locale, Locale::En);
    store
        .save(&AppSettings {
            locale: Locale::Ko,
            ..AppSettings::default()
        })
        .unwrap();
    assert_eq!(store.load().unwrap().locale, Locale::Ko);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 2); // settings + lock, no temporary
}
#[test]
fn pure_profile_path_convention_and_unknown_fields_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let store = SettingsStore::for_profile_path(temp.path().join("profiles.json"));
    assert_eq!(store.path(), temp.path().join("app-settings.json"));
    assert_eq!(
        SettingsStore::for_profile_path("profiles.json").path(),
        Path::new("./app-settings.json")
    );
    let original = serde_json::json!({"schema_version":1,"locale":"en","future":{"nested":["한글",true,42]},"enabled":true});
    fs::write(store.path(), serde_json::to_vec(&original).unwrap()).unwrap();
    store.set_locale(Locale::Ko).unwrap();
    let updated: Value = serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
    assert_eq!(updated["future"], original["future"]);
    assert_eq!(updated["enabled"], true);
    assert_eq!(updated["locale"], "ko");
}
#[test]
fn corrupt_future_ambiguous_invalid_locale_and_oversize_originals_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let store = SettingsStore::new(temp.path().join("app-settings.json"));
    for bytes in [
        b"not json".to_vec(),
        b"{}".to_vec(),
        b"{\"schema_version\":2,\"locale\":\"en\"}".to_vec(),
        b"{\"schema_version\":1,\"locale\":\"fr\"}".to_vec(),
        b"{\"schema_version\":1,\"schema_version\":2,\"locale\":\"ko\"}".to_vec(),
        b"{\"schema_version\":1,\"locale\":\"ko\",\"locale\":\"en\"}".to_vec(),
        vec![b' '; MAX_SETTINGS_BYTES + 1],
        vec![0xff],
    ] {
        fs::write(store.path(), &bytes).unwrap();
        assert!(store.load().is_err());
        assert!(store.set_locale(Locale::Ko).is_err());
        assert_eq!(fs::read(store.path()).unwrap(), bytes);
    }
}
#[test]
fn rereads_latest_file_before_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let store = SettingsStore::new(temp.path().join("app-settings.json"));
    store.set_locale(Locale::En).unwrap();
    let prior = store.load().unwrap();
    let changed = b"{\"schema_version\":99,\"locale\":\"ko\"}";
    fs::write(store.path(), changed).unwrap();
    assert!(matches!(
        store.save(&prior),
        Err(StoreError::UnsupportedVersion(99))
    ));
    assert_eq!(fs::read(store.path()).unwrap(), changed);
}
#[test]
fn utf8_bound_uses_bytes_and_valid_unknown_unicode_survives() {
    let temp = tempfile::tempdir().unwrap();
    let store = SettingsStore::new(temp.path().join("app-settings.json"));
    let bytes=serde_json::to_vec(&serde_json::json!({"schema_version":1,"locale":"ko","note":"한".repeat(MAX_SETTINGS_BYTES/3)})).unwrap();
    assert!(bytes.len() > MAX_SETTINGS_BYTES);
    fs::write(store.path(), &bytes).unwrap();
    assert!(matches!(
        store.set_locale(Locale::En),
        Err(StoreError::TooLarge)
    ));
    assert_eq!(fs::read(store.path()).unwrap(), bytes);
}
#[cfg(unix)]
#[test]
fn symlinks_fifos_and_lock_symlinks_are_rejected_without_reading() {
    use std::{
        ffi::CString,
        os::unix::{ffi::OsStrExt, fs::symlink},
    };
    let temp = tempfile::tempdir().unwrap();
    let outside = temp.path().join("outside.json");
    let original = b"{\"schema_version\":1,\"locale\":\"ko\"}";
    fs::write(&outside, original).unwrap();
    let path = temp.path().join("app-settings.json");
    let store = SettingsStore::new(&path);
    symlink(&outside, &path).unwrap();
    assert!(store.load().is_err());
    assert!(store.set_locale(Locale::En).is_err());
    assert_eq!(fs::read(&outside).unwrap(), original);
    fs::remove_file(&path).unwrap();
    let cpath = CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: cpath is a valid NUL-terminated path; creates only a temporary fixture FIFO.
    assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
    assert!(store.load().is_err());
    assert!(store.set_locale(Locale::En).is_err());
    fs::remove_file(&path).unwrap();
    let lock = temp.path().join("app-settings.json.lock");
    fs::remove_file(&lock).unwrap();
    symlink(&outside, &lock).unwrap();
    assert!(store.set_locale(Locale::En).is_err());
    assert_eq!(fs::read(outside).unwrap(), original);
    assert!(!path.exists());
}
#[test]
fn concurrent_writers_produce_valid_complete_settings_and_keep_unknowns() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("app-settings.json");
    fs::write(
        &path,
        b"{\"schema_version\":1,\"locale\":\"ko\",\"keep\":{\"id\":7}}",
    )
    .unwrap();
    std::thread::scope(|scope| {
        for index in 0..4 {
            let path = path.clone();
            scope.spawn(move || {
                for _ in 0..8 {
                    SettingsStore::new(&path)
                        .set_locale(if index % 2 == 0 {
                            Locale::En
                        } else {
                            Locale::Ko
                        })
                        .unwrap();
                }
            });
        }
    });
    assert!(SettingsStore::new(&path).load().is_ok());
    let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(value["keep"]["id"], 7);
}

#[test]
fn stale_settings_snapshot_does_not_drop_newer_unknown_fields() {
    let temp = tempfile::tempdir().unwrap();
    let store = SettingsStore::new(temp.path().join("app-settings.json"));
    store.set_locale(Locale::Ko).unwrap();
    let mut stale = store.load().unwrap();
    let latest = serde_json::json!({"schema_version":1,"locale":"ko","added_later":{"name":"한글","enabled":true}});
    fs::write(store.path(), serde_json::to_vec(&latest).unwrap()).unwrap();
    stale.locale = Locale::En;
    store.save(&stale).unwrap();
    let result: Value = serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
    assert_eq!(result["locale"], "en");
    assert_eq!(result["added_later"], latest["added_later"]);
}

#[test]
fn language_only_document_migrates_defaults_and_new_settings_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let store = SettingsStore::new(temp.path().join("app-settings.json"));
    fs::write(
        store.path(),
        br#"{"schema_version":1,"locale":"en","future":{"keep":7}}"#,
    )
    .unwrap();
    assert_eq!(
        store.load().unwrap(),
        AppSettings {
            locale: Locale::En,
            ..AppSettings::default()
        }
    );
    store.apply(SettingsChange::EditorFontSize(18)).unwrap();
    store.apply(SettingsChange::EditorTabWidth(8)).unwrap();
    store.apply(SettingsChange::TablePageSize(250)).unwrap();
    store.set_layout(320, 420).unwrap();
    // CLI language changes patch the latest document, including GUI preferences.
    let actual = store.set_locale(Locale::Ko).unwrap();
    assert_eq!(
        actual,
        AppSettings {
            locale: Locale::Ko,
            editor_font_size: 18,
            editor_tab_width: 8,
            table_page_size: 250,
            sidebar_width: 320,
            editor_height: 420
        }
    );
    assert_eq!(store.load().unwrap(), actual);
    let value: Value = serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
    assert_eq!(value["future"]["keep"], 7);
}

#[test]
fn invalid_bounds_never_replace_existing_settings() {
    let temp = tempfile::tempdir().unwrap();
    let store = SettingsStore::new(temp.path().join("app-settings.json"));
    store.set_locale(Locale::En).unwrap();
    let original = fs::read(store.path()).unwrap();
    for change in [
        SettingsChange::EditorFontSize(9),
        SettingsChange::EditorFontSize(25),
        SettingsChange::EditorTabWidth(1),
        SettingsChange::EditorTabWidth(9),
        SettingsChange::TablePageSize(49),
        SettingsChange::TablePageSize(501),
        SettingsChange::Layout {
            sidebar_width: 179,
            editor_height: 300,
        },
        SettingsChange::Layout {
            sidebar_width: 280,
            editor_height: 721,
        },
    ] {
        assert!(matches!(store.apply(change), Err(StoreError::InvalidData)));
        assert_eq!(fs::read(store.path()).unwrap(), original);
    }
    fs::write(
        store.path(),
        br#"{"schema_version":1,"locale":"en","table_page_size":65535}"#,
    )
    .unwrap();
    let invalid = fs::read(store.path()).unwrap();
    assert!(store.load().is_err());
    assert!(store.set_locale(Locale::Ko).is_err());
    assert_eq!(fs::read(store.path()).unwrap(), invalid);
}

#[test]
fn independent_concurrent_patches_preserve_each_other_and_unknown_fields() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("app-settings.json");
    fs::write(
        &path,
        br#"{"schema_version":1,"locale":"ko","future":{"keep":true}}"#,
    )
    .unwrap();
    std::thread::scope(|scope| {
        for change in [
            SettingsChange::Locale(Locale::En),
            SettingsChange::EditorFontSize(20),
            SettingsChange::EditorTabWidth(2),
            SettingsChange::TablePageSize(500),
            SettingsChange::Layout {
                sidebar_width: 400,
                editor_height: 500,
            },
        ] {
            let path = path.clone();
            scope.spawn(move || {
                for _ in 0..8 {
                    SettingsStore::new(&path).apply(change).unwrap();
                }
            });
        }
    });
    assert_eq!(
        SettingsStore::new(&path).load().unwrap(),
        AppSettings {
            locale: Locale::En,
            editor_font_size: 20,
            editor_tab_width: 2,
            table_page_size: 500,
            sidebar_width: 400,
            editor_height: 500
        }
    );
    let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(value["future"]["keep"], true);
}
