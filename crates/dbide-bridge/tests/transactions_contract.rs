use dbide_bridge::{
    BridgeEvent, CommitPolicy, DatabaseBridge, ExecutionReport, OperationId, Outcome,
    ServerTransactionState as Tx,
};
use dbide_profiles::{ConnectionProfile, ConnectionRequest};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
fn request() -> ConnectionRequest {
    ConnectionRequest {
        profile: ConnectionProfile::fixture(),
        password: "dbide_fixture_only".into(),
    }
}
fn next(bridge: &mut DatabaseBridge) -> BridgeEvent {
    let start = Instant::now();
    loop {
        if let Some(e) = bridge.try_next() {
            return e;
        }
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn finish(bridge: &mut DatabaseBridge, id: OperationId) -> ExecutionReport {
    loop {
        match next(bridge) {
            BridgeEvent::Connecting { operation, .. }
            | BridgeEvent::Connected { operation }
            | BridgeEvent::Executing { operation, .. } => assert_eq!(operation, id),
            BridgeEvent::Finished { operation, report } => {
                assert_eq!(operation, id);
                return report;
            }
            e => panic!("{e:?}"),
        }
    }
}
fn execute(bridge: &mut DatabaseBridge, sql: &str) -> ExecutionReport {
    let id = bridge.execute(sql.into()).unwrap();
    finish(bridge, id)
}
fn policy(bridge: &mut DatabaseBridge, policy: CommitPolicy, state: Tx) {
    let id = bridge.set_commit_policy(policy).unwrap();
    assert!(
        matches!(next(bridge),BridgeEvent::CommitPolicyChanged{operation,policy:p,transaction_state} if operation==id && p==policy && transaction_state==state)
    );
}
fn scalar(report: &ExecutionReport) -> String {
    report
        .events
        .iter()
        .find_map(|e| match e {
            dbide_pg_spike::Event::Row { values } => values.first().cloned().flatten(),
            _ => None,
        })
        .unwrap()
}
#[test]
#[ignore = "requires project-owned isolated PostgreSQL fixture at127.0.0.1:55439"]
fn manual_policy_real_wire_state_controls_and_retirement() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let observer = runtime
        .block_on(dbide_pg_spike::Session::connect_request(
            &request(),
            Default::default(),
        ))
        .unwrap();
    let observe = |sql: &str| {
        runtime
            .block_on(observer.execute(sql, CommitPolicy::Auto))
            .unwrap()
    };
    let mut bridge = DatabaseBridge::new().unwrap();
    bridge.configure(request()).unwrap();
    assert!(matches!(next(&mut bridge), BridgeEvent::Configured { .. }));
    let table = format!(
        "dbide_tx_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    assert_eq!(
        execute(&mut bridge, &format!("CREATE TABLE {table}(v integer)")).transaction_state,
        Tx::Idle
    );
    assert_eq!(
        execute(
            &mut bridge,
            "CREATE TEMP TABLE tx_preserved(v text); INSERT INTO tx_preserved VALUES('kept')"
        )
        .outcome,
        Outcome::Completed
    );
    policy(&mut bridge, CommitPolicy::Manual, Tx::Idle);
    let inserted = execute(&mut bridge, &format!("INSERT INTO {table} VALUES(1)"));
    assert_eq!(inserted.outcome, Outcome::Completed);
    assert_eq!(inserted.transaction_state, Tx::Active);
    assert_eq!(inserted.commit_policy, CommitPolicy::Manual);
    assert_eq!(
        scalar(&observe(&format!("SELECT count(*) FROM {table}"))),
        "0"
    );
    let mut rejected = request();
    rejected.profile.port = 55438;
    let changed = bridge.configure(rejected.clone()).unwrap();
    assert!(
        matches!(next(&mut bridge),BridgeEvent::Error{operation:Some(id),message} if id==changed && message.contains("transaction was preserved"))
    );
    assert_eq!(
        scalar(&execute(&mut bridge, "SELECT v FROM tx_preserved")),
        "kept"
    );
    policy(&mut bridge, CommitPolicy::Auto, Tx::Active);
    assert_eq!(
        scalar(&observe(&format!("SELECT count(*) FROM {table}"))),
        "0",
        "changing policy committed a transaction"
    );
    let id = bridge.commit().unwrap();
    let committed = finish(&mut bridge, id);
    assert_eq!(committed.outcome, Outcome::Completed);
    assert_eq!(committed.transaction_state, Tx::Idle);
    assert_eq!(
        scalar(&observe(&format!("SELECT count(*) FROM {table}"))),
        "1"
    );
    policy(&mut bridge, CommitPolicy::Manual, Tx::Idle);
    assert_eq!(
        execute(&mut bridge, &format!("INSERT INTO {table} VALUES(2)")).transaction_state,
        Tx::Active
    );
    let id = bridge.rollback().unwrap();
    assert_eq!(finish(&mut bridge, id).transaction_state, Tx::Idle);
    assert_eq!(
        scalar(&observe(&format!("SELECT count(*) FROM {table}"))),
        "1"
    );
    assert_eq!(
        execute(&mut bridge, "SELECT 42").transaction_state,
        Tx::Active,
        "next Manual execution failed to begin a fresh transaction"
    );
    let failed = execute(&mut bridge, "SELECT 1/0");
    assert_eq!(failed.outcome, Outcome::Failed);
    assert_eq!(failed.transaction_state, Tx::Failed);
    let changed = bridge.configure(rejected).unwrap();
    assert!(
        matches!(next(&mut bridge),BridgeEvent::Error{operation:Some(id),message} if id==changed && message.contains("transaction was preserved"))
    );
    let commit = bridge.commit().unwrap();
    assert!(
        matches!(next(&mut bridge),BridgeEvent::Error{operation:Some(id),message} if id==commit && message.contains("Rollback"))
    );
    assert_eq!(
        execute(&mut bridge, "SELECT 1").transaction_state,
        Tx::Failed
    );
    let id = bridge.rollback().unwrap();
    assert_eq!(finish(&mut bridge, id).transaction_state, Tx::Idle);
    policy(&mut bridge, CommitPolicy::Auto, Tx::Idle);
    assert_eq!(execute(&mut bridge, "BEGIN").transaction_state, Tx::Active);
    assert_eq!(
        execute(&mut bridge, &format!("INSERT INTO {table} VALUES(3)")).transaction_state,
        Tx::Active
    );
    policy(&mut bridge, CommitPolicy::Manual, Tx::Active);
    assert_eq!(
        scalar(&observe(&format!("SELECT count(*) FROM {table}"))),
        "1",
        "Auto to Manual committed an existing transaction"
    );
    policy(&mut bridge, CommitPolicy::Auto, Tx::Active);
    assert_eq!(execute(&mut bridge, "COMMIT").transaction_state, Tx::Idle);
    policy(&mut bridge, CommitPolicy::Manual, Tx::Idle);
    let pid = scalar(&execute(&mut bridge, "SELECT pg_backend_pid()"));
    let slow = bridge.execute("SELECT pg_sleep(30)".into()).unwrap();
    assert!(matches!(next(&mut bridge),BridgeEvent::Executing{operation,..} if operation==slow));
    let started = Instant::now();
    loop {
        if scalar(&observe(&format!(
            "SELECT count(*) FROM pg_stat_activity WHERE pid={pid} AND wait_event='PgSleep'"
        ))) == "1"
        {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(2));
    }
    let cancel = bridge.cancel(slow).unwrap();
    assert!(matches!(next(&mut bridge),BridgeEvent::Cancelling{operation,..} if operation==cancel));
    match next(&mut bridge) {
        BridgeEvent::Finished { operation, report } => {
            assert_eq!(operation, slow);
            assert_eq!(report.outcome, Outcome::Cancelled);
            assert!(report.connection_retired);
            assert_eq!(report.transaction_state, Tx::Unknown);
        }
        e => panic!("{e:?}"),
    }
    let rollback = bridge.rollback().unwrap();
    assert!(matches!(next(&mut bridge),BridgeEvent::Error{operation:Some(id),..} if id==rollback));
    assert_eq!(
        execute(&mut bridge, "SELECT 1").transaction_state,
        Tx::Active
    );
    let id = bridge.rollback().unwrap();
    assert_eq!(finish(&mut bridge, id).transaction_state, Tx::Idle);
    policy(&mut bridge, CommitPolicy::Auto, Tx::Idle);
    // A transport failure reports Unknown; Commit must never reconnect into a different transaction.
    let report = execute(&mut bridge, "SELECT pg_terminate_backend(pg_backend_pid())");
    assert_eq!(report.outcome, Outcome::OutcomeUnknown);
    assert_eq!(report.transaction_state, Tx::Unknown);
    let commit = bridge.commit().unwrap();
    assert!(matches!(next(&mut bridge),BridgeEvent::Error{operation:Some(id),..} if id==commit));
    assert_eq!(
        observe(&format!("DROP TABLE {table}")).outcome,
        Outcome::Completed
    );
    observer.close();
}
