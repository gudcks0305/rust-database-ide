use dbide_profiles::{
    ConnectionProfile, DatabaseEngine, ExecutionTargetError, HistoryEntry, ProfileStore,
    SCHEMA_VERSION, StoredExecutionTarget, WorkspaceState,
};
use std::fs;

#[test]
fn rename_and_default_database_changes_preserve_explicit_override() {
    let original = ConnectionProfile::default();
    let target = StoredExecutionTarget::capture(&original, "analytics").unwrap();
    let mut current = original.clone();
    current.name = "Renamed".into();
    current.database = "other-default".into();
    let resolved = target.resolve(&current).unwrap();
    assert_eq!(resolved.database, "analytics");
    assert_eq!(resolved.name, "Renamed");
    let serialized = serde_json::to_string(&target).unwrap();
    assert!(!serialized.contains("password"));
    assert!(!serialized.contains(&original.username));
    assert!(!serialized.contains(&original.host));
    let restored: StoredExecutionTarget = serde_json::from_str(&serialized).unwrap();
    assert_eq!(restored.resolve(&current).unwrap().database, "analytics");
}

#[test]
fn every_effective_endpoint_change_requires_reselection() {
    let original = ConnectionProfile::default();
    let target = StoredExecutionTarget::capture(&original, "analytics").unwrap();
    let variants = [
        ConnectionProfile {
            host: "elsewhere".into(),
            ..original.clone()
        },
        ConnectionProfile {
            port: 5433,
            ..original.clone()
        },
        ConnectionProfile {
            username: "other-user".into(),
            ..original.clone()
        },
        ConnectionProfile {
            tls_mode: dbide_profiles::TlsMode::Require,
            ..original.clone()
        },
        ConnectionProfile {
            root_cert_path: Some("/chosen-ca.pem".into()),
            ..original.clone()
        },
        ConnectionProfile {
            read_only: true,
            ..original.clone()
        },
        ConnectionProfile {
            connect_timeout_secs: 20,
            ..original.clone()
        },
    ];
    for changed in variants {
        assert_eq!(
            target.resolve(&changed),
            Err(ExecutionTargetError::EndpointChanged)
        );
    }
    let mut changed = original.clone();
    changed.id = "other-id".into();
    assert_eq!(
        target.resolve(&changed),
        Err(ExecutionTargetError::ProfileMismatch)
    );
    changed = original;
    changed.engine = DatabaseEngine::MySql;
    assert_eq!(
        target.resolve(&changed),
        Err(ExecutionTargetError::ProfileMismatch)
    );
}

#[test]
fn sqlite_logical_main_is_tied_to_effective_file_configuration() {
    let profile = ConnectionProfile {
        engine: DatabaseEngine::Sqlite,
        sqlite_path: Some("/tmp/chosen.sqlite".into()),
        ..ConnectionProfile::default()
    };
    assert!(StoredExecutionTarget::capture(&profile, "other").is_err());
    let target = StoredExecutionTarget::capture(&profile, "main").unwrap();
    let mut irrelevant = profile.clone();
    irrelevant.host.clear();
    irrelevant.username.clear();
    irrelevant.port = 0;
    irrelevant.root_cert_path = Some("/not-read.pem".into());
    assert_eq!(target.resolve(&irrelevant).unwrap().database, "main");
    for changed in [
        ConnectionProfile {
            sqlite_path: Some("/tmp/changed.sqlite".into()),
            ..profile.clone()
        },
        ConnectionProfile {
            sqlite_create: true,
            ..profile.clone()
        },
        ConnectionProfile {
            read_only: true,
            ..profile.clone()
        },
    ] {
        assert_eq!(
            target.resolve(&changed),
            Err(ExecutionTargetError::EndpointChanged)
        );
    }
}

#[test]
fn legacy_history_migrates_without_binding_to_current_profile() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profiles.json");
    let mut state = WorkspaceState::default();
    let profile = ConnectionProfile::default();
    state.upsert(profile.clone()).unwrap();
    state.select(Some(&profile.id)).unwrap();
    state.persist_history = true;
    state
        .record_history(HistoryEntry {
            profile_id: profile.id.clone(),
            sql: "SELECT 1".into(),
            executed_at_unix_ms: 1,
            outcome: "completed".into(),
            row_count: Some(1),
            execution_target: None,
        })
        .unwrap();
    let mut legacy = serde_json::to_value(&state).unwrap();
    legacy["schema_version"] = 1.into();
    legacy["history"][0]
        .as_object_mut()
        .unwrap()
        .remove("execution_target");
    let bytes = serde_json::to_vec(&legacy).unwrap();
    fs::write(&path, &bytes).unwrap();
    let store = ProfileStore::open(&path).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.schema_version, SCHEMA_VERSION);
    assert!(loaded.history[0].execution_target.is_none());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    store.save(&loaded).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(path).unwrap()).unwrap()["schema_version"],
        SCHEMA_VERSION
    );
}

#[test]
fn malformed_stored_target_preserves_original_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profiles.json");
    let store = ProfileStore::open(&path).unwrap();
    let profile = ConnectionProfile::default();
    let mut state = WorkspaceState::default();
    state.upsert(profile.clone()).unwrap();
    state.persist_history = true;
    let mut target = StoredExecutionTarget::capture(&profile, "analytics").unwrap();
    target.endpoint_fingerprint = "invalid".into();
    state.history.push(HistoryEntry {
        profile_id: profile.id,
        sql: "SELECT 1".into(),
        executed_at_unix_ms: 1,
        outcome: "completed".into(),
        row_count: None,
        execution_target: Some(target),
    });
    let bytes = serde_json::to_vec(&state).unwrap();
    fs::write(&path, &bytes).unwrap();
    assert!(store.load().is_err());
    assert!(store.save(&WorkspaceState::default()).is_err());
    assert_eq!(fs::read(path).unwrap(), bytes);
}
