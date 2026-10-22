use dbide_workspace::{DocumentSnapshot, SCHEMA_VERSION, WorkspaceSnapshot, WorkspaceStore};
use std::path::PathBuf;
fn snapshot() -> WorkspaceSnapshot {
    WorkspaceSnapshot {
        schema_version: SCHEMA_VERSION,
        active_document_id: Some(1),
        documents: vec![DocumentSnapshot {
            id: 1,
            title: "draft.sql".into(),
            sql: "SELECT 'unsaved';".into(),
            saved_sql: "SELECT 'disk';".into(),
            revision: 2,
            path: Some(PathBuf::from("/missing/project/queries/draft.sql")),
            project_root: Some(PathBuf::from("/missing/project")),
            profile_id: None,
            execution_target: None,
        }],
    }
}
#[test]
fn project_root_and_dirty_baseline_restore_without_reading_original_paths() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceStore::new(dir.path().join("recovery.json"));
    let expected = snapshot();
    store.save(&expected).unwrap();
    assert_eq!(store.load().unwrap().unwrap(), expected);
    assert!(expected.documents[0].dirty());
}
#[test]
fn version_two_without_root_migrates_and_escaping_root_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("recovery.json");
    let store = WorkspaceStore::new(&path);
    let mut value = serde_json::to_value(snapshot()).unwrap();
    value["schema_version"] = 2.into();
    value["documents"][0]
        .as_object_mut()
        .unwrap()
        .remove("project_root");
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let loaded = store.load().unwrap().unwrap();
    assert_eq!(loaded.schema_version, SCHEMA_VERSION);
    assert!(loaded.documents[0].project_root.is_none());
    let mut invalid = snapshot();
    invalid.documents[0].path = Some("/missing/project/../other.sql".into());
    assert!(invalid.validate().is_err());
    invalid.documents[0].path = Some("/outside/file.sql".into());
    assert!(invalid.validate().is_err());
}
