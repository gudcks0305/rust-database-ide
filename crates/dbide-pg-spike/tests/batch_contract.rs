use dbide_pg_spike::{
    CommitPolicy as Policy, Event, Limits, Outcome, ParameterizedStatement as Statement,
    ServerTransactionState as Tx, Session,
};
use dbide_profiles::{ConnectionProfile, ConnectionRequest};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
fn request() -> ConnectionRequest {
    ConnectionRequest {
        profile: ConnectionProfile::fixture(),
        password: "dbide_fixture_only".into(),
    }
}
fn statement(sql: String, parameters: Vec<Option<String>>, expected: u64) -> Statement {
    Statement {
        sql,
        parameters,
        expected_affected_rows: Some(expected),
    }
}
fn value(s: &str) -> Option<String> {
    Some(s.into())
}
fn rows(report: &dbide_pg_spike::ExecutionReport) -> Vec<Vec<Option<String>>> {
    report
        .events
        .iter()
        .filter_map(|event| match event {
            Event::Row { values } => Some(values.clone()),
            _ => None,
        })
        .collect()
}
fn scalar(report: &dbide_pg_spike::ExecutionReport) -> String {
    rows(report)[0][0].clone().unwrap()
}
async fn sql(session: &Session, sql: &str) -> dbide_pg_spike::ExecutionReport {
    let report = session.execute(sql, Policy::Auto).await.unwrap();
    assert_eq!(report.outcome, Outcome::Completed, "{report:?}");
    report
}
fn name() -> String {
    format!(
        "dbide_batch_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}
#[tokio::test]
#[ignore = "requires project-owned isolated PostgreSQL fixture at127.0.0.1:55439"]
async fn parameterized_batches_are_atomic_exclusive_and_transaction_aware() {
    let session = Session::connect_request(&request(), Limits::default())
        .await
        .unwrap();
    let observer = Session::connect_request(&request(), Limits::default())
        .await
        .unwrap();
    let table = name();
    sql(
        &session,
        &format!(
            "CREATE TABLE {table}(id integer PRIMARY KEY,note text,amount numeric,maybe text)"
        ),
    )
    .await;
    let injection = "한글🙂'); DROP TABLE must_not_execute; --";
    let insert = |id: &str, note: &str| {
        statement(
            format!(
                "INSERT INTO {table}(id,note,amount,maybe) VALUES($1::text::integer,$2::text,$3::text::numeric,$4::text)"
            ),
            vec![value(id), value(note), value("1.2300"), None],
            1,
        )
    };
    let report = session
        .execute_batch(
            vec![insert("1", "original"), insert("2", injection)],
            Policy::Auto,
        )
        .await
        .unwrap();
    assert_eq!(report.outcome, Outcome::Completed);
    assert_eq!(report.transaction_state, Tx::Idle);
    assert_eq!(
        rows(
            &sql(
                &session,
                &format!("SELECT note,amount::text,maybe FROM {table} WHERE id=2")
            )
            .await
        ),
        vec![vec![value(injection), value("1.2300"), None]]
    );
    let update = |id: &str, new: &str, old: &str| {
        statement(
            format!(
                "UPDATE {table} SET note=$1::text WHERE id=$2::text::integer AND note IS NOT DISTINCT FROM $3::text"
            ),
            vec![value(new), value(id), value(old)],
            1,
        )
    };
    let conflict = session
        .execute_batch(
            vec![
                update("1", "changed", "original"),
                update("2", "changed", "missing"),
            ],
            Policy::Auto,
        )
        .await
        .unwrap();
    assert_eq!(conflict.outcome, Outcome::Failed);
    assert_eq!(conflict.transaction_state, Tx::Idle);
    assert!(
        conflict
            .terminal_error
            .as_ref()
            .unwrap()
            .message
            .starts_with("Conflict:")
    );
    assert!(conflict.terminal_error.as_ref().unwrap().sqlstate.is_none());
    assert_eq!(
        conflict
            .events
            .iter()
            .filter_map(|e| match e {
                Event::CommandComplete { affected_rows } => Some(*affected_rows),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![1, 0]
    );
    assert_eq!(
        scalar(&sql(&observer, &format!("SELECT note FROM {table} WHERE id=1")).await),
        "original"
    );
    let manual = session
        .execute_batch(vec![insert("3", "pending")], Policy::Manual)
        .await
        .unwrap();
    assert_eq!(manual.transaction_state, Tx::Active);
    assert_eq!(
        scalar(
            &sql(
                &observer,
                &format!("SELECT count(*) FROM {table} WHERE id=3")
            )
            .await
        ),
        "0"
    );
    assert_eq!(
        session
            .rollback(Policy::Manual)
            .await
            .unwrap()
            .transaction_state,
        Tx::Idle
    );
    assert_eq!(
        scalar(
            &sql(
                &observer,
                &format!("SELECT count(*) FROM {table} WHERE id=3")
            )
            .await
        ),
        "0"
    );
    let earlier = session
        .execute(
            &format!("INSERT INTO {table}(id,note) VALUES(4,'earlier')"),
            Policy::Manual,
        )
        .await
        .unwrap();
    assert_eq!(earlier.transaction_state, Tx::Active);
    let active_success = session
        .execute_batch(vec![insert("7", "active-success")], Policy::Auto)
        .await
        .unwrap();
    assert_eq!(active_success.outcome, Outcome::Completed);
    assert_eq!(active_success.transaction_state, Tx::Active);
    assert_eq!(
        scalar(
            &sql(
                &observer,
                &format!("SELECT count(*) FROM {table} WHERE id=7")
            )
            .await
        ),
        "0"
    );
    let conflict = session
        .execute_batch(
            vec![
                update("1", "changed", "original"),
                update("2", "changed", "missing"),
            ],
            Policy::Auto,
        )
        .await
        .unwrap();
    assert_eq!(conflict.outcome, Outcome::Failed);
    assert_eq!(conflict.transaction_state, Tx::Active);
    assert_eq!(
        scalar(
            &sql(
                &session,
                &format!("SELECT count(*) FROM {table} WHERE id=4")
            )
            .await
        ),
        "1"
    );
    assert_eq!(
        scalar(
            &sql(
                &observer,
                &format!("SELECT count(*) FROM {table} WHERE id=4")
            )
            .await
        ),
        "0"
    );
    let invalid = statement(
        format!("UPDATE {table} SET missing_column=$1::text"),
        vec![value("private-value")],
        1,
    );
    let failed_prepare = session
        .execute_batch(
            vec![update("1", "never-dispatched", "original"), invalid],
            Policy::Auto,
        )
        .await
        .unwrap();
    assert_eq!(failed_prepare.outcome, Outcome::Failed);
    assert_eq!(failed_prepare.transaction_state, Tx::Active);
    assert_eq!(
        failed_prepare
            .terminal_error
            .as_ref()
            .unwrap()
            .sqlstate
            .as_deref(),
        Some("42703")
    );
    assert!(!format!("{failed_prepare:?}").contains("private-value"));
    assert_eq!(
        scalar(&sql(&session, &format!("SELECT note FROM {table} WHERE id=1")).await),
        "original"
    );
    assert_eq!(
        scalar(
            &sql(
                &session,
                &format!("SELECT count(*) FROM {table} WHERE id=4")
            )
            .await
        ),
        "1"
    );
    assert_eq!(
        session
            .commit(Policy::Manual)
            .await
            .unwrap()
            .transaction_state,
        Tx::Idle
    );
    assert_eq!(
        scalar(
            &sql(
                &observer,
                &format!("SELECT count(*) FROM {table} WHERE id=4")
            )
            .await
        ),
        "1"
    );
    // Queued SQL can observe only the complete batch, never its first mutation.
    let batch=session.submit_batch(vec![update("1","phase","original"),statement(format!("UPDATE {table} SET note=$1::text FROM (SELECT pg_sleep(0.05)) delay WHERE id=2"),vec![value("phase")],1)],Policy::Auto).unwrap();
    let queued = session
        .submit(
            &format!("SELECT count(*) FROM {table} WHERE note='phase'"),
            Policy::Auto,
        )
        .unwrap();
    assert_eq!(batch.finish().await.unwrap().outcome, Outcome::Completed);
    assert_eq!(scalar(&queued.finish().await.unwrap()), "2");
    // PostgreSQL errors roll back a newly opened Manual batch, rather than leaving Failed.
    let null_fail = statement(
        format!("INSERT INTO {table}(id) VALUES($1::text::integer)"),
        vec![None],
        1,
    );
    let failed = session
        .execute_batch(vec![insert("5", "rolled back"), null_fail], Policy::Manual)
        .await
        .unwrap();
    assert_eq!(failed.outcome, Outcome::Failed);
    assert_eq!(failed.transaction_state, Tx::Idle);
    assert_eq!(
        scalar(
            &sql(
                &observer,
                &format!("SELECT count(*) FROM {table} WHERE id=5")
            )
            .await
        ),
        "0"
    );
    assert_eq!(
        session
            .execute("SELECT 1/0", Policy::Manual)
            .await
            .unwrap()
            .transaction_state,
        Tx::Failed
    );
    let rejected = session
        .execute_batch(vec![insert("8", "must not dispatch")], Policy::Auto)
        .await
        .unwrap();
    assert_eq!(rejected.outcome, Outcome::Rejected);
    assert_eq!(rejected.transaction_state, Tx::Failed);
    assert_eq!(
        session
            .rollback(Policy::Manual)
            .await
            .unwrap()
            .transaction_state,
        Tx::Idle
    );
    let pid = scalar(&sql(&session, "SELECT pg_backend_pid()").await);
    let slow=session.submit_batch(vec![update("1","cancel-first","phase"),statement(format!("UPDATE {table} SET note=$1::text FROM (SELECT pg_sleep(30)) delay WHERE id=2"),vec![value("cancel-second")],1)],Policy::Auto).unwrap();
    let start = Instant::now();
    loop {
        if scalar(
            &sql(
                &observer,
                &format!(
                    "SELECT count(*) FROM pg_stat_activity WHERE pid={pid} AND wait_event='PgSleep'"
                ),
            )
            .await,
        ) == "1"
        {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(3));
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(
        session.cancel(slow.key).await,
        dbide_pg_spike::CancelResult::RequestedConnectionWillRetire
    );
    let cancelled = tokio::time::timeout(Duration::from_secs(6), slow.finish())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cancelled.outcome, Outcome::Cancelled);
    assert!(cancelled.connection_retired);
    assert_eq!(cancelled.transaction_state, Tx::Unknown);
    assert_eq!(
        scalar(&sql(&observer, &format!("SELECT note FROM {table} WHERE id=1")).await),
        "phase"
    );
    assert_eq!(
        session
            .execute_batch(vec![insert("6", "must not replay")], Policy::Auto)
            .await
            .unwrap()
            .outcome,
        Outcome::Rejected
    );
    sql(&observer, &format!("DROP TABLE {table}")).await;
    observer.close();
    session.close();
}
#[tokio::test]
#[ignore = "requires project-owned isolated PostgreSQL fixture at127.0.0.1:55439"]
async fn read_only_profile_sets_a_server_default_with_explicit_override_semantics() {
    let owner = Session::connect_request(&request(), Limits::default())
        .await
        .unwrap();
    let table = name();
    sql(&owner, &format!("CREATE TABLE {table}(id integer)")).await;
    let mut request = request();
    request.profile.read_only = true;
    let session = Session::connect_request(&request, Limits::default())
        .await
        .unwrap();
    assert_eq!(
        scalar(&sql(&session, "SHOW default_transaction_read_only").await),
        "on"
    );
    let report = session
        .execute_batch(
            vec![statement(
                format!("INSERT INTO {table} VALUES($1::text::integer)"),
                vec![value("1")],
                1,
            )],
            Policy::Auto,
        )
        .await
        .unwrap();
    assert_eq!(report.outcome, Outcome::Failed);
    assert_eq!(report.transaction_state, Tx::Idle);
    assert_eq!(
        report.terminal_error.unwrap().sqlstate.as_deref(),
        Some("25006")
    );
    sql(&session, "SET default_transaction_read_only=off").await;
    assert_eq!(
        session
            .execute_batch(
                vec![statement(
                    format!("INSERT INTO {table} VALUES($1::text::integer)"),
                    vec![value("1")],
                    1
                )],
                Policy::Auto
            )
            .await
            .unwrap()
            .outcome,
        Outcome::Completed
    );
    session.close();
    sql(&owner, &format!("DROP TABLE {table}")).await;
    owner.close();
}
#[tokio::test]
async fn postgres_adapter_rejects_other_engines_before_network_io() {
    for engine in [
        dbide_profiles::DatabaseEngine::Sqlite,
        dbide_profiles::DatabaseEngine::MySql,
    ] {
        let mut request = request();
        request.profile.engine = engine;
        let error = Session::connect_request(&request, Limits::default())
            .await
            .err()
            .expect("wrong engine must fail");
        assert_eq!(
            error.kind,
            dbide_pg_spike::ConnectionErrorKind::Configuration
        );
        assert!(error.message.contains("engine"));
    }
}

#[tokio::test]
#[ignore = "requires project-owned isolated PostgreSQL fixture at127.0.0.1:55439"]
async fn deferred_constraint_commit_failure_is_confirmed_rolled_back() {
    let session = Session::connect_request(&request(), Limits::default())
        .await
        .unwrap();
    sql(
        &session,
        "CREATE TEMP TABLE batch_deferred(v integer UNIQUE DEFERRABLE INITIALLY DEFERRED)",
    )
    .await;
    let insert = || {
        statement(
            "INSERT INTO batch_deferred(v) VALUES($1::text::integer)".into(),
            vec![value("1")],
            1,
        )
    };
    let report = session
        .execute_batch(vec![insert(), insert()], Policy::Auto)
        .await
        .unwrap();
    assert_eq!(report.outcome, Outcome::Failed);
    assert_eq!(report.transaction_state, Tx::Idle);
    assert!(!report.connection_retired);
    assert_eq!(
        report.terminal_error.unwrap().sqlstate.as_deref(),
        Some("23505")
    );
    assert_eq!(
        scalar(&sql(&session, "SELECT count(*) FROM batch_deferred").await),
        "0"
    );
    assert_eq!(
        session
            .execute_batch(vec![insert()], Policy::Auto)
            .await
            .unwrap()
            .outcome,
        Outcome::Completed
    );
    session.close();
}
/// Fixture-only transparent protocol relay. It deliberately withholds COMMIT's
/// acknowledgement after observing the server's final ReadyForQuery, then closes.
/// It never logs frames; authentication contains only the generated fixture secret.
fn commit_ack_drop_proxy() -> (u16, std::thread::JoinHandle<bool>) {
    fixture_fault_proxy(false)
}
fn fixture_fault_proxy(stall_after_guard_lock: bool) -> (u16, std::thread::JoinHandle<bool>) {
    use std::{
        io::{Read, Write},
        net::{Shutdown, TcpListener, TcpStream},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    fn frame(socket: &mut TcpStream) -> std::io::Result<(u8, Vec<u8>)> {
        let mut header = [0u8; 5];
        socket.read_exact(&mut header)?;
        let length = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
        if !(4..=8 * 1024 * 1024).contains(&length) {
            return Err(std::io::Error::other(
                "fixture protocol frame exceeded bounds",
            ));
        }
        let mut body = vec![0; length - 4];
        socket.read_exact(&mut body)?;
        let mut wire = header.to_vec();
        wire.extend_from_slice(&body);
        Ok((header[0], wire))
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        let (mut client, _) = listener.accept().unwrap();
        let mut server = TcpStream::connect("127.0.0.1:55439").unwrap();
        for socket in [&client, &server] {
            socket
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(10)))
                .unwrap();
        }
        let mut length = [0u8; 4];
        client.read_exact(&mut length).unwrap();
        let size = u32::from_be_bytes(length) as usize;
        assert!((8..=65536).contains(&size));
        let mut startup = vec![0; size - 4];
        client.read_exact(&mut startup).unwrap();
        server.write_all(&length).unwrap();
        server.write_all(&startup).unwrap();
        let withholding = Arc::new(AtomicBool::new(false));
        let forward_flag = withholding.clone();
        let identity_seen = Arc::new(AtomicBool::new(false));
        let forward_identity = identity_seen.clone();
        let mut client_read = client.try_clone().unwrap();
        let mut server_write = server.try_clone().unwrap();
        let frontend = std::thread::spawn(move || {
            while let Ok((kind, wire)) = frame(&mut client_read) {
                if !stall_after_guard_lock && kind == b'Q' && &wire[5..] == b"COMMIT\0" {
                    forward_flag.store(true, Ordering::Release);
                }
                let marker = b"SELECT c.oid,c.relkind";
                if stall_after_guard_lock
                    && kind == b'P'
                    && wire.windows(marker.len()).any(|part| part == marker)
                {
                    forward_identity.store(true, Ordering::Release);
                }
                if server_write.write_all(&wire).is_err() {
                    break;
                }
            }
            let _ = server_write.shutdown(Shutdown::Both);
        });
        let mut committed = false;
        let mut saw_commit = false;
        let mut saw_lock = false;
        while let Ok((kind, wire)) = frame(&mut server) {
            if stall_after_guard_lock {
                if withholding.load(Ordering::Acquire) {
                    continue;
                }
                if kind == b'C' && &wire[5..] == b"LOCK TABLE\0" {
                    saw_lock = true;
                }
                if client.write_all(&wire).is_err() {
                    break;
                }
                if saw_lock && kind == b'Z' && wire.get(5) == Some(&b'T') {
                    committed = true;
                    withholding.store(true, Ordering::Release);
                }
                continue;
            }
            if withholding.load(Ordering::Acquire) {
                if kind == b'C' && &wire[5..] == b"COMMIT\0" {
                    saw_commit = true;
                }
                if kind == b'Z' {
                    committed = saw_commit && wire.get(5) == Some(&b'I');
                    break;
                }
            } else if client.write_all(&wire).is_err() {
                break;
            }
        }
        let _ = client.shutdown(Shutdown::Both);
        let _ = server.shutdown(Shutdown::Both);
        frontend.join().unwrap();
        committed && (!stall_after_guard_lock || identity_seen.load(Ordering::Acquire))
    });
    (port, handle)
}
#[tokio::test]
#[ignore = "requires project-owned isolated PostgreSQL fixture at127.0.0.1:55439"]
async fn dropped_commit_ack_reports_unknown_and_never_replays_committed_write() {
    let observer = Session::connect_request(&request(), Limits::default())
        .await
        .unwrap();
    let table = name();
    sql(&observer, &format!("CREATE TABLE {table}(v integer)")).await;
    let (port, proxy) = commit_ack_drop_proxy();
    let mut request = request();
    request.profile.port = port;
    let session = Session::connect_request(&request, Limits::default())
        .await
        .unwrap();
    let insert = || {
        statement(
            format!("INSERT INTO {table}(v) VALUES($1::text::integer)"),
            vec![value("1")],
            1,
        )
    };
    let report = tokio::time::timeout(
        Duration::from_secs(10),
        session.execute_batch(vec![insert()], Policy::Auto),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(report.outcome, Outcome::OutcomeUnknown);
    assert_eq!(report.transaction_state, Tx::Unknown);
    assert!(report.connection_retired);
    assert!(
        proxy.join().unwrap(),
        "fixture proxy did not observe committed ReadyForQuery before dropping the ack"
    );
    assert_eq!(
        scalar(&sql(&observer, &format!("SELECT count(*) FROM {table}")).await),
        "1"
    );
    assert_eq!(
        session
            .execute_batch(vec![insert()], Policy::Auto)
            .await
            .unwrap()
            .outcome,
        Outcome::Rejected
    );
    assert_eq!(
        scalar(&sql(&observer, &format!("SELECT count(*) FROM {table}")).await),
        "1",
        "uncertain COMMIT replayed a write"
    );
    sql(&observer, &format!("DROP TABLE {table}")).await;
    observer.close();
    session.close();
}

#[tokio::test]
#[ignore = "requires owned TLS PostgreSQL fixture55440 and explicit DBIDE_TEST_TLS_CA PEM path"]
async fn parameterized_batch_over_verified_tls() {
    let ca = std::env::var("DBIDE_TEST_TLS_CA")
        .expect("DBIDE_TEST_TLS_CA must explicitly name the fixture PEM CA");
    let mut request = request();
    request.profile.host = "localhost".into();
    request.profile.port = 55440;
    request.profile.tls_mode = dbide_profiles::TlsMode::VerifyFull;
    request.profile.root_cert_path = Some(ca);
    let session = Session::connect_request(&request, Limits::default())
        .await
        .unwrap();
    assert_eq!(
        scalar(
            &sql(
                &session,
                "SELECT ssl FROM pg_stat_ssl WHERE pid=pg_backend_pid()"
            )
            .await
        ),
        "t"
    );
    sql(
        &session,
        "CREATE TEMP TABLE batch_tls(note text,amount numeric,maybe text)",
    )
    .await;
    let report = session
        .execute_batch(
            vec![statement(
                "INSERT INTO batch_tls VALUES($1::text,$2::text::numeric,$3::text)".into(),
                vec![value("TLS 한글🙂"), value("123.4500"), None],
                1,
            )],
            Policy::Auto,
        )
        .await
        .unwrap();
    assert_eq!(report.outcome, Outcome::Completed);
    assert_eq!(report.transaction_state, Tx::Idle);
    assert_eq!(
        rows(&sql(&session, "SELECT note,amount::text,maybe FROM batch_tls").await),
        vec![vec![value("TLS 한글🙂"), value("123.4500"), None]]
    );
    session.close();
}

#[tokio::test]
#[ignore = "requires project-owned isolated PostgreSQL fixture at127.0.0.1:55439; exercises internal5s client deadline"]
async fn internal_commit_deadline_requests_server_cancel_before_retirement() {
    let session = Session::connect_request(&request(), Limits::default())
        .await
        .unwrap();
    let observer = Session::connect_request(&request(), Limits::default())
        .await
        .unwrap();
    let schema = name();
    let table = format!("\"{schema}\".target");
    sql(&session,&format!("CREATE SCHEMA \"{schema}\"; CREATE TABLE {table}(v integer); CREATE FUNCTION \"{schema}\".slow_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(30); RETURN NEW; END $$; CREATE CONSTRAINT TRIGGER slow_commit AFTER INSERT ON {table} DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION \"{schema}\".slow_commit()")).await;
    let pid = scalar(&sql(&session, "SELECT pg_backend_pid()").await);
    let started = Instant::now();
    let report = session
        .execute_batch(
            vec![statement(
                format!("INSERT INTO {table}(v) VALUES($1::text::integer)"),
                vec![value("1")],
                1,
            )],
            Policy::Auto,
        )
        .await
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(9));
    assert_eq!(report.outcome, Outcome::OutcomeUnknown);
    assert!(report.connection_retired);
    assert_eq!(report.transaction_state, Tx::Unknown);
    let gone = Instant::now();
    loop {
        if scalar(
            &sql(
                &observer,
                &format!("SELECT count(*) FROM pg_stat_activity WHERE pid={pid}"),
            )
            .await,
        ) == "0"
        {
            break;
        }
        assert!(
            gone.elapsed() < Duration::from_secs(2),
            "client deadline left server-side COMMIT trigger running"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        scalar(&sql(&observer, &format!("SELECT count(*) FROM {table}")).await),
        "0"
    );
    sql(&observer, &format!("DROP SCHEMA \"{schema}\" CASCADE")).await;
    session.close();
    observer.close();
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL fixture55439; blackholes identity response after guard LOCK acknowledgement"]
async fn guard_identity_response_blackhole_is_bounded_and_never_dispatches_mutation() {
    use dbide_pg_spike::{TableIdentityGuard, table_metadata_fingerprint, table_metadata_query};
    let observer = Session::connect_request(&request(), Limits::default())
        .await
        .unwrap();
    let table = name();
    let qualified = format!("public.{table}");
    sql(&observer, &format!("CREATE TABLE {qualified}(v integer)")).await;
    let oid = scalar(
        &sql(
            &observer,
            &format!("SELECT '{qualified}'::regclass::oid::text"),
        )
        .await,
    )
    .parse()
    .unwrap();
    let canonical = scalar(&sql(&observer, &table_metadata_query(oid)).await);
    let guard = TableIdentityGuard {
        schema: "public".into(),
        table,
        oid,
        metadata_fingerprint: Some(table_metadata_fingerprint(&canonical)),
    };
    let (port, proxy) = fixture_fault_proxy(true);
    let mut request = request();
    request.profile.port = port;
    let session = Session::connect_request(&request, Limits::default())
        .await
        .unwrap();
    let started = Instant::now();
    let report = session
        .execute_guarded_batch(
            guard,
            vec![statement(
                format!("INSERT INTO {qualified}(v) VALUES($1::text::integer)"),
                vec![value("1")],
                1,
            )],
            Policy::Auto,
        )
        .await
        .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(4500));
    assert!(started.elapsed() < Duration::from_secs(9));
    assert_eq!(report.outcome, Outcome::OutcomeUnknown);
    assert!(report.connection_retired);
    assert_eq!(report.transaction_state, Tx::Unknown);
    assert!(
        !report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandComplete { .. }))
    );
    assert!(
        proxy.join().unwrap(),
        "proxy did not observe guard LOCK ReadyForQuery followed by identity parse"
    );
    assert_eq!(
        scalar(&sql(&observer, &format!("SELECT count(*) FROM {qualified}")).await),
        "0"
    );
    sql(&observer, &format!("DROP TABLE {qualified}")).await;
    session.close();
    observer.close();
}
