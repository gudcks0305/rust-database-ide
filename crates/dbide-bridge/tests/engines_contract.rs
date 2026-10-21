//! Real native SQLite files run offline; network tests use only owned loopback fixtures.
use dbide_bridge::{
    BridgeEvent, CommitPolicy, DatabaseBridge, ExecutionReport, OperationId, Outcome,
    ParameterizedStatement, SchemaColumn, SchemaObject, ServerTransactionState, TableIdentityGuard,
};
use dbide_driver_api::Event;
use dbide_profiles::{ConnectionProfile, ConnectionRequest, DatabaseEngine, TlsMode};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static FILE_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
struct File(std::path::PathBuf);
impl File {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = FILE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!(
            "dbide-bridge-{}-{stamp}-{sequence}.sqlite",
            std::process::id()
        )))
    }
    fn request(&self) -> ConnectionRequest {
        ConnectionRequest {
            profile: ConnectionProfile {
                engine: DatabaseEngine::Sqlite,
                sqlite_path: Some(self.0.to_string_lossy().into_owned()),
                sqlite_create: true,
                connect_timeout_secs: 2,
                ..ConnectionProfile::default()
            },
            password: "not-passed-to-sqlite".into(),
        }
    }
}
impl Drop for File {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn next(bridge: &mut DatabaseBridge) -> BridgeEvent {
    let start = Instant::now();
    loop {
        if let Some(event) = bridge.try_next() {
            return event;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "bridge event deadline"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn configure(bridge: &mut DatabaseBridge, request: ConnectionRequest) {
    let id = bridge.configure(request).unwrap();
    assert!(matches!(next(bridge),BridgeEvent::Configured{operation} if operation==id));
}
fn terminal(bridge: &mut DatabaseBridge, id: OperationId) -> ExecutionReport {
    loop {
        match next(bridge) {
            BridgeEvent::Connecting { operation, .. }
            | BridgeEvent::Connected { operation }
            | BridgeEvent::Executing { operation, .. } => assert_eq!(operation, id),
            BridgeEvent::Finished { operation, report } => {
                assert_eq!(operation, id);
                return report;
            }
            event => panic!("unexpected event: {event:?}"),
        }
    }
}
fn execute(bridge: &mut DatabaseBridge, sql: &str) -> ExecutionReport {
    let id = bridge.execute(sql.into()).unwrap();
    terminal(bridge, id)
}
fn completed(report: ExecutionReport) -> ExecutionReport {
    assert_eq!(
        report.outcome,
        Outcome::Completed,
        "{:?}",
        report.terminal_error
    );
    report
}
fn scalar(report: &ExecutionReport) -> String {
    report
        .events
        .iter()
        .find_map(|event| match event {
            Event::Row { values } => values.first().cloned().flatten(),
            _ => None,
        })
        .unwrap()
}
fn schema(bridge: &mut DatabaseBridge) -> (Vec<SchemaObject>, bool) {
    let id = bridge.inspect_schema().unwrap();
    assert!(matches!(next(bridge),BridgeEvent::Connecting{operation,..} if operation==id));
    match next(bridge) {
        BridgeEvent::SchemaLoaded {
            operation,
            objects,
            truncated,
        } => {
            assert_eq!(operation, id);
            (objects, truncated)
        }
        event => panic!("{event:?}"),
    }
}
#[test]
fn sqlite_file_autoconnect_metadata_controls_and_idle_switch() {
    let file = File::new();
    let other = File::new();
    let mut bridge = DatabaseBridge::new().unwrap();
    configure(&mut bridge, file.request());
    completed(execute(
        &mut bridge,
        "CREATE TABLE bridge_fixture(id INTEGER PRIMARY KEY, value TEXT NOT NULL); CREATE TEMP TABLE local_fixture(v INTEGER); INSERT INTO local_fixture VALUES(17)",
    ));
    let (objects, truncated) = schema(&mut bridge);
    assert!(!truncated);
    let object = objects.iter().find(|o| o.name == "bridge_fixture").unwrap();
    assert_eq!(object.schema, "main");
    assert_eq!(object.kind, "table");
    assert_eq!(object.columns.len(), 2);
    assert_eq!(object.columns[1].name, "value");
    assert!(!object.columns[1].nullable);
    assert!(!objects.iter().any(|o| o.name == "local_fixture"));
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT v FROM local_fixture"
        ))),
        "17"
    );
    bridge.set_commit_policy(CommitPolicy::Manual).unwrap();
    assert!(matches!(
        next(&mut bridge),
        BridgeEvent::CommitPolicyChanged {
            policy: CommitPolicy::Manual,
            ..
        }
    ));
    let report = completed(execute(
        &mut bridge,
        "INSERT INTO bridge_fixture VALUES(1,'pending')",
    ));
    assert_eq!(report.transaction_state, ServerTransactionState::Active);
    bridge.configure(other.request()).unwrap();
    assert!(
        matches!(next(&mut bridge),BridgeEvent::Error{message,..} if message.contains("preserved"))
    );
    // The metadata probe sees only committed schema and leaves the transaction intact.
    assert!(
        schema(&mut bridge)
            .0
            .iter()
            .any(|o| o.name == "bridge_fixture")
    );
    let rollback = bridge.rollback().unwrap();
    assert_eq!(
        completed(terminal(&mut bridge, rollback)).transaction_state,
        ServerTransactionState::Idle
    );
    bridge.set_commit_policy(CommitPolicy::Auto).unwrap();
    next(&mut bridge);
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT count(*) FROM bridge_fixture"
        ))),
        "0"
    );
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT v FROM local_fixture"
        ))),
        "17"
    );
    configure(&mut bridge, other.request());
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT count(*) FROM sqlite_schema WHERE name='bridge_fixture'"
        ))),
        "0"
    );
    configure(&mut bridge, file.request());
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT count(*) FROM sqlite_schema WHERE name='bridge_fixture'"
        ))),
        "1"
    );
}
#[test]
fn sqlite_cancel_exact_execution_preserves_connection_and_stale_target_is_safe() {
    let file = File::new();
    let mut bridge = DatabaseBridge::new().unwrap();
    configure(&mut bridge, file.request());
    let first = completed(execute(
        &mut bridge,
        "CREATE TEMP TABLE kept(v); INSERT INTO kept VALUES(23); SELECT 23",
    ));
    let id=bridge.execute("WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n".into()).unwrap();
    let key = match next(&mut bridge) {
        BridgeEvent::Executing {
            operation,
            execution,
        } => {
            assert_eq!(operation, id);
            execution
        }
        e => panic!("{e:?}"),
    };
    let cancel = bridge.cancel(key).unwrap();
    assert!(
        matches!(next(&mut bridge),BridgeEvent::Cancelling{operation,execution} if operation==cancel && execution==key)
    );
    let cancelled = terminal(&mut bridge, id);
    assert_eq!(cancelled.outcome, Outcome::Cancelled);
    assert!(!cancelled.connection_retired);
    let later = completed(execute(&mut bridge, "SELECT v FROM kept"));
    assert_eq!(later.key.session, first.key.session);
    assert_eq!(scalar(&later), "23");
    bridge.cancel(key).unwrap();
    assert!(matches!(next(&mut bridge), BridgeEvent::Error { .. }));
    assert_eq!(
        scalar(&completed(execute(&mut bridge, "SELECT v FROM kept"))),
        "23"
    );
}
#[test]
fn sqlite_pending_operation_cancel_prevents_dispatch_or_interrupts_exact_query() {
    let file = File::new();
    let mut bridge = DatabaseBridge::new().unwrap();
    configure(&mut bridge, file.request());
    let id=bridge.execute("WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n".into()).unwrap();
    let cancel = bridge.cancel(id).unwrap();
    let mut terminal = false;
    let mut acknowledged = false;
    while !terminal || !acknowledged {
        match next(&mut bridge) {
            BridgeEvent::Connecting { operation, .. }
            | BridgeEvent::Connected { operation }
            | BridgeEvent::Executing { operation, .. } => assert_eq!(operation, id),
            BridgeEvent::Error {
                operation: Some(operation),
                message,
            } if operation == id => {
                assert!(message.contains("cancelled"));
                terminal = true;
            }
            BridgeEvent::Finished { operation, report } => {
                assert_eq!(operation, id);
                assert_eq!(report.outcome, Outcome::Cancelled);
                terminal = true;
            }
            BridgeEvent::Cancelling { operation, .. }
            | BridgeEvent::Error {
                operation: Some(operation),
                ..
            } if operation == cancel => acknowledged = true,
            e => panic!("{e:?}"),
        }
    }
    completed(execute(&mut bridge, "SELECT 1"));
}
#[test]
fn sqlite_missing_file_diagnostic_and_readonly_write_failure_are_safe() {
    let file = File::new();
    let mut bridge = DatabaseBridge::new().unwrap();
    let mut request = file.request();
    request.profile.sqlite_create = false;
    configure(&mut bridge, request);
    bridge.connect().unwrap();
    assert!(
        matches!(next(&mut bridge),BridgeEvent::Connecting{target,..} if target.host=="SQLite" && target.port==0)
    );
    match next(&mut bridge) {
        BridgeEvent::ConnectionFailed { diagnostic, .. } => {
            assert_eq!(
                diagnostic.kind,
                dbide_bridge::ConnectionErrorKind::DatabaseMissing
            );
            assert!(!format!("{diagnostic:?}").contains("not-passed-to-sqlite"));
        }
        e => panic!("{e:?}"),
    }
    configure(&mut bridge, file.request());
    completed(execute(&mut bridge, "CREATE TABLE r(v)"));
    let mut request = file.request();
    request.profile.sqlite_create = false;
    request.profile.read_only = true;
    configure(&mut bridge, request);
    assert_eq!(
        execute(&mut bridge, "INSERT INTO r VALUES(1)").outcome,
        Outcome::Failed
    );
    assert_eq!(
        scalar(&completed(execute(&mut bridge, "SELECT count(*) FROM r"))),
        "0"
    );
}
fn batch() -> Vec<ParameterizedStatement> {
    vec![ParameterizedStatement {
        sql: "INSERT INTO bridge_batch(v) VALUES($1)".into(),
        parameters: vec![Some("value".into())],
        expected_affected_rows: Some(1),
    }]
}
#[test]
fn apply_never_connects_and_rejects_unsupported_engine_without_sql() {
    let file = File::new();
    let mut bridge = DatabaseBridge::new().unwrap();
    configure(&mut bridge, file.request());
    let id = bridge.apply_batch(batch()).unwrap();
    assert!(
        matches!(next(&mut bridge),BridgeEvent::Error{operation:Some(operation),message} if operation==id && message.contains("existing connection"))
    );
    assert!(
        !file.0.exists(),
        "apply must not silently create/connect a file"
    );
    bridge
        .apply_guarded_batch(
            TableIdentityGuard {
                schema: "public".into(),
                table: "bridge_batch".into(),
                oid: 1,
                metadata_fingerprint: None,
            },
            batch(),
        )
        .unwrap();
    assert!(
        matches!(next(&mut bridge),BridgeEvent::Error{message,..} if message.contains("existing connection"))
    );
    assert!(!file.0.exists());
    completed(execute(&mut bridge, "CREATE TABLE bridge_batch(v TEXT)"));
    bridge.apply_batch(batch()).unwrap();
    assert!(
        matches!(next(&mut bridge),BridgeEvent::Error{message,..} if message.contains("only for PostgreSQL"))
    );
    bridge
        .apply_guarded_batch(
            TableIdentityGuard {
                schema: "public".into(),
                table: "bridge_batch".into(),
                oid: 1,
                metadata_fingerprint: None,
            },
            batch(),
        )
        .unwrap();
    assert!(
        matches!(next(&mut bridge),BridgeEvent::Error{message,..} if message.contains("only for PostgreSQL"))
    );
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT count(*) FROM bridge_batch"
        ))),
        "0"
    );
}
#[test]
fn sqlite_schema_is_bounded_and_quoted_names_are_data() {
    let file = File::new();
    let mut bridge = DatabaseBridge::new().unwrap();
    configure(&mut bridge, file.request());
    completed(execute(
        &mut bridge,
        "CREATE TABLE \"a'\"\"; SELECT bad\"(\"column'\"\"\" TEXT)",
    ));
    let objects = schema(&mut bridge).0;
    assert_eq!(objects[0].name, "a'\"; SELECT bad");
    assert_eq!(objects[0].columns[0].name, "column'\"");
    let columns = (0..1100)
        .map(|n| format!("c{n} TEXT"))
        .collect::<Vec<_>>()
        .join(",");
    completed(execute(
        &mut bridge,
        &format!("CREATE TABLE b({columns}); CREATE TABLE c({columns})"),
    ));
    let (objects, truncated) = schema(&mut bridge);
    assert!(truncated);
    let retained_columns = objects.iter().map(|o| o.columns.len()).sum::<usize>();
    assert!(retained_columns > 0 && retained_columns <= 2048);
    // The unchanged driver byte budget can stop before the hard 2,048-row cap.
}
fn fixture(engine: DatabaseEngine) -> ConnectionRequest {
    assert_eq!(
        std::env::var("DBIDE_BRIDGE_FIXTURES").as_deref(),
        Ok("owned-loopback"),
        "set DBIDE_BRIDGE_FIXTURES=owned-loopback"
    );
    let mut profile = ConnectionProfile::fixture();
    profile.engine = engine;
    if engine == DatabaseEngine::MySql {
        profile.port = 55441;
        profile.username = "root".into();
    }
    profile.tls_mode = TlsMode::Disable;
    ConnectionRequest {
        profile,
        password: "dbide_fixture_only".into(),
    }
}
fn network_contract(engine: DatabaseEngine) {
    let mut bridge = DatabaseBridge::new().unwrap();
    configure(&mut bridge, fixture(engine));
    completed(execute(
        &mut bridge,
        "DROP TABLE IF EXISTS bridge_engine_fixture; CREATE TABLE bridge_engine_fixture(id INTEGER PRIMARY KEY, value VARCHAR(30) NOT NULL)",
    ));
    assert!(
        schema(&mut bridge)
            .0
            .iter()
            .any(|o| o.name == "bridge_engine_fixture" && o.columns.len() == 2)
    );
    completed(execute(
        &mut bridge,
        "CREATE TEMPORARY TABLE bridge_session_temp(v INTEGER); INSERT INTO bridge_session_temp VALUES(9)",
    ));
    bridge.set_commit_policy(CommitPolicy::Manual).unwrap();
    next(&mut bridge);
    assert_eq!(
        completed(execute(
            &mut bridge,
            "INSERT INTO bridge_engine_fixture VALUES(1,'x')"
        ))
        .transaction_state,
        ServerTransactionState::Active
    );
    let file = File::new();
    bridge.configure(file.request()).unwrap();
    assert!(
        matches!(next(&mut bridge),BridgeEvent::Error{message,..} if message.contains("preserved"))
    );
    let id = bridge.rollback().unwrap();
    completed(terminal(&mut bridge, id));
    bridge.set_commit_policy(CommitPolicy::Auto).unwrap();
    next(&mut bridge);
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT v FROM bridge_session_temp"
        ))),
        "9"
    );
    configure(&mut bridge, file.request());
    completed(execute(&mut bridge, "CREATE TABLE selected_local(v)"));
    configure(&mut bridge, fixture(engine));
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT count(*) FROM bridge_engine_fixture"
        ))),
        "0"
    );
    completed(execute(&mut bridge, "DROP TABLE bridge_engine_fixture"));
}
#[test]
#[ignore = "requires explicitly owned PostgreSQL fixture at 55439 and DBIDE_BRIDGE_FIXTURES=owned-loopback"]
fn postgresql_engine_dispatch_metadata_and_switching() {
    network_contract(DatabaseEngine::PostgreSql);
}
#[test]
#[ignore = "requires explicitly owned MySQL fixture at 55441 and DBIDE_BRIDGE_FIXTURES=owned-loopback"]
fn mysql_engine_dispatch_metadata_and_switching() {
    network_contract(DatabaseEngine::MySql);
}
#[test]
#[ignore = "requires explicitly owned PostgreSQL fixture at 55439 and DBIDE_BRIDGE_FIXTURES=owned-loopback"]
fn postgres_apply_batch_obeys_policy_and_existing_session() {
    let mut bridge = DatabaseBridge::new().unwrap();
    configure(&mut bridge, fixture(DatabaseEngine::PostgreSql));
    completed(execute(
        &mut bridge,
        "DROP TABLE IF EXISTS bridge_batch; CREATE TABLE bridge_batch(v TEXT)",
    ));
    bridge.set_commit_policy(CommitPolicy::Manual).unwrap();
    next(&mut bridge);
    let id = bridge.apply_batch(batch()).unwrap();
    let applied = completed(terminal(&mut bridge, id));
    assert_eq!(applied.transaction_state, ServerTransactionState::Active);
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT count(*) FROM bridge_batch"
        ))),
        "1"
    );
    let id = bridge.rollback().unwrap();
    completed(terminal(&mut bridge, id));
    bridge.set_commit_policy(CommitPolicy::Auto).unwrap();
    next(&mut bridge);
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT count(*) FROM bridge_batch"
        ))),
        "0"
    );
    completed(execute(&mut bridge, "DROP TABLE bridge_batch"));
}

#[test]
#[ignore = "requires explicitly owned PostgreSQL fixture at 55439 and DBIDE_BRIDGE_FIXTURES=owned-loopback"]
fn postgres_guarded_apply_routes_identity_check_and_preserves_existing_transaction() {
    let mut bridge = DatabaseBridge::new().unwrap();
    configure(&mut bridge, fixture(DatabaseEngine::PostgreSql));
    let initial = completed(execute(
        &mut bridge,
        "DROP TABLE IF EXISTS bridge_guarded_fixture; CREATE TABLE bridge_guarded_fixture(v TEXT); CREATE TEMP TABLE bridge_guarded_session(v INTEGER); INSERT INTO bridge_guarded_session VALUES(31); SELECT 'public.bridge_guarded_fixture'::regclass::oid::text",
    ));
    let oid = scalar(&initial).parse::<u32>().unwrap();
    let guard = TableIdentityGuard {
        schema: "public".into(),
        table: "bridge_guarded_fixture".into(),
        oid,
        metadata_fingerprint: None,
    };
    let batch = || {
        vec![ParameterizedStatement {
            sql: "INSERT INTO public.bridge_guarded_fixture(v) VALUES($1)".into(),
            parameters: vec![Some("guarded".into())],
            expected_affected_rows: Some(1),
        }]
    };
    let replaced = completed(execute(
        &mut bridge,
        "BEGIN; DROP TABLE bridge_guarded_fixture; CREATE TABLE bridge_guarded_fixture(v TEXT)",
    ));
    assert_eq!(replaced.transaction_state, ServerTransactionState::Active);
    let id = bridge.apply_guarded_batch(guard.clone(), batch()).unwrap();
    let rejected = terminal(&mut bridge, id);
    assert_eq!(rejected.outcome, Outcome::Failed);
    let diagnostic = rejected
        .terminal_error
        .as_ref()
        .expect("identity mismatch diagnostic");
    assert!(diagnostic.message.starts_with("TableIdentityChanged:"));
    assert_eq!(diagnostic.sqlstate, None);
    assert_eq!(rejected.transaction_state, ServerTransactionState::Active);
    assert_eq!(rejected.key.session, initial.key.session);
    assert!(!rejected.connection_retired);
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT count(*) FROM bridge_guarded_fixture"
        ))),
        "0"
    );
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT v FROM bridge_guarded_session"
        ))),
        "31"
    );
    let id = bridge.rollback().unwrap();
    completed(terminal(&mut bridge, id));
    // Rollback restored the original table OID. The same guard now succeeds.
    let id = bridge.apply_guarded_batch(guard, batch()).unwrap();
    let applied = completed(terminal(&mut bridge, id));
    assert_eq!(applied.key.session, initial.key.session);
    assert_eq!(applied.transaction_state, ServerTransactionState::Idle);
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT v FROM bridge_guarded_fixture"
        ))),
        "guarded"
    );
    completed(execute(&mut bridge, "DROP TABLE bridge_guarded_fixture"));
}

#[test]
#[ignore = "requires explicitly owned PostgreSQL fixture at 55439 and DBIDE_BRIDGE_FIXTURES=owned-loopback"]
fn postgres_guarded_metadata_fingerprint_routes_exact_and_stale_values() {
    let mut bridge = DatabaseBridge::new().unwrap();
    configure(&mut bridge, fixture(DatabaseEngine::PostgreSql));
    let initial = completed(execute(
        &mut bridge,
        "DROP TABLE IF EXISTS bridge_metadata_fixture; CREATE TABLE bridge_metadata_fixture(id INTEGER PRIMARY KEY, value TEXT); CREATE TEMP TABLE bridge_metadata_session(v INTEGER); INSERT INTO bridge_metadata_session VALUES(35); SELECT 'public.bridge_metadata_fixture'::regclass::oid::text",
    ));
    let oid = scalar(&initial).parse::<u32>().unwrap();
    let canonical = scalar(&completed(execute(
        &mut bridge,
        &dbide_pg_spike::table_metadata_query(oid),
    )));
    let fingerprint = dbide_pg_spike::table_metadata_fingerprint(&canonical);
    let guard = TableIdentityGuard {
        schema: "public".into(),
        table: "bridge_metadata_fixture".into(),
        oid,
        metadata_fingerprint: Some(fingerprint),
    };
    bridge.set_commit_policy(CommitPolicy::Manual).unwrap();
    next(&mut bridge);
    let id = bridge
        .apply_guarded_batch(
            guard.clone(),
            vec![ParameterizedStatement {
                sql: "INSERT INTO public.bridge_metadata_fixture(id,value) VALUES(1,$1)".into(),
                parameters: vec![Some("kept".into())],
                expected_affected_rows: Some(1),
            }],
        )
        .unwrap();
    let accepted = completed(terminal(&mut bridge, id));
    assert_eq!(accepted.transaction_state, ServerTransactionState::Active);
    assert_eq!(accepted.key.session, initial.key.session);
    completed(execute(
        &mut bridge,
        "ALTER TABLE bridge_metadata_fixture ADD COLUMN added INTEGER",
    ));
    let id = bridge
        .apply_guarded_batch(
            guard,
            vec![ParameterizedStatement {
                sql: "UPDATE public.bridge_metadata_fixture SET value=$1 WHERE id=1".into(),
                parameters: vec![Some("must-not-write".into())],
                expected_affected_rows: Some(1),
            }],
        )
        .unwrap();
    let rejected = terminal(&mut bridge, id);
    assert_eq!(rejected.outcome, Outcome::Failed);
    assert_eq!(rejected.transaction_state, ServerTransactionState::Active);
    assert_eq!(rejected.key.session, initial.key.session);
    assert!(!rejected.connection_retired);
    let error = rejected
        .terminal_error
        .as_ref()
        .expect("metadata mismatch diagnostic");
    assert!(error.message.starts_with("TableMetadataChanged:"));
    assert_eq!(error.sqlstate, None);
    // The successful prior edit, subsequent DDL and temporary session context survive.
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT value FROM bridge_metadata_fixture WHERE id=1"
        ))),
        "kept"
    );
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT count(*) FROM information_schema.columns WHERE table_schema='public' AND table_name='bridge_metadata_fixture' AND column_name='added'"
        ))),
        "1"
    );
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT v FROM bridge_metadata_session"
        ))),
        "35"
    );
    let id = bridge.rollback().unwrap();
    completed(terminal(&mut bridge, id));
    bridge.set_commit_policy(CommitPolicy::Auto).unwrap();
    next(&mut bridge);
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT count(*) FROM bridge_metadata_fixture"
        ))),
        "0"
    );
    completed(execute(&mut bridge, "DROP TABLE bridge_metadata_fixture"));
}

fn column<'a>(objects: &'a [SchemaObject], table: &str, name: &str) -> &'a SchemaColumn {
    objects
        .iter()
        .find(|o| o.name == table)
        .unwrap_or_else(|| panic!("missing object {table}"))
        .columns
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("missing column {table}.{name}"))
}
#[test]
fn sqlite_basic_column_metadata_and_primary_key_nullability() {
    let file = File::new();
    let mut bridge = DatabaseBridge::new().unwrap();
    configure(&mut bridge, file.request());
    completed(execute(
        &mut bridge,
        r#"CREATE TABLE metadata_parent(id INTEGER PRIMARY KEY);
        CREATE TABLE metadata_fields(id INTEGER PRIMARY KEY, "Parent_Id" INTEGER,
            label TEXT NOT NULL DEFAULT 'seed', calc INTEGER GENERATED ALWAYS AS("Parent_Id"+1) STORED,
            FOREIGN KEY(parent_id) REFERENCES metadata_parent(id));
        CREATE VIEW metadata_view AS SELECT id,label FROM metadata_fields;
        CREATE TABLE nullable_text(v TEXT PRIMARY KEY);
        CREATE TABLE nullable_composite(a TEXT,b INTEGER,PRIMARY KEY(b,a));
        CREATE TABLE nullable_desc(v INTEGER PRIMARY KEY DESC);
        CREATE TABLE alias_pk(v INTEGER PRIMARY KEY);
        CREATE TABLE without_rowid_pk(v TEXT PRIMARY KEY) WITHOUT ROWID;
        CREATE TABLE strict_pk(v TEXT PRIMARY KEY) STRICT;
        CREATE TABLE sqliteuser(v INTEGER)"#,
    ));
    let (objects, truncated) = schema(&mut bridge);
    assert!(!truncated);
    let id = column(&objects, "metadata_fields", "id");
    assert_eq!(id.primary_key_ordinal, Some(1));
    assert!(!id.nullable);
    assert_eq!(id.extra.as_deref(), Some("rowid alias"));
    let fk = column(&objects, "metadata_fields", "Parent_Id");
    assert!(fk.foreign_key);
    assert!(fk.nullable);
    assert_eq!(fk.primary_key_ordinal, None);
    let label = column(&objects, "metadata_fields", "label");
    assert_eq!(label.default.as_deref(), Some("'seed'"));
    assert!(!label.nullable);
    assert_eq!(label.extra, None);
    assert_eq!(
        column(&objects, "metadata_fields", "calc").extra.as_deref(),
        Some("generated stored")
    );
    assert_eq!(
        objects
            .iter()
            .find(|o| o.name == "metadata_view")
            .unwrap()
            .kind,
        "view"
    );
    assert_eq!(
        objects
            .iter()
            .find(|o| o.name == "metadata_fields")
            .unwrap()
            .kind,
        "table"
    );
    let text = column(&objects, "nullable_text", "v");
    assert_eq!(text.primary_key_ordinal, Some(1));
    assert!(text.nullable);
    let a = column(&objects, "nullable_composite", "a");
    assert_eq!(a.primary_key_ordinal, Some(2));
    assert!(a.nullable);
    let b = column(&objects, "nullable_composite", "b");
    assert_eq!(b.primary_key_ordinal, Some(1));
    assert!(b.nullable);
    let desc = column(&objects, "nullable_desc", "v");
    assert!(desc.nullable);
    assert_eq!(desc.primary_key_ordinal, Some(1));
    assert_eq!(desc.extra, None);
    assert!(!column(&objects, "alias_pk", "v").nullable);
    assert!(!column(&objects, "without_rowid_pk", "v").nullable);
    assert!(!column(&objects, "strict_pk", "v").nullable);
    assert!(
        objects.iter().any(|o| o.name == "sqliteuser"),
        "only actual reserved sqlite_ prefix is excluded"
    );
    // Native SQLite allows NULL keys for these rowid-table declarations.
    completed(execute(
        &mut bridge,
        "INSERT INTO nullable_text VALUES(NULL); INSERT INTO nullable_composite VALUES(NULL,NULL); INSERT INTO nullable_desc VALUES(NULL)",
    ));
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT count(*) FROM nullable_text WHERE v IS NULL"
        ))),
        "1"
    );
}
fn network_column_metadata(engine: DatabaseEngine) {
    let mut bridge = DatabaseBridge::new().unwrap();
    configure(&mut bridge, fixture(engine));
    let drop = "DROP VIEW IF EXISTS bridge_meta_view; DROP TABLE IF EXISTS bridge_meta_columns; DROP TABLE IF EXISTS bridge_meta_composite; DROP TABLE IF EXISTS bridge_meta_parent";
    completed(execute(&mut bridge, drop));
    let create = match engine {
        DatabaseEngine::PostgreSql => {
            "CREATE TABLE bridge_meta_parent(id INTEGER PRIMARY KEY); CREATE TABLE bridge_meta_columns(id INTEGER GENERATED ALWAYS AS IDENTITY, parent_id INTEGER REFERENCES bridge_meta_parent(id), label TEXT NOT NULL DEFAULT 'seed', calc INTEGER GENERATED ALWAYS AS(parent_id+1) STORED, included TEXT, PRIMARY KEY(id) INCLUDE(included)); CREATE TABLE bridge_meta_composite(a INTEGER,b INTEGER,PRIMARY KEY(b,a)); CREATE VIEW bridge_meta_view AS SELECT id,label FROM bridge_meta_columns"
        }
        DatabaseEngine::MySql => {
            "CREATE TABLE bridge_meta_parent(id INTEGER PRIMARY KEY); CREATE TABLE bridge_meta_columns(id INTEGER AUTO_INCREMENT PRIMARY KEY, parent_id INTEGER, label VARCHAR(30) NOT NULL DEFAULT 'seed', calc INTEGER GENERATED ALWAYS AS(parent_id+1) STORED, included TEXT, FOREIGN KEY(parent_id) REFERENCES bridge_meta_parent(id)); CREATE TABLE bridge_meta_composite(a INTEGER,b INTEGER,PRIMARY KEY(b,a)); CREATE VIEW bridge_meta_view AS SELECT id,label FROM bridge_meta_columns"
        }
        DatabaseEngine::Sqlite => unreachable!(),
    };
    completed(execute(&mut bridge, create));
    let (objects, truncated) = schema(&mut bridge);
    assert!(!truncated);
    let id = column(&objects, "bridge_meta_columns", "id");
    assert_eq!(id.primary_key_ordinal, Some(1));
    assert!(!id.nullable);
    assert_eq!(
        id.extra.as_deref(),
        Some(if engine == DatabaseEngine::PostgreSql {
            "identity always"
        } else {
            "auto_increment"
        })
    );
    let fk = column(&objects, "bridge_meta_columns", "parent_id");
    assert!(fk.foreign_key);
    assert!(fk.nullable);
    let label = column(&objects, "bridge_meta_columns", "label");
    assert!(!label.nullable);
    assert!(
        label
            .default
            .as_deref()
            .is_some_and(|value| value.contains("seed"))
    );
    assert!(
        column(&objects, "bridge_meta_columns", "calc")
            .extra
            .as_deref()
            .is_some_and(|value| value.to_lowercase().contains("generated"))
    );
    assert_eq!(
        column(&objects, "bridge_meta_columns", "included").primary_key_ordinal,
        None
    );
    assert_eq!(
        column(&objects, "bridge_meta_composite", "b").primary_key_ordinal,
        Some(1)
    );
    assert_eq!(
        column(&objects, "bridge_meta_composite", "a").primary_key_ordinal,
        Some(2)
    );
    assert_eq!(
        objects
            .iter()
            .find(|o| o.name == "bridge_meta_view")
            .unwrap()
            .kind,
        "view"
    );
    assert_eq!(
        objects
            .iter()
            .find(|o| o.name == "bridge_meta_columns")
            .unwrap()
            .kind,
        "table"
    );
    completed(execute(&mut bridge, drop));
}
#[test]
#[ignore = "requires explicitly owned PostgreSQL fixture at 55439 and DBIDE_BRIDGE_FIXTURES=owned-loopback"]
fn postgres_basic_column_metadata() {
    network_column_metadata(DatabaseEngine::PostgreSql);
}
#[test]
#[ignore = "requires explicitly owned MySQL fixture at 55441 and DBIDE_BRIDGE_FIXTURES=owned-loopback"]
fn mysql_basic_column_metadata() {
    network_column_metadata(DatabaseEngine::MySql);
}

fn listed(bridge: &mut DatabaseBridge, refresh: bool) -> Vec<dbide_bridge::DatabaseInfo> {
    let id = if refresh {
        bridge.refresh_databases()
    } else {
        bridge.list_databases()
    }
    .unwrap();
    assert!(matches!(next(bridge),BridgeEvent::Connecting{operation,..} if operation==id));
    match next(bridge) {
        BridgeEvent::DatabasesLoaded {
            operation,
            databases,
            truncated,
        } => {
            assert_eq!(operation, id);
            assert!(!truncated);
            databases
        }
        event => panic!("unexpected discovery event {event:?}"),
    }
}
#[test]
fn sqlite_discovery_is_explicit_main_only_and_preserves_physical_session() {
    let mut bridge = DatabaseBridge::new().unwrap();
    let id = bridge.list_databases().unwrap();
    assert!(
        matches!(next(&mut bridge),BridgeEvent::Error{operation:Some(operation),..} if operation==id)
    );
    let file = File::new();
    configure(&mut bridge, file.request());
    let first = completed(execute(
        &mut bridge,
        "CREATE TABLE discovery_fixture(v INTEGER); CREATE TEMP TABLE discovery_temp(v INTEGER); INSERT INTO discovery_temp VALUES(41); ATTACH DATABASE ':memory:' AS private_attachment; CREATE TABLE private_attachment.kept(v INTEGER); INSERT INTO private_attachment.kept VALUES(42); BEGIN; INSERT INTO discovery_fixture VALUES(43)",
    ));
    assert_eq!(first.transaction_state, ServerTransactionState::Active);
    for refresh in [false, true] {
        assert_eq!(
            listed(&mut bridge, refresh),
            vec![dbide_bridge::DatabaseInfo {
                name: "main".into(),
                accessible: true,
                current: true
            }]
        );
    }
    let after = completed(execute(&mut bridge, "SELECT v FROM discovery_temp"));
    assert_eq!(after.key.session, first.key.session);
    assert_eq!(after.transaction_state, ServerTransactionState::Active);
    assert_eq!(scalar(&after), "41");
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT v FROM private_attachment.kept"
        ))),
        "42"
    );
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT v FROM discovery_fixture"
        ))),
        "43"
    );
    let id = bridge.rollback().unwrap();
    completed(terminal(&mut bridge, id));
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT count(*) FROM discovery_fixture"
        ))),
        "0"
    );
}
fn network_discovery(engine: DatabaseEngine) {
    let request = fixture(engine);
    let expected = request.profile.database.clone();
    let mut bridge = DatabaseBridge::new().unwrap();
    configure(&mut bridge, request);
    completed(execute(
        &mut bridge,
        "DROP TABLE IF EXISTS bridge_discovery_fixture; CREATE TABLE bridge_discovery_fixture(v INTEGER); CREATE TEMPORARY TABLE bridge_discovery_temp(v INTEGER); INSERT INTO bridge_discovery_temp VALUES(47)",
    ));
    let first = completed(execute(
        &mut bridge,
        "BEGIN; INSERT INTO bridge_discovery_fixture VALUES(48)",
    ));
    assert_eq!(first.transaction_state, ServerTransactionState::Active);
    let databases = listed(&mut bridge, false);
    assert!(
        databases
            .iter()
            .any(|d| d.name == expected && d.current && d.accessible)
    );
    assert_eq!(databases.iter().filter(|d| d.current).count(), 1);
    assert!(databases.len() <= 256);
    if engine == DatabaseEngine::PostgreSql {
        assert!(
            !databases
                .iter()
                .any(|d| d.name == "template0" || d.name == "template1")
        );
    }
    if engine == DatabaseEngine::MySql {
        assert_eq!(
            scalar(&completed(execute(&mut bridge, "SELECT ROW_COUNT()"))),
            "1",
            "sidecar discovery must not change user SQL diagnostics"
        );
    }
    let after = completed(execute(&mut bridge, "SELECT v FROM bridge_discovery_temp"));
    assert_eq!(after.key.session, first.key.session);
    assert_eq!(after.transaction_state, ServerTransactionState::Active);
    assert_eq!(scalar(&after), "47");
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT v FROM bridge_discovery_fixture"
        ))),
        "48"
    );
    let id = bridge.rollback().unwrap();
    completed(terminal(&mut bridge, id));
    assert_eq!(
        scalar(&completed(execute(
            &mut bridge,
            "SELECT count(*) FROM bridge_discovery_fixture"
        ))),
        "0"
    );
    completed(execute(&mut bridge, "DROP TABLE bridge_discovery_fixture"));
}
#[test]
#[ignore = "requires explicitly owned PostgreSQL fixture at 55439 and DBIDE_BRIDGE_FIXTURES=owned-loopback"]
fn postgres_database_discovery_preserves_transaction_and_temp() {
    network_discovery(DatabaseEngine::PostgreSql);
}
#[test]
#[ignore = "requires explicitly owned MySQL fixture at 55441 and DBIDE_BRIDGE_FIXTURES=owned-loopback"]
fn mysql_database_discovery_preserves_transaction_and_diagnostics() {
    network_discovery(DatabaseEngine::MySql);
}
