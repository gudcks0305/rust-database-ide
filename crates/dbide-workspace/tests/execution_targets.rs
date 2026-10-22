use dbide_profiles::{ConnectionProfile, StoredExecutionTarget};
use dbide_workspace::{DocumentSnapshot, SCHEMA_VERSION, WorkspaceSnapshot, WorkspaceStore};
use std::fs;
fn snapshot() -> WorkspaceSnapshot {
    let profile = ConnectionProfile::default();
    WorkspaceSnapshot {
        schema_version: SCHEMA_VERSION,
        documents: vec![DocumentSnapshot {
            project_root: None,
            id: 1,
            title: "Console".into(),
            sql: "SELECT 2".into(),
            saved_sql: "SELECT 1".into(),
            revision: 3,
            path: None,
            profile_id: Some(profile.id.clone()),
            execution_target: Some(StoredExecutionTarget::capture(&profile, "analytics").unwrap()),
        }],
        active_document_id: Some(1),
    }
}
#[test]
fn document_round_trip_preserves_execution_target_and_dirty_state() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceStore::new(dir.path().join("workspace.json"));
    let snapshot = snapshot();
    store.save(&snapshot).unwrap();
    assert_eq!(store.load().unwrap().unwrap(), snapshot);
    assert!(snapshot.documents[0].dirty());
}
#[test]
fn legacy_document_context_is_unresolved_and_migration_does_not_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.json");
    let mut legacy = serde_json::to_value(snapshot()).unwrap();
    legacy["schema_version"] = 1.into();
    legacy["documents"][0]
        .as_object_mut()
        .unwrap()
        .remove("execution_target");
    let bytes = serde_json::to_vec(&legacy).unwrap();
    fs::write(&path, &bytes).unwrap();
    let store = WorkspaceStore::open(&path).unwrap();
    let recovered = store.load().unwrap().unwrap();
    assert_eq!(recovered.schema_version, SCHEMA_VERSION);
    assert!(recovered.documents[0].execution_target.is_none());
    assert!(recovered.documents[0].profile_id.is_some());
    assert!(recovered.documents[0].dirty());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    store.save(&recovered).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(path).unwrap()).unwrap()["schema_version"],
        SCHEMA_VERSION
    );
}
