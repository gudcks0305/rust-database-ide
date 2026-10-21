//! Owned-loopback, read-only validation of the table reader's separate bridge.
//! Explicit BEGIN/ROLLBACK exercise transaction ownership; no DDL or row writes.
use dbide_bridge::{
    BridgeEvent, CommitPolicy, DatabaseBridge, OperationId, ServerTransactionState as Tx,
};
use dbide_pg_spike::{Event, ExecutionReport, Outcome};
use dbide_profiles::{ConnectionProfile, ConnectionRequest, StoredExecutionTarget};
use dbide_ui_spike::{
    console_session::{ConsoleEffect, ConsoleSession},
    navigator::{Key, Relation, Scope},
    table_open_model::PendingTableOpen,
    workbench_model::{ResultOwner, profile_target},
};
use std::time::{Duration, Instant};
fn request(database: &str) -> ConnectionRequest {
    assert!(matches!(database, "dbide_fixture" | "dbide_console_b"));
    let mut profile = ConnectionProfile::fixture();
    assert_eq!(profile.host, "127.0.0.1");
    assert_eq!(profile.port, 55439);
    profile.database = database.into();
    ConnectionRequest {
        profile,
        password: "dbide_fixture_only".into(),
    }
}
fn owner(console: &ConsoleSession) -> ResultOwner {
    let p = console.target().unwrap();
    ResultOwner {
        document_id: console.route().console_id,
        document_title: "unchanged-console.sql".into(),
        profile_id: p.id.clone(),
        profile_name: p.name.clone(),
        target: profile_target(p),
    }
}
fn console_result(console: &mut ConsoleSession) -> ExecutionReport {
    let started = Instant::now();
    let mut finished = None;
    loop {
        if let Some(effects) = console.poll_one()
            && let Some(effect) = effects.into_iter().next()
        {
            let ConsoleEffect::Finished { report, .. } = effect;
            assert_eq!(report.outcome, Outcome::Completed);
            finished = Some(report);
        }
        if !console.busy()
            && let Some(report) = finished.take()
        {
            return report;
        }
        assert!(
            started.elapsed() < Duration::from_secs(12),
            "owned console deadline"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn event(bridge: &mut DatabaseBridge) -> BridgeEvent {
    let started = Instant::now();
    loop {
        if let Some(event) = bridge.try_next() {
            return event;
        }
        assert!(
            started.elapsed() < Duration::from_secs(12),
            "owned table reader deadline"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn database(report: &ExecutionReport) -> &str {
    report
        .events
        .iter()
        .find_map(|event| match event {
            Event::Row { values } => values.first().and_then(Option::as_deref),
            _ => None,
        })
        .expect("database row")
}
fn query(bridge: &mut DatabaseBridge, sql: &str) -> ExecutionReport {
    let op = bridge.execute(sql.into()).unwrap();
    loop {
        match event(bridge) {
            BridgeEvent::Finished { operation, report } if operation == op => {
                assert_eq!(report.outcome, Outcome::Completed);
                return report;
            }
            BridgeEvent::Error { message, .. } => panic!("owned table query: {message}"),
            BridgeEvent::ConnectionFailed { diagnostic, .. } => {
                panic!("owned table query: {}", diagnostic.message)
            }
            _ => {}
        }
    }
}
#[test]
#[ignore = "requires DBIDE_BRIDGE_FIXTURES=owned-loopback and owned PG databases at127.0.0.1:55439"]
fn preparing_and_disconnecting_table_reader_never_rebinds_or_rolls_back_sql_console() {
    assert_eq!(
        std::env::var("DBIDE_BRIDGE_FIXTURES").as_deref(),
        Ok("owned-loopback")
    );
    let mut console = ConsoleSession::new(41);
    console
        .bind(console.route(), request("dbide_fixture"))
        .unwrap();
    let route = console.route();
    let original = console.stored_target().unwrap();
    console
        .execute(
            route,
            "BEGIN; SELECT current_database()".into(),
            owner(&console),
            true,
        )
        .unwrap();
    let report = console_result(&mut console);
    assert_eq!(report.transaction_state, Tx::Active);
    assert_eq!(database(&report), "dbide_fixture");
    let request = request("dbide_console_b");
    let identity =
        StoredExecutionTarget::capture(&request.profile, &request.profile.database).unwrap();
    let relation = Relation {
        schema: "public".into(),
        name: "unused_in_this_readonly_probe".into(),
        kind: "table".into(),
    };
    let key = Key {
        profile_id: request.profile.id.clone(),
        fingerprint: identity.endpoint_fingerprint,
        generation: 1,
        database: request.profile.database.clone(),
        scope: Scope::Columns(relation.clone()),
    };
    let mut opening = PendingTableOpen::new(key, request.clone(), relation).unwrap();
    let mut reader = DatabaseBridge::new().unwrap();
    let configured = reader.configure(request).unwrap();
    assert!(opening.configure(configured));
    loop {
        match event(&mut reader) {
            BridgeEvent::Configured { operation } if operation == configured => {
                let connect = reader.connect().unwrap();
                assert!(opening.connect(operation, connect));
            }
            BridgeEvent::Connected { operation } if opening.waiting_connected(operation) => {
                let observe = reader.set_commit_policy(CommitPolicy::Auto).unwrap();
                assert!(opening.observe(operation, observe));
            }
            BridgeEvent::CommitPolicyChanged {
                operation,
                transaction_state,
                ..
            } => {
                assert!(opening.observed(operation, transaction_state).unwrap());
                break;
            }
            BridgeEvent::Error { message, .. } => panic!("owned prepare: {message}"),
            BridgeEvent::ConnectionFailed { diagnostic, .. } => {
                panic!("owned prepare: {}", diagnostic.message)
            }
            _ => {}
        }
    }
    assert!(opening.ready_for(
        true,
        Some(&opening.request().profile),
        true,
        false,
        Tx::Idle
    ));
    let report = query(&mut reader, "SELECT current_database()");
    assert_eq!(database(&report), "dbide_console_b");
    assert_eq!(report.transaction_state, Tx::Idle);
    // Cancellation/Disconnect belongs to the reader only, even with an active A.
    opening.close();
    let disconnect = reader.disconnect().unwrap();
    loop {
        if matches!(event(&mut reader),BridgeEvent::Disconnected{operation:Some(operation)} if operation==disconnect)
        {
            break;
        }
    }
    assert!(!opening.owns(OperationId(1)));
    assert_eq!(console.route(), route);
    assert_eq!(console.stored_target().unwrap(), original);
    assert_eq!(console.transaction_state, Tx::Active);
    console
        .execute(
            route,
            "SELECT current_database()".into(),
            owner(&console),
            true,
        )
        .unwrap();
    let report = console_result(&mut console);
    assert_eq!(database(&report), "dbide_fixture");
    assert_eq!(report.transaction_state, Tx::Active);
    console.transaction(route, false, owner(&console)).unwrap();
    assert_eq!(console_result(&mut console).transaction_state, Tx::Idle);
}
