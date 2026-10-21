use dbide_bridge::{BridgeEvent, FixtureBridge, Outcome};
use std::time::{Duration, Instant};
fn next(bridge: &mut FixtureBridge) -> BridgeEvent {
    let started = Instant::now();
    loop {
        if let Some(event) = bridge.try_next() {
            return event;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "bridge event deadline"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn connect(bridge: &mut FixtureBridge) {
    let operation = bridge.connect().unwrap();
    assert!(matches!(next(bridge), BridgeEvent::Connected { operation: id } if id == operation));
}
fn execute(bridge: &mut FixtureBridge, sql: &str) -> dbide_bridge::ExecutionReport {
    let operation = bridge.execute(sql.into()).unwrap();
    assert!(
        matches!(next(bridge), BridgeEvent::Executing { operation: id, .. } if id == operation)
    );
    match next(bridge) {
        BridgeEvent::Finished {
            operation: id,
            report,
        } if id == operation => report,
        event => panic!("unexpected event: {event:?}"),
    }
}
fn pid(bridge: &mut FixtureBridge) -> String {
    execute(bridge, "SELECT pg_backend_pid()")
        .events
        .iter()
        .find_map(|event| match event {
            dbide_pg_spike::Event::Row { values } => values.first().cloned().flatten(),
            _ => None,
        })
        .unwrap()
}
fn backend_gone(pid: &str) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let observer = dbide_pg_spike::Session::connect("postgresql://postgres:dbide_fixture_only@127.0.0.1:55439/dbide_fixture", dbide_pg_spike::Limits::default()).await.unwrap();
        let start = Instant::now();
        loop {
            let report = observer.execute(&format!("SELECT count(*) FROM pg_stat_activity WHERE pid={pid}"), dbide_pg_spike::CommitPolicy::Auto).await.unwrap();
            let absent = report.events.iter().any(|event| matches!(event, dbide_pg_spike::Event::Row { values } if values == &vec![Some("0".into())]));
            if absent { break; }
            assert!(start.elapsed() < Duration::from_secs(3), "bridge lifecycle left live PostgreSQL backend");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
}
#[test]
#[ignore = "requires explicitly provisioned disposable loopback PostgreSQL fixture at port55439"]
fn fixture_bridge_exec_cancel_reconnect_disconnect_and_drop() {
    for _ in 0..3 {
        let mut bridge = FixtureBridge::new().unwrap();
        connect(&mut bridge);
        let cancel_pid = pid(&mut bridge);
        let temp = execute(
            &mut bridge,
            "CREATE TEMP TABLE bridge_pin(v text); INSERT INTO bridge_pin VALUES('bridge 한글'); SELECT v FROM bridge_pin",
        );
        assert_eq!(temp.outcome, Outcome::Completed);
        let next_query = bridge.execute("SELECT pg_sleep(0.05)".into()).unwrap();
        let stale_cancel = bridge.cancel(temp.key).unwrap();
        assert!(
            matches!(next(&mut bridge), BridgeEvent::Executing { operation, .. } if operation == next_query)
        );
        assert!(
            matches!(next(&mut bridge), BridgeEvent::Error { operation: Some(id), .. } if id == stale_cancel)
        );
        match next(&mut bridge) {
            BridgeEvent::Finished { operation, report } => {
                assert_eq!(operation, next_query);
                assert_eq!(report.outcome, Outcome::Completed);
                assert!(
                    !report.connection_retired,
                    "stale cancel retired the next execution"
                );
            }
            event => panic!("stale cancel hit next query: {event:?}"),
        }
        // Enqueue cancel immediately, before polling Executing. No scheduling sleep.
        let slow = bridge.execute("SELECT pg_sleep(30)".into()).unwrap();
        let overlapping = bridge.execute("SELECT 2".into()).unwrap();
        let cancel = bridge.cancel(slow).unwrap();
        assert!(
            matches!(next(&mut bridge), BridgeEvent::Executing { operation, .. } if operation == slow)
        );
        assert!(
            matches!(next(&mut bridge), BridgeEvent::Error { operation: Some(id), .. } if id == overlapping)
        );
        assert!(
            matches!(next(&mut bridge), BridgeEvent::Cancelling { operation, .. } if operation == cancel)
        );
        match next(&mut bridge) {
            BridgeEvent::Finished { operation, report } => {
                assert_eq!(operation, slow);
                assert_eq!(report.outcome, Outcome::Cancelled);
                assert!(report.connection_retired);
            }
            event => panic!("unexpected cancel completion: {event:?}"),
        }
        backend_gone(&cancel_pid);
        let rejected = bridge.execute("SELECT 1".into()).unwrap();
        assert!(
            matches!(next(&mut bridge), BridgeEvent::Error { operation: Some(id), .. } if id == rejected)
        );
        connect(&mut bridge);
        let drop_pid = pid(&mut bridge);
        let slow = bridge.execute("SELECT pg_sleep(30)".into()).unwrap();
        assert!(
            matches!(next(&mut bridge), BridgeEvent::Executing { operation, .. } if operation == slow)
        );
        let start = Instant::now();
        drop(bridge);
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "UI Drop blocked"
        );
        backend_gone(&drop_pid);
        let mut bridge = FixtureBridge::new().unwrap();
        connect(&mut bridge);
        let disconnect_pid = pid(&mut bridge);
        let slow = bridge.execute("SELECT pg_sleep(30)".into()).unwrap();
        // Enqueue disconnect before waiting for Executing, exercising native Queued.
        let disconnect = bridge.disconnect().unwrap();
        assert!(
            matches!(next(&mut bridge), BridgeEvent::Executing { operation, .. } if operation == slow)
        );
        assert!(
            matches!(next(&mut bridge), BridgeEvent::Finished { operation, .. } if operation == slow)
        );
        assert!(
            matches!(next(&mut bridge), BridgeEvent::Disconnected { operation: Some(id) } if id == disconnect)
        );
        backend_gone(&disconnect_pid);
    }
    println!(
        "bridge contract passed: 3 rounds immediate cancel/disconnect, overlap rejection, explicit reconnect, nonblocking Drop, and backend disappearance for all9 lifecycles"
    );
}
