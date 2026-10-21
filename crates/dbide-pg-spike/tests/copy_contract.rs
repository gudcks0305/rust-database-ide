//! M0 risk evidence only: console COPY is unsupported, not a transfer feature.
use dbide_pg_spike::{
    CancelResult, CommitPolicy as Policy, Event, ExecutionReport, Limits, Outcome,
    ServerTransactionState as Tx, Session,
};
use dbide_profiles::{ConnectionProfile, ConnectionRequest};
use futures_util::StreamExt;
use std::time::{Duration, Instant};
fn request() -> ConnectionRequest {
    ConnectionRequest {
        profile: ConnectionProfile::fixture(),
        password: "dbide_fixture_only".into(),
    }
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
async fn sql(session: &Session, query: &str) -> ExecutionReport {
    let report = session.execute(query, Policy::Auto).await.unwrap();
    assert_eq!(report.outcome, Outcome::Completed, "{report:?}");
    report
}
async fn backend_gone(observer: &Session, pid: &str) {
    let start = Instant::now();
    loop {
        if scalar(
            &sql(
                observer,
                &format!("SELECT count(*) FROM pg_stat_activity WHERE pid={pid}"),
            )
            .await,
        ) == "0"
        {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "unsupported COPY left its owned PostgreSQL backend alive"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
#[tokio::test]
#[ignore = "requires explicitly provisioned project-owned PostgreSQL fixture at127.0.0.1:55439"]
async fn unsupported_console_copy_directions_retire_without_hanging_or_backend_leaks() {
    let observer = Session::connect_request(&request(), Limits::default())
        .await
        .unwrap();
    for query in [
        "COPY (SELECT generate_series(1,3)) TO STDOUT",
        "COPY copy_m0_input(v) FROM STDIN",
    ] {
        let session = Session::connect_request(&request(), Limits::default())
            .await
            .unwrap();
        sql(&session, "CREATE TEMP TABLE copy_m0_input(v integer)").await;
        let pid = scalar(&sql(&session, "SELECT pg_backend_pid()").await);
        let execution = session.submit(query, Policy::Auto).unwrap();
        let key = execution.key;
        let start = Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(3), execution.finish()).await;
        if result.is_err() {
            let _ = tokio::time::timeout(Duration::from_secs(4), session.cancel(key)).await;
            session.close();
        }
        let report = result
            .expect("console COPY did not terminate within3s")
            .unwrap();
        assert!(start.elapsed() < Duration::from_secs(3));
        assert_eq!(report.outcome, Outcome::OutcomeUnknown);
        assert!(report.connection_retired);
        assert_eq!(report.transaction_state, Tx::Unknown);
        assert!(report.terminal_error.is_some());
        assert!(report.retained_bytes <= Limits::default().retained_bytes);
        assert!(
            !report
                .events
                .iter()
                .any(|event| matches!(event, Event::Row { .. })),
            "unsupported console path unexpectedly transferred COPY rows"
        );
        assert_eq!(session.cancel(key).await, CancelResult::NotRunning);
        assert_eq!(
            session
                .execute("SELECT 99", Policy::Auto)
                .await
                .unwrap()
                .outcome,
            Outcome::Rejected,
            "retired COPY session reused a protocol-ambiguous connection"
        );
        backend_gone(&observer, &pid).await;
        let fresh = Session::connect_request(&request(), Limits::default())
            .await
            .unwrap();
        let report = sql(&fresh, "SELECT 42").await;
        assert_eq!(scalar(&report), "42");
        assert_eq!(report.transaction_state, Tx::Idle);
        assert_ne!(report.key.session, key.session);
        fresh.close();
        session.close();
    }
    observer.close();
}
#[tokio::test]
#[ignore = "requires explicitly provisioned project-owned PostgreSQL fixture at127.0.0.1:55439; native candidate API only"]
async fn native_candidate_copy_out_can_drain_three_rows_and_reuse_its_connection() {
    let observer = Session::connect_request(&request(), Limits::default())
        .await
        .unwrap();
    let mut config = tokio_postgres::Config::new();
    config
        .host("127.0.0.1")
        .port(55439)
        .user("postgres")
        .password("dbide_fixture_only")
        .dbname("dbide_fixture")
        .connect_timeout(Duration::from_secs(3));
    let (client, connection) = config.connect(tokio_postgres::NoTls).await.unwrap();
    let driver = tokio::spawn(connection);
    let pid = client
        .simple_query("SELECT pg_backend_pid()")
        .await
        .unwrap()
        .iter()
        .find_map(|message| match message {
            tokio_postgres::SimpleQueryMessage::Row(row) => row.get(0).map(str::to_owned),
            _ => None,
        })
        .unwrap();
    let transfer = tokio::time::timeout(Duration::from_secs(3), async {
        let stream = client
            .copy_out("COPY (SELECT generate_series(1,3)) TO STDOUT")
            .await
            .unwrap();
        tokio::pin!(stream);
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.unwrap();
            assert!(
                bytes.len() + chunk.len() <= 1024,
                "small native COPY candidate exceeded byte budget"
            );
            bytes.extend_from_slice(&chunk);
        }
        bytes
    })
    .await;
    if transfer.is_err() {
        driver.abort();
        let _ = driver.await;
        panic!("native COPY candidate did not drain within3s");
    }
    assert_eq!(transfer.unwrap(), b"1\n2\n3\n");
    assert!(client.simple_query("SELECT 42").await.unwrap().iter().any(|message|matches!(message,tokio_postgres::SimpleQueryMessage::Row(row) if row.get(0)==Some("42"))));
    drop(client);
    driver.abort();
    let _ = driver.await;
    backend_gone(&observer, &pid).await;
    observer.close();
}
