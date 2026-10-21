//! Explicit, isolated loopback fixture only. Never read saved user profiles.
use dbide_mysql::{
    CancelResult, CommitPolicy, DatabaseEngine, Event, ExecutionReport, Limits, Outcome,
    ServerTransactionState, Session,
};
use dbide_profiles::{ConnectionProfile, ConnectionRequest, TlsMode};
use std::time::Duration;

fn request() -> ConnectionRequest {
    assert_eq!(
        std::env::var("DBIDE_MYSQL_FIXTURE").as_deref(),
        Ok("owned-loopback"),
        "set DBIDE_MYSQL_FIXTURE=owned-loopback for the owned Docker fixture"
    );
    ConnectionRequest {
        profile: ConnectionProfile {
            engine: DatabaseEngine::MySql,
            host: "127.0.0.1".into(),
            port: 55441,
            database: "dbide_fixture".into(),
            username: "root".into(),
            tls_mode: TlsMode::Disable,
            connect_timeout_secs: 3,
            ..ConnectionProfile::default()
        },
        password: "dbide_fixture_only".into(),
    }
}
async fn session() -> Session {
    Session::connect_request(&request(), Limits::default())
        .await
        .unwrap()
}
async fn run(session: &Session, sql: &str, policy: CommitPolicy) -> ExecutionReport {
    session.execute(sql, policy).await.unwrap()
}
fn rows(report: &ExecutionReport) -> Vec<Vec<Option<String>>> {
    report
        .events
        .iter()
        .filter_map(|event| {
            if let Event::Row { values } = event {
                Some(values.clone())
            } else {
                None
            }
        })
        .collect()
}
fn scalar(report: &ExecutionReport) -> String {
    rows(report)[0][0].clone().unwrap()
}
fn completed(report: &ExecutionReport) {
    assert_eq!(
        report.outcome,
        Outcome::Completed,
        "{:?}",
        report.terminal_error
    );
}

#[tokio::test]
#[ignore = "requires owned MySQL 8.4 loopback fixture on 55441"]
async fn dedicated_connection_preserves_id_temp_table_and_variables() {
    let s = session().await;
    let id = scalar(&run(&s, "SELECT CONNECTION_ID()", CommitPolicy::Auto).await);
    completed(&run(&s,"CREATE TEMPORARY TABLE dbide_temp(v INT); INSERT INTO dbide_temp VALUES(7); SET @dbide_value='한글'",CommitPolicy::Auto).await);
    let result = run(
        &s,
        "SELECT CONNECTION_ID(),v,@dbide_value FROM dbide_temp",
        CommitPolicy::Auto,
    )
    .await;
    completed(&result);
    assert_eq!(
        rows(&result),
        vec![vec![Some(id), Some("7".into()), Some("한글".into())]]
    );
    s.close();
}

#[tokio::test]
#[ignore = "requires owned MySQL 8.4 loopback fixture on 55441"]
async fn manual_commit_rollback_and_actual_implicit_commit_state() {
    let writer = session().await;
    let reader = session().await;
    let suffix = scalar(&run(&writer, "SELECT CONNECTION_ID()", CommitPolicy::Auto).await);
    let table = format!("dbide_manual_{suffix}");
    completed(
        &run(
            &writer,
            &format!("CREATE TABLE {table}(v INT) ENGINE=InnoDB"),
            CommitPolicy::Auto,
        )
        .await,
    );
    let first = run(
        &writer,
        &format!("INSERT INTO {table} VALUES(1)"),
        CommitPolicy::Manual,
    )
    .await;
    completed(&first);
    assert_eq!(first.transaction_state, ServerTransactionState::Active);
    assert_eq!(
        scalar(
            &run(
                &reader,
                &format!("SELECT COUNT(*) FROM {table}"),
                CommitPolicy::Auto
            )
            .await
        ),
        "0"
    );
    let commit = writer.commit(CommitPolicy::Manual).await.unwrap();
    completed(&commit);
    assert_eq!(commit.transaction_state, ServerTransactionState::Idle);
    assert_eq!(
        scalar(
            &run(
                &reader,
                &format!("SELECT COUNT(*) FROM {table}"),
                CommitPolicy::Auto
            )
            .await
        ),
        "1"
    );
    completed(
        &run(
            &writer,
            &format!("INSERT INTO {table} VALUES(2)"),
            CommitPolicy::Manual,
        )
        .await,
    );
    let rollback = writer.rollback(CommitPolicy::Manual).await.unwrap();
    completed(&rollback);
    assert_eq!(rollback.transaction_state, ServerTransactionState::Idle);
    assert_eq!(
        scalar(
            &run(
                &reader,
                &format!("SELECT COUNT(*) FROM {table}"),
                CommitPolicy::Auto
            )
            .await
        ),
        "1"
    );
    // DDL commits implicitly. Status must come from server, not submitted policy/SQL prefix.
    completed(
        &run(
            &writer,
            &format!("INSERT INTO {table} VALUES(3)"),
            CommitPolicy::Manual,
        )
        .await,
    );
    let ddl = run(
        &writer,
        &format!("ALTER TABLE {table} ADD COLUMN name TEXT"),
        CommitPolicy::Manual,
    )
    .await;
    completed(&ddl);
    assert_eq!(ddl.transaction_state, ServerTransactionState::Idle);
    completed(&run(&writer, &format!("DROP TABLE {table}"), CommitPolicy::Auto).await);
    writer.close();
    reader.close();
}

#[tokio::test]
#[ignore = "requires owned MySQL 8.4 loopback fixture on 55441"]
async fn raw_multiple_results_counts_values_and_later_error_preserve_rows() {
    let s = session().await;
    let result=run(&s,"SELECT NULL AS n,'' AS `empty`,CAST(1234567890.000000001 AS DECIMAL(30,9)) AS decimal_value,'한글 😀' AS unicode_value,_binary 0x00ff AS bytes; CREATE TEMPORARY TABLE dbide_counts(v INT); INSERT INTO dbide_counts VALUES(1),(2); SELECT 'semi;colon' AS literal; SELECT * FROM dbide_missing_table; SELECT 'must not execute'",CommitPolicy::Auto).await;
    assert_eq!(result.outcome, Outcome::Failed);
    assert_eq!(
        result.terminal_error.as_ref().unwrap().sqlstate.as_deref(),
        Some("42S02")
    );
    assert_eq!(
        rows(&result),
        vec![
            vec![
                None,
                Some("".into()),
                Some("1234567890.000000001".into()),
                Some("한글 😀".into()),
                Some("0x00ff".into())
            ],
            vec![Some("semi;colon".into())]
        ]
    );
    assert!(
        result
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandComplete { affected_rows: 2 }))
    );
    assert_eq!(result.transaction_state, ServerTransactionState::Unknown);
    completed(&run(&s, "SELECT 8", CommitPolicy::Auto).await);
    s.close();
}

#[tokio::test]
#[ignore = "requires owned MySQL 8.4 loopback fixture on 55441"]
async fn retention_is_bounded_but_protocol_drains_and_terminal_error_survives() {
    let s = Session::connect_request(
        &request(),
        Limits {
            events: 2,
            retained_bytes: 100,
            sql_bytes: 4096,
        },
    )
    .await
    .unwrap();
    let r = run(
        &s,
        "SELECT REPEAT('한',1000); SELECT 2; SELECT * FROM dbide_missing_table",
        CommitPolicy::Auto,
    )
    .await;
    assert!(r.truncated);
    assert!(r.retained_bytes <= 100);
    assert!(r.events.len() <= 2);
    assert!(r.terminal_error.is_some());
    assert_eq!(r.outcome, Outcome::Failed);
    completed(&run(&s, "SELECT 1", CommitPolicy::Auto).await);
    assert!(s.submit(&"x".repeat(4097), CommitPolicy::Auto).is_err());
    s.close();
}

#[tokio::test]
#[ignore = "requires owned MySQL 8.4 loopback fixture on 55441"]
async fn stale_cancel_cannot_kill_next_execution_and_accepted_cancel_retires() {
    let s = session().await;
    let unaffected = session().await;
    let previous = s.submit("SELECT 1", CommitPolicy::Auto).unwrap();
    let stale = previous.key;
    completed(&previous.finish().await.unwrap());
    let running = s.submit("SELECT SLEEP(20)", CommitPolicy::Auto).unwrap();
    let key = running.key;
    tokio::time::timeout(Duration::from_secs(2), async {
        while !s.is_running(key).await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(s.cancel(stale).await, CancelResult::NotRunning);
    assert!(s.is_running(key).await);
    let queued = s
        .submit("SELECT 'must not run'", CommitPolicy::Auto)
        .unwrap();
    assert_eq!(
        s.cancel(key).await,
        CancelResult::RequestedConnectionWillRetire
    );
    let result = tokio::time::timeout(Duration::from_secs(7), running.finish())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        result.outcome,
        Outcome::Cancelled | Outcome::OutcomeUnknown
    ));
    assert!(result.connection_retired);
    assert!(s.is_closed());
    assert_eq!(queued.finish().await.unwrap().outcome, Outcome::Rejected);
    assert!(s.submit_commit(CommitPolicy::Manual).is_err());
    assert!(s.submit_rollback(CommitPolicy::Manual).is_err());
    assert_eq!(s.cancel(key).await, CancelResult::NotRunning);
    completed(&run(&unaffected, "SELECT 5", CommitPolicy::Auto).await);
    unaffected.close();
}

#[tokio::test]
#[ignore = "requires owned MySQL 8.4 loopback fixture on 55441"]
async fn broken_connection_never_reconnects_or_replays() {
    let s = session().await;
    let killer = session().await;
    let id = scalar(&run(&s, "SELECT CONNECTION_ID()", CommitPolicy::Auto).await);
    completed(
        &run(
            &killer,
            &format!("KILL CONNECTION {id}"),
            CommitPolicy::Auto,
        )
        .await,
    );
    let r = run(&s, "SELECT 1", CommitPolicy::Auto).await;
    assert_eq!(r.outcome, Outcome::OutcomeUnknown);
    assert!(r.connection_retired);
    assert!(s.is_closed());
    assert!(s.submit("SELECT 2", CommitPolicy::Auto).is_err());
    killer.close();
}

#[tokio::test]
#[ignore = "requires owned MySQL 8.4 loopback fixture on 55441"]
async fn secure_default_rejects_untrusted_fixture_certificate() {
    let mut r = request();
    r.profile.tls_mode = TlsMode::VerifyFull;
    assert!(
        Session::connect_request(&r, Limits::default())
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires owned MySQL 8.4 loopback fixture on 55441"]
async fn read_only_default_is_applied_and_honestly_not_a_sql_security_boundary() {
    let owner = session().await;
    let suffix = scalar(&run(&owner, "SELECT CONNECTION_ID()", CommitPolicy::Auto).await);
    let table = format!("dbide_read_only_{suffix}");
    completed(
        &run(
            &owner,
            &format!("CREATE TABLE {table}(v INT) ENGINE=InnoDB"),
            CommitPolicy::Auto,
        )
        .await,
    );
    let mut r = request();
    r.profile.read_only = true;
    let reader = Session::connect_request(&r, Limits::default())
        .await
        .unwrap();
    let denied = run(
        &reader,
        &format!("INSERT INTO {table} VALUES(1)"),
        CommitPolicy::Manual,
    )
    .await;
    assert_eq!(denied.outcome, Outcome::Failed);
    completed(&reader.rollback(CommitPolicy::Manual).await.unwrap());
    // Demonstrate the documented boundary: this privileged SQL user can change
    // its own default. A read-only profile is not server-side authorization.
    completed(
        &run(
            &reader,
            "SET SESSION TRANSACTION READ WRITE",
            CommitPolicy::Auto,
        )
        .await,
    );
    completed(
        &run(
            &reader,
            &format!("INSERT INTO {table} VALUES(2)"),
            CommitPolicy::Auto,
        )
        .await,
    );
    assert_eq!(
        scalar(
            &run(
                &owner,
                &format!("SELECT COUNT(*) FROM {table}"),
                CommitPolicy::Auto
            )
            .await
        ),
        "1"
    );
    completed(&run(&owner, &format!("DROP TABLE {table}"), CommitPolicy::Auto).await);
    reader.close();
    owner.close();
}

fn tls_request() -> ConnectionRequest {
    let mut r = request();
    r.profile.tls_mode = TlsMode::VerifyFull;
    r.profile.root_cert_path = Some(
        std::env::var("DBIDE_MYSQL_FIXTURE_CA")
            .expect("set DBIDE_MYSQL_FIXTURE_CA to the owned fixture CA PEM"),
    );
    r
}

#[tokio::test]
#[ignore = "requires owned MySQL TLS fixture and DBIDE_MYSQL_FIXTURE_CA"]
async fn verified_tls_with_explicit_ca_encrypts_dns_and_ip_connections() {
    for host in ["localhost", "127.0.0.1"] {
        for mode in [TlsMode::VerifyFull, TlsMode::Require] {
            let mut r = tls_request();
            r.profile.host = host.into();
            r.profile.tls_mode = mode;
            let s = Session::connect_request(&r, Limits::default())
                .await
                .unwrap();
            let report = run(
                &s,
                "SHOW SESSION STATUS LIKE 'Ssl_cipher'",
                CommitPolicy::Auto,
            )
            .await;
            completed(&report);
            let values = rows(&report);
            assert_eq!(values.len(), 1);
            assert_eq!(values[0][0].as_deref(), Some("Ssl_cipher"));
            assert!(
                !values[0][1].as_deref().unwrap().is_empty(),
                "TLS cipher empty for {host} {mode:?}"
            );
            s.close();
        }
    }
}

#[tokio::test]
#[ignore = "requires owned MySQL TLS fixture and DBIDE_MYSQL_FIXTURE_CA"]
async fn verified_tls_rejects_unrelated_valid_ca() {
    let mut r = tls_request();
    r.profile.root_cert_path = Some(format!(
        "{}/tests/fixtures/untrusted-ca.pem",
        env!("CARGO_MANIFEST_DIR")
    ));
    assert!(matches!(
        Session::connect_request(&r, Limits::default()).await,
        Err(dbide_mysql::ConnectError::Tls)
    ));
}

#[tokio::test]
#[ignore = "requires owned MySQL TLS fixture and DBIDE_MYSQL_FIXTURE_CA"]
async fn driver_rejects_hostname_mismatch_even_with_correct_trust_anchor() {
    use mysql_async::{Conn, OptsBuilder, SslOpts};
    let r = tls_request();
    let tls = SslOpts::default()
        .with_root_certs(vec![
            std::path::PathBuf::from(r.profile.root_cert_path.unwrap()).into(),
        ])
        // Test-only driver API lets the TLS verification name differ from the
        // loopback TCP destination. It does not disable certificate validation.
        .with_danger_tls_hostname_override(Some("wrong.invalid"));
    assert!(!tls.skip_domain_validation());
    assert!(!tls.accept_invalid_certs());
    let opts = OptsBuilder::default()
        .ip_or_hostname("127.0.0.1")
        .tcp_port(55441)
        .user(Some("root"))
        .pass(Some("dbide_fixture_only"))
        .db_name(Some("dbide_fixture"))
        .prefer_socket(false)
        .ssl_opts(tls);
    let result = tokio::time::timeout(Duration::from_secs(3), Conn::new(opts))
        .await
        .unwrap();
    assert!(matches!(
        result,
        Err(mysql_async::Error::Io(mysql_async::IoError::Tls(_)))
    ));
}

#[tokio::test]
#[ignore = "requires owned MySQL 8.4 loopback fixture on 55441"]
async fn protocol_status_preserves_row_count_warnings_and_error_diagnostics() {
    let s = session().await;
    completed(&run(&s,"CREATE TEMPORARY TABLE dbide_diagnostics(v INT); INSERT INTO dbide_diagnostics VALUES(1),(2),(3)",CommitPolicy::Auto).await);
    completed(
        &run(
            &s,
            "UPDATE dbide_diagnostics SET v=v+10 WHERE v<3",
            CommitPolicy::Auto,
        )
        .await,
    );
    assert_eq!(
        scalar(&run(&s, "SELECT ROW_COUNT()", CommitPolicy::Auto).await),
        "2"
    );
    completed(
        &run(
            &s,
            "SELECT CAST('not-a-number' AS UNSIGNED)",
            CommitPolicy::Auto,
        )
        .await,
    );
    let warnings = run(&s, "SHOW WARNINGS", CommitPolicy::Auto).await;
    completed(&warnings);
    assert!(
        !rows(&warnings).is_empty(),
        "user warnings must survive status synchronization"
    );
    assert!(
        rows(&warnings)
            .iter()
            .any(|r| r[1].as_deref() == Some("1292"))
    );
    let failed = run(
        &s,
        "SELECT * FROM dbide_missing_diagnostics_table",
        CommitPolicy::Manual,
    )
    .await;
    assert_eq!(failed.outcome, Outcome::Failed);
    assert_eq!(failed.transaction_state, ServerTransactionState::Unknown);
    let blocked = run(&s, "SELECT 'must not dispatch'", CommitPolicy::Manual).await;
    assert_eq!(blocked.outcome, Outcome::Rejected);
    let error_warnings = run(&s, "SHOW WARNINGS", CommitPolicy::Auto).await;
    assert_eq!(
        error_warnings.transaction_state,
        ServerTransactionState::Active
    );
    completed(&error_warnings);
    assert!(
        rows(&error_warnings)
            .iter()
            .any(|r| r[1].as_deref() == Some("1146")),
        "server error diagnostics must survive state synchronization"
    );
    let rollback = s.rollback(CommitPolicy::Manual).await.unwrap();
    completed(&rollback);
    assert_eq!(rollback.transaction_state, ServerTransactionState::Idle);
    s.close();
}

#[tokio::test]
#[ignore = "requires owned MySQL 8.4 loopback fixture on 55441"]
async fn selects_close_their_own_result_before_each_update_count() {
    let s = session().await;
    completed(&run(&s,"CREATE TEMPORARY TABLE dbide_event_boundaries(v INT); INSERT INTO dbide_event_boundaries VALUES(1),(2)",CommitPolicy::Auto).await);
    let r=run(&s,"SELECT 7 AS first; UPDATE dbide_event_boundaries SET v=v+1; SELECT 9 AS last; SELECT v AS none FROM dbide_event_boundaries WHERE FALSE",CommitPolicy::Auto).await;
    completed(&r);
    assert_eq!(
        r.events,
        vec![
            Event::ResultSetStarted {
                columns: vec!["first".into()]
            },
            Event::Row {
                values: vec![Some("7".into())]
            },
            Event::CommandComplete { affected_rows: 1 },
            Event::CommandComplete { affected_rows: 2 },
            Event::ResultSetStarted {
                columns: vec!["last".into()]
            },
            Event::Row {
                values: vec![Some("9".into())]
            },
            Event::CommandComplete { affected_rows: 1 },
            Event::ResultSetStarted {
                columns: vec!["none".into()]
            },
            Event::CommandComplete { affected_rows: 0 },
        ]
    );
    s.close();
}
