//! Separate processes exercise the same lock used by GUI and CLI writers.
use dbide_profiles::{ConnectionProfile, HistoryEntry, ProfileStore, StoreError};
use std::{fs, process::Command, time::Duration};

#[test]
fn process_update_worker() {
    let Some(path) = std::env::var_os("DBIDE_PROFILES_TEST_STORE") else {
        return;
    };
    let worker = std::env::var("DBIDE_PROFILES_TEST_WORKER").unwrap();
    let store = ProfileStore::open(path).unwrap();
    for iteration in 0..8 {
        store
            .update(|state| {
                // Widen contention to detect an unlocked read/modify/write implementation.
                std::thread::sleep(Duration::from_millis(5));
                let id = format!("worker-{worker}-{iteration}");
                state.upsert(ConnectionProfile {
                    id: id.clone(),
                    name: id.clone(),
                    ..ConnectionProfile::default()
                })?;
                state.record_history(HistoryEntry {
                    profile_id: id,
                    sql: "SELECT 1".into(),
                    executed_at_unix_ms: iteration,
                    outcome: "completed".into(),
                    row_count: Some(1),
                    execution_target: None,
                })
            })
            .unwrap();
    }
}

#[test]
fn concurrent_processes_preserve_every_profile_history_and_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profiles.json");
    let store = ProfileStore::open(&path).unwrap();
    store
        .update(|state| {
            state.upsert(ConnectionProfile::fixture())?;
            state.select(Some("dbide-local-fixture"))?;
            state.persist_history = true;
            Ok(())
        })
        .unwrap();
    let binary = std::env::current_exe().unwrap();
    let children: Vec<_> = (0..4)
        .map(|worker| {
            Command::new(&binary)
                .args(["--exact", "process_update_worker"])
                .env("DBIDE_PROFILES_TEST_STORE", &path)
                .env("DBIDE_PROFILES_TEST_WORKER", worker.to_string())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "fixture worker failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let final_state = store.load().unwrap();
    assert_eq!(final_state.profiles.len(), 33);
    assert_eq!(final_state.history.len(), 32);
    assert!(final_state.persist_history);
    assert_eq!(
        final_state.selected_profile_id.as_deref(),
        Some("dbide-local-fixture")
    );
    for worker in 0..4 {
        for iteration in 0..8 {
            let id = format!("worker-{worker}-{iteration}");
            assert!(final_state.profiles.iter().any(|p| p.id == id));
            assert_eq!(
                final_state
                    .history
                    .iter()
                    .filter(|h| h.profile_id == id)
                    .count(),
                1
            );
        }
    }
    let lock_path = path.with_extension("json.lock");
    assert!(lock_path.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(lock_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn failed_update_preserves_original_and_releases_writer_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profiles.json");
    let store = ProfileStore::open(&path).unwrap();
    store
        .update(|state| state.upsert(ConnectionProfile::fixture()))
        .unwrap();
    let original = fs::read(&path).unwrap();
    let result = store.update(|state| {
        state.profiles.clear();
        Err(StoreError::InvalidState("fixture callback rejected".into()))
    });
    assert!(result.is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(
        store
            .update(|state| {
                state.selected_profile_id = Some("missing".into());
                Ok(())
            })
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    let fresh = store
        .update(|state| state.select(Some("dbide-local-fixture")))
        .unwrap();
    assert_eq!(
        fresh.selected_profile_id.as_deref(),
        Some("dbide-local-fixture")
    );
}

#[test]
fn stale_snapshot_cannot_remove_independent_updates_when_using_mutations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profiles.json");
    let gui = ProfileStore::open(&path).unwrap();
    let cli = ProfileStore::open(&path).unwrap();
    let _old_gui_state = gui.load().unwrap();
    let profile = ConnectionProfile::fixture();
    cli.update(|state| state.upsert(profile.clone())).unwrap();
    let fresh = gui
        .update(|state| {
            state.persist_history = true;
            Ok(())
        })
        .unwrap();
    assert_eq!(fresh.profiles, vec![profile]);
    assert!(fresh.persist_history);
}
