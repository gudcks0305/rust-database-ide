use dbide_bridge::{BridgeEvent, ConnectionErrorKind, DatabaseBridge, ExecutionReport, Outcome};
use dbide_profiles::{ConnectionProfile, ConnectionRequest, TlsMode};
use std::time::{Duration, Instant};
fn request() -> ConnectionRequest {
    ConnectionRequest {
        profile: ConnectionProfile::fixture(),
        password: "dbide_fixture_only".into(),
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
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn configure(bridge: &mut DatabaseBridge, request: ConnectionRequest) {
    let op = bridge.configure(request).unwrap();
    assert!(matches!(next(bridge),BridgeEvent::Configured{operation} if operation==op));
}
fn execute(bridge: &mut DatabaseBridge, sql: &str) -> ExecutionReport {
    let op = bridge.execute(sql.into()).unwrap();
    loop {
        match next(bridge) {
            BridgeEvent::Connecting {
                operation, attempt, ..
            } => {
                assert_eq!(operation, op);
                assert_eq!(attempt, 1);
            }
            BridgeEvent::Connected { operation } | BridgeEvent::Executing { operation, .. } => {
                assert_eq!(operation, op)
            }
            BridgeEvent::Finished { operation, report } => {
                assert_eq!(operation, op);
                return report;
            }
            event => panic!("unexpected event {event:?}"),
        }
    }
}
fn scalar(report: &ExecutionReport) -> String {
    report
        .events
        .iter()
        .find_map(|event| match event {
            dbide_pg_spike::Event::Row { values } => values.first().cloned().flatten(),
            _ => None,
        })
        .unwrap()
}
#[test]
fn unconfigured_run_and_invalid_profile_have_terminal_feedback() {
    let mut bridge = DatabaseBridge::new().unwrap();
    let op = bridge.execute("SELECT 1".into()).unwrap();
    assert!(matches!(next(&mut bridge),BridgeEvent::Error{operation:Some(id),..} if id==op));
    let mut bad = request();
    bad.profile.host = "postgresql://password@example.com".into();
    bridge.configure(bad).unwrap();
    assert!(
        matches!(next(&mut bridge),BridgeEvent::ConnectionFailed{diagnostic,..} if diagnostic.kind==ConnectionErrorKind::Configuration)
    );
}
#[test]
#[ignore = "requires explicitly provisioned project-owned loopback PostgreSQL fixture at55439"]
fn configured_connections_retry_feedback_and_session_isolation() {
    let mut bridge = DatabaseBridge::new().unwrap();
    let mut bad = request();
    bad.profile.port = 55438;
    configure(&mut bridge, bad);
    for _ in 0..2 {
        let op = bridge.execute("SELECT 1".into()).unwrap();
        assert!(
            matches!(next(&mut bridge),BridgeEvent::Connecting{operation,attempt:1,..} if operation==op)
        );
        match next(&mut bridge) {
            BridgeEvent::ConnectionFailed {
                operation,
                diagnostic,
            } => {
                assert_eq!(operation, op);
                assert_eq!(diagnostic.kind, ConnectionErrorKind::Refused);
                assert_eq!(diagnostic.target.port, 55438);
                assert!(diagnostic.os_code.is_some());
                assert!(!format!("{diagnostic:?}").contains("dbide_fixture_only"));
            }
            event => panic!("{event:?}"),
        }
    }
    let mut wrong = request();
    wrong.password = "never-show-this-secret".into();
    configure(&mut bridge, wrong);
    bridge.connect().unwrap();
    assert!(matches!(next(&mut bridge), BridgeEvent::Connecting { .. }));
    match next(&mut bridge) {
        BridgeEvent::ConnectionFailed { diagnostic, .. } => {
            assert_eq!(diagnostic.kind, ConnectionErrorKind::Authentication);
            assert_eq!(diagnostic.sqlstate.as_deref(), Some("28P01"));
            assert!(!format!("{diagnostic:?}").contains("never-show-this-secret"));
        }
        event => panic!("{event:?}"),
    }
    let mut missing = request();
    missing.profile.database = "dbide_fixture_missing_database".into();
    configure(&mut bridge, missing);
    bridge.connect().unwrap();
    next(&mut bridge);
    assert!(
        matches!(next(&mut bridge),BridgeEvent::ConnectionFailed{diagnostic,..} if diagnostic.kind==ConnectionErrorKind::DatabaseMissing)
    );
    configure(&mut bridge, request());
    let report = execute(
        &mut bridge,
        "CREATE TEMP TABLE preserved(v integer); INSERT INTO preserved VALUES(7); SELECT pg_backend_pid()",
    );
    assert_eq!(report.outcome, Outcome::Completed);
    let old_pid = scalar(&report);
    bridge.test_connection(request()).unwrap();
    assert!(matches!(next(&mut bridge), BridgeEvent::Connecting { .. }));
    assert!(matches!(
        next(&mut bridge),
        BridgeEvent::ConnectionTested { .. }
    ));
    assert_eq!(
        scalar(&execute(&mut bridge, "SELECT v FROM preserved")),
        "7"
    );
    bridge.inspect_schema().unwrap();
    assert!(matches!(next(&mut bridge), BridgeEvent::Connecting { .. }));
    assert!(matches!(
        next(&mut bridge),
        BridgeEvent::SchemaLoaded { .. }
    ));
    assert_eq!(
        scalar(&execute(&mut bridge, "SELECT pg_backend_pid()")),
        old_pid
    );
    let slow = bridge.execute("SELECT pg_sleep(0.05)".into()).unwrap();
    bridge.configure(request()).unwrap();
    assert!(matches!(next(&mut bridge),BridgeEvent::Executing{operation,..} if operation==slow));
    assert!(matches!(next(&mut bridge), BridgeEvent::Error { .. }));
    assert!(matches!(next(&mut bridge), BridgeEvent::Finished { .. }));
    configure(&mut bridge, request());
    let new_pid = scalar(&execute(&mut bridge, "SELECT pg_backend_pid()"));
    assert_ne!(old_pid, new_pid);
    assert_eq!(
        execute(&mut bridge, "SELECT * FROM preserved").outcome,
        Outcome::Failed
    );
    bridge.disconnect().unwrap();
    assert!(matches!(
        next(&mut bridge),
        BridgeEvent::Disconnected { .. }
    ));
    assert_eq!(scalar(&execute(&mut bridge, "SELECT 42")), "42");
    let old_pid = scalar(&execute(&mut bridge, "SELECT pg_backend_pid()"));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let observer = dbide_pg_spike::Session::connect_request(&request(), Default::default())
            .await
            .unwrap();
        observer
            .execute(
                &format!("SELECT pg_terminate_backend({old_pid})"),
                dbide_pg_spike::CommitPolicy::Auto,
            )
            .await
            .unwrap();
        observer.close();
    });
    std::thread::sleep(Duration::from_millis(20));
    assert_ne!(
        scalar(&execute(&mut bridge, "SELECT pg_backend_pid()")),
        old_pid
    );
    // A post-dispatch error cannot replay the SQL; a later user Run is a new request.
    let report = execute(&mut bridge, "SELECT 1/0");
    assert_eq!(report.outcome, Outcome::Failed);
    assert_eq!(scalar(&execute(&mut bridge, "SELECT 9")), "9");
    let report = execute(&mut bridge, "SELECT pg_terminate_backend(pg_backend_pid())");
    assert_eq!(report.outcome, Outcome::OutcomeUnknown);
    assert!(report.connection_retired);
    assert_eq!(scalar(&execute(&mut bridge, "SELECT 10")), "10");
}
#[test]
#[ignore = "requires project-owned TLS PostgreSQL fixture55440 and DBIDE_TEST_TLS_CA explicit PEM path"]
fn verified_tls_no_downgrade_and_custom_ca() {
    let ca = std::env::var("DBIDE_TEST_TLS_CA")
        .expect("DBIDE_TEST_TLS_CA must explicitly name fixture PEM CA");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut p = request();
        p.profile.tls_mode = TlsMode::VerifyFull;
        let err = dbide_pg_spike::Session::connect_request(&p, Default::default())
            .await
            .err()
            .expect("plaintext server must not downgrade");
        assert_eq!(err.kind, ConnectionErrorKind::Tls);
        p.profile.host = "localhost".into();
        p.profile.port = 55440;
        for mode in [TlsMode::VerifyFull, TlsMode::Require] {
            p.profile.tls_mode = mode;
            p.profile.root_cert_path = None;
            let err = dbide_pg_spike::Session::connect_request(&p, Default::default())
                .await
                .err()
                .expect("untrusted certificate must fail");
            assert_eq!(err.kind, ConnectionErrorKind::Tls);
            p.profile.root_cert_path = Some(ca.clone());
            let session = dbide_pg_spike::Session::connect_request(&p, Default::default())
                .await
                .expect("explicit fixtureCA should verify");
            let report = session
                .execute(
                    "SELECT ssl FROM pg_stat_ssl WHERE pid=pg_backend_pid()",
                    dbide_pg_spike::CommitPolicy::Auto,
                )
                .await
                .unwrap();
            assert_eq!(scalar(&report), "t");
            session.close();
        }
    });
}

#[test]
fn pending_connection_cancel_and_disconnect_are_prompt_and_targeted() {
    use std::{io::Read, net::TcpListener, sync::mpsc};
    for disconnect in [true, false] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (accepted_tx, accepted) = mpsc::channel();
        let (bytes_tx, bytes) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            accepted_tx.send(()).unwrap();
            let mut received = vec![];
            let _ = socket.read_to_end(&mut received);
            bytes_tx.send(received).unwrap();
        });
        let mut bridge = DatabaseBridge::new().unwrap();
        let mut profile = request();
        profile.profile.port = port;
        profile.profile.connect_timeout_secs = 30;
        configure(&mut bridge, profile);
        let pending = bridge
            .execute("SELECT 'blackhole_sql_must_never_dispatch'".into())
            .unwrap();
        assert!(
            matches!(next(&mut bridge),BridgeEvent::Connecting{operation,..} if operation==pending)
        );
        accepted
            .recv_timeout(Duration::from_secs(2))
            .expect("owned blackhole accepted transport");
        let started = Instant::now();
        let action = if disconnect {
            bridge.disconnect().unwrap()
        } else {
            bridge.cancel(pending).unwrap()
        };
        assert!(
            matches!(next(&mut bridge),BridgeEvent::Error{operation:Some(id),message} if id==pending && message.contains("SQL was not submitted"))
        );
        let ack = next(&mut bridge);
        if disconnect {
            assert!(matches!(ack,BridgeEvent::Disconnected{operation:Some(id)} if id==action));
        } else {
            assert!(matches!(ack,BridgeEvent::Error{operation:Some(id),..} if id==action));
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "pending connection cancellation waited for30s timeout"
        );
        let received = bytes
            .recv_timeout(Duration::from_secs(2))
            .expect("cancelled connect socket closed");
        assert!(!String::from_utf8_lossy(&received).contains("blackhole_sql_must_never_dispatch"));
        server.join().unwrap();
        // A stale cancel must not poison a new attempt, including cancel-before-handler.
        configure(&mut bridge, request());
        let stale = bridge.cancel(pending).unwrap();
        assert!(matches!(next(&mut bridge),BridgeEvent::Error{operation:Some(id),..} if id==stale));
    }
    // A stale target arriving while a NEW connection waits must not interrupt it.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (accepted_tx, accepted) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        accepted_tx.send(()).unwrap();
        let mut received = vec![];
        let _ = socket.read_to_end(&mut received);
        assert!(!String::from_utf8_lossy(&received).contains("stale_cancel_new_attempt"));
    });
    let mut bridge = DatabaseBridge::new().unwrap();
    let mut profile = request();
    profile.profile.port = port;
    configure(&mut bridge, profile);
    let current = bridge
        .execute("SELECT 'stale_cancel_new_attempt'".into())
        .unwrap();
    assert!(
        matches!(next(&mut bridge),BridgeEvent::Connecting{operation,..} if operation==current)
    );
    accepted.recv_timeout(Duration::from_secs(2)).unwrap();
    let stale = bridge.cancel(dbide_bridge::OperationId(0)).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    assert!(
        bridge.try_next().is_none(),
        "stale cancel interrupted the new attempt"
    );
    let cancel = bridge.cancel(current).unwrap();
    assert!(matches!(next(&mut bridge),BridgeEvent::Error{operation:Some(id),..} if id==current));
    assert!(matches!(next(&mut bridge),BridgeEvent::Error{operation:Some(id),..} if id==stale));
    assert!(matches!(next(&mut bridge),BridgeEvent::Error{operation:Some(id),..} if id==cancel));
    server.join().unwrap();
    // No fixture needed: repeated cancel-before-handler releases every event reservation.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut bridge = DatabaseBridge::new().unwrap();
    let mut profile = request();
    profile.profile.port = listener.local_addr().unwrap().port();
    configure(&mut bridge, profile);
    for _ in 0..12 {
        let pending = bridge.execute("SELECT 1".into()).unwrap();
        bridge.cancel(pending).unwrap();
        assert!(matches!(next(&mut bridge), BridgeEvent::Connecting { .. }));
        assert!(
            matches!(next(&mut bridge),BridgeEvent::Error{operation:Some(id),..} if id==pending)
        );
        assert!(matches!(next(&mut bridge), BridgeEvent::Error { .. }));
    }
}
