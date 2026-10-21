//! Real adapter probes against newly created test-owned SQLite files only.
use dbide_bridge::{BridgeEvent, DatabaseBridge};
use dbide_profiles::{ConnectionProfile, ConnectionRequest, DatabaseEngine};
use dbide_ui_spike::navigator::{Data, Key, Navigator, Relation, Scope};
use std::time::{Duration, Instant};

fn request(path: &std::path::Path) -> ConnectionRequest {
    let mut profile = ConnectionProfile::new();
    profile.engine = DatabaseEngine::Sqlite;
    profile.sqlite_path = Some(path.to_str().unwrap().into());
    profile.sqlite_create = false;
    profile.database = "main".into();
    ConnectionRequest {
        profile,
        password: String::new(),
    }
}
fn seed(request: ConnectionRequest, sql: &str) {
    let mut bridge = DatabaseBridge::new().unwrap();
    let configured = bridge.configure(request).unwrap();
    let start = Instant::now();
    let mut executing = None;
    loop {
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "owned SQLite seed deadline"
        );
        match bridge.try_next() {
            Some(BridgeEvent::Configured { operation }) if operation == configured => {
                executing = Some(bridge.execute(sql.into()).unwrap())
            }
            Some(BridgeEvent::Finished { operation, report }) if Some(operation) == executing => {
                assert_eq!(report.outcome, dbide_pg_spike::Outcome::Completed);
                return;
            }
            Some(BridgeEvent::ConnectionFailed { diagnostic, .. }) => {
                panic!("owned SQLite seed: {}", diagnostic.message)
            }
            Some(BridgeEvent::Error { message, .. }) => panic!("owned SQLite seed: {message}"),
            _ => std::thread::sleep(Duration::from_millis(2)),
        }
    }
}
fn loaded(n: &mut Navigator, key: &Key) {
    let start = Instant::now();
    loop {
        n.poll();
        assert!(n.running() <= 2);
        assert!(n.pending() <= 16);
        let branch = n.branch(key).unwrap();
        if let Some(error) = &branch.error {
            panic!("owned SQLite navigation: {error}");
        }
        if branch.snapshot.is_some() {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "owned SQLite metadata deadline"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn real_sqlite_branches_are_lazy_isolated_and_keep_actual_fields() {
    let directory = tempfile::tempdir().unwrap();
    let a_path = directory.path().join("a.sqlite");
    let b_path = directory.path().join("b.sqlite");
    std::fs::File::create(&a_path).unwrap();
    std::fs::File::create(&b_path).unwrap();
    let a = request(&a_path);
    let b = request(&b_path);
    seed(
        a.clone(),
        "CREATE TABLE same_name(id INTEGER PRIMARY KEY, only_a TEXT NOT NULL DEFAULT 'A'); CREATE VIEW a_view AS SELECT id FROM same_name;",
    );
    seed(
        b.clone(),
        "CREATE TABLE same_name(id INTEGER PRIMARY KEY, only_b INTEGER);",
    );
    let mut n = Navigator::default();
    n.sync_profiles(&[a.profile.clone(), b.profile.clone()]);
    assert_eq!(n.running(), 0);
    assert_eq!(n.pending(), 0);
    assert_eq!(n.stats().resident_entries, 0);
    let a_root = n.root(&a.profile.id).unwrap();
    let b_root = n.root(&b.profile.id).unwrap();
    n.expand(a_root.clone(), a.clone(), false).unwrap();
    loaded(&mut n, &a_root);
    assert!(
        matches!(&n.snapshot(&a_root).unwrap().data,Data::Databases(v) if v.len()==1&&v[0].name=="main")
    );
    assert!(n.branch(&b_root).is_none());
    let a_objects = a_root.child("main", Scope::Objects("main".into()));
    let b_objects = b_root.child("main", Scope::Objects("main".into()));
    n.expand(a_objects.clone(), a.clone(), false).unwrap();
    n.expand(b_objects.clone(), b.clone(), false).unwrap();
    loaded(&mut n, &a_objects);
    loaded(&mut n, &b_objects);
    assert!(
        matches!(&n.snapshot(&a_objects).unwrap().data,Data::Objects(v) if v.len()==2&&v.iter().any(|o|o.name=="a_view"&&o.kind=="view"))
    );
    assert!(
        matches!(&n.snapshot(&b_objects).unwrap().data,Data::Objects(v) if v.len()==1&&v[0].name=="same_name")
    );
    let relation = Relation {
        schema: "main".into(),
        name: "same_name".into(),
        kind: "table".into(),
    };
    let a_columns = a_objects.child("main", Scope::Columns(relation.clone()));
    let b_columns = b_objects.child("main", Scope::Columns(relation));
    assert!(n.branch(&a_columns).is_none());
    assert!(n.branch(&b_columns).is_none());
    n.expand(a_columns.clone(), a.clone(), false).unwrap();
    n.expand(b_columns.clone(), b, false).unwrap();
    loaded(&mut n, &a_columns);
    loaded(&mut n, &b_columns);
    assert!(
        matches!(&n.snapshot(&a_columns).unwrap().data,Data::Columns(v) if v[0].primary_key_ordinal==Some(1)&&v.iter().any(|c|c.name=="only_a"&&!c.nullable&&c.default.as_deref()==Some("'A'")))
    );
    assert!(
        matches!(&n.snapshot(&b_columns).unwrap().data,Data::Columns(v) if v.iter().any(|c|c.name=="only_b")&&!v.iter().any(|c|c.name=="only_a"))
    );
    let held = n.snapshot(&a_columns).unwrap();
    let old = n.stats().pinned_bytes;
    n.expand(a_objects.clone(), a, true).unwrap();
    assert!(n.branch(&a_columns).is_none());
    assert!(n.branch(&b_columns).is_some());
    assert!(!held.is_empty());
    assert!(n.stats().pinned_bytes > 0);
    assert!(n.stats().pinned_bytes <= old);
    loaded(&mut n, &a_objects);
}

#[test]
#[ignore = "requires DBIDE_BRIDGE_FIXTURES=owned-loopback and owned PG fixture at127.0.0.1:55439"]
fn owned_postgres_database_discovery_uses_captured_database_per_branch() {
    assert_eq!(
        std::env::var("DBIDE_BRIDGE_FIXTURES").as_deref(),
        Ok("owned-loopback")
    );
    let request = ConnectionRequest {
        profile: ConnectionProfile::fixture(),
        password: "dbide_fixture_only".into(),
    };
    assert_eq!(request.profile.host, "127.0.0.1");
    assert_eq!(request.profile.port, 55439);
    let mut n = Navigator::default();
    n.sync_profiles(std::slice::from_ref(&request.profile));
    let root = n.root(&request.profile.id).unwrap();
    n.expand(root.clone(), request.clone(), false).unwrap();
    loaded(&mut n, &root);
    assert!(
        matches!(&n.snapshot(&root).unwrap().data,Data::Databases(databases) if databases.iter().any(|d|d.name=="dbide_fixture")&&databases.iter().any(|d|d.name=="dbide_console_b"))
    );
    let a = root.child("dbide_fixture", Scope::Schemas);
    let b = root.child("dbide_console_b", Scope::Schemas);
    let mut other = request.clone();
    other.profile.database = "dbide_console_b".into();
    n.expand(a.clone(), request, false).unwrap();
    n.expand(b.clone(), other, false).unwrap();
    loaded(&mut n, &a);
    loaded(&mut n, &b);
    assert!(
        matches!(&n.snapshot(&a).unwrap().data,Data::Schemas(v) if v.iter().any(|s|s.name=="public"))
    );
    assert!(
        matches!(&n.snapshot(&b).unwrap().data,Data::Schemas(v) if v.iter().any(|s|s.name=="public"))
    );
    assert_eq!(n.profile(&a).unwrap().database, "dbide_fixture");
    assert_eq!(n.profile(&b).unwrap().database, "dbide_console_b");
}

#[test]
fn browsing_missing_sqlite_database_never_creates_a_file_even_if_connect_creation_allowed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("missing.sqlite");
    let mut request = request(&path);
    request.profile.sqlite_create = true;
    request.profile.read_only = false;
    let mut n = Navigator::default();
    n.sync_profiles(std::slice::from_ref(&request.profile));
    let root = n.root(&request.profile.id).unwrap();
    n.expand(root.clone(), request, false).unwrap();
    let start = Instant::now();
    loop {
        n.poll();
        if !n.branch(&root).unwrap().loading {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(8));
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(n.branch(&root).unwrap().error.is_some());
    assert!(!path.exists());
    assert!(n.profile(&root).unwrap().sqlite_create);
    assert!(!n.profile(&root).unwrap().read_only);
}
