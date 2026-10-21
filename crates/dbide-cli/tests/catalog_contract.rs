//! Offline CLI catalog inspection; only test-owned private metadata files.
use dbide_catalog_store::{Data, Key, Object, Scope, Snapshot, Store};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

fn run(parent: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dbide"))
        .arg("--config")
        .arg(parent.join("profiles.json"))
        .args(args)
        .env_remove("DATABASE_URL")
        .env_remove("PGPASSWORD")
        .env_remove("MYSQL_PWD")
        .output()
        .unwrap()
}
fn json(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "catalog reads must not connect or print progress"
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn fixture() -> (TempDir, PathBuf, Store, Key, Snapshot) {
    let temporary = TempDir::new().unwrap();
    // The store refuses symlink ancestors; macOS /var is an alias of /private/var.
    let parent = temporary.path().canonicalize().unwrap();
    let store = Store::new(parent.join("catalog-cache"));
    let key = Key {
        profile_id: "owned-fixture".into(),
        fingerprint: "fixture-endpoint-fingerprint".into(),
        database: "fixture-db".into(),
        scope: Scope::Objects("public".into()),
    };
    let snapshot = Snapshot {
        captured_unix_ms: 1234,
        truncated: true,
        fields_partial: false,
        data: Data::Objects(vec![Object {
            schema: "public".into(),
            name: "fixture_customers".into(),
            kind: "table".into(),
        }]),
    };
    (temporary, parent, store, key, snapshot)
}

#[test]
fn missing_cache_is_empty_without_creation_or_profile_parsing() {
    let (_temporary, parent, _store, _key, _snapshot) = fixture();
    let help = run(&parent, &["catalog", "--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("SHARD_ID"));
    assert_eq!(run(&parent, &["catalog", "show"]).status.code(), Some(2));
    assert_eq!(fs::read_dir(&parent).unwrap().count(), 0);
    fs::write(
        parent.join("profiles.json"),
        b"{intentionally malformed profile",
    )
    .unwrap();
    assert_eq!(
        json(run(&parent, &["catalog", "list"])),
        serde_json::json!([])
    );
    let id = format!("{}.json", "0".repeat(64));
    let missing = run(&parent, &["catalog", "show", &id]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(missing.stdout.is_empty());
    assert!(!parent.join("catalog-cache").exists());
    assert_eq!(
        fs::read(parent.join("profiles.json")).unwrap(),
        b"{intentionally malformed profile"
    );
}

#[test]
fn list_reads_only_manifest_and_show_reads_one_exact_snapshot() {
    let (_temporary, parent, mut store, key, snapshot) = fixture();
    store.save(key.clone(), snapshot.clone()).unwrap();
    let mut unrelated = key.clone();
    unrelated.database = "unrelated-db".into();
    store.save(unrelated.clone(), snapshot.clone()).unwrap();
    let root = parent.join("catalog-cache");
    // A malformed unrelated shard cannot break manifest-only listing or selected read.
    fs::write(root.join(unrelated.filename()), b"{unrelated-shard-corrupt").unwrap();
    let listing = json(run(&parent, &["catalog", "list"]));
    let entries = listing.as_array().unwrap();
    assert_eq!(entries.len(), 2);
    let entry = entries
        .iter()
        .find(|entry| entry["shard_id"] == key.filename())
        .unwrap();
    assert_eq!(entry["key"], serde_json::to_value(&key).unwrap());
    assert_eq!(entry["captured_unix_ms"], 1234);
    assert!(entry["bytes"].as_u64().unwrap() > 0);
    assert!(entry.get("snapshot").is_none());
    assert!(!listing.to_string().contains("fixture_customers"));
    let shown = json(run(&parent, &["catalog", "show", &key.filename()]));
    assert_eq!(shown["shard_id"], key.filename());
    assert_eq!(shown["historical"], true);
    assert_eq!(shown["key"], serde_json::to_value(&key).unwrap());
    assert_eq!(shown["snapshot"], serde_json::to_value(&snapshot).unwrap());
    let corrupt = run(&parent, &["catalog", "show", &unrelated.filename()]);
    assert_eq!(corrupt.status.code(), Some(2));
    assert!(corrupt.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&corrupt.stderr).contains("unrelated-shard-corrupt"));
    assert_eq!(
        fs::read(root.join(unrelated.filename())).unwrap(),
        b"{unrelated-shard-corrupt"
    );
}

#[test]
fn exact_ids_cannot_be_replaced_with_paths_or_unknown_shards() {
    let (_temporary, parent, mut store, key, snapshot) = fixture();
    store.save(key.clone(), snapshot).unwrap();
    let outside = parent.join("outside.json");
    fs::write(&outside, b"private-test-sentinel").unwrap();
    for id in [
        "../outside.json".to_owned(),
        outside.to_str().unwrap().to_owned(),
        format!("./{}", key.filename()),
        key.filename().to_uppercase(),
        key.filename().trim_end_matches(".json").to_owned(),
        format!("{}.json", "0".repeat(64)),
    ] {
        let output = run(&parent, &["catalog", "show", &id]);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("private-test-sentinel"));
    }
    assert_eq!(fs::read(outside).unwrap(), b"private-test-sentinel");
    json(run(&parent, &["catalog", "show", &key.filename()]));
}

#[test]
fn malformed_manifest_is_reported_and_preserved() {
    let (_temporary, parent, mut store, key, snapshot) = fixture();
    store.save(key.clone(), snapshot).unwrap();
    let index = parent.join("catalog-cache/index-v1.json");
    fs::write(&index, b"{malformed-manifest-sentinel").unwrap();
    for args in [
        &["catalog", "list"][..],
        &["catalog", "show", &key.filename()][..],
    ] {
        let output = run(&parent, args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("malformed-manifest-sentinel"));
        assert_eq!(fs::read(&index).unwrap(), b"{malformed-manifest-sentinel");
    }
}
