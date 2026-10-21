use dbide_pg_spike::{
    CommitPolicy as Policy, Event, Limits, Outcome, ParameterizedStatement,
    ServerTransactionState as Tx, Session, TableIdentityGuard,
};
use dbide_profiles::{ConnectionProfile, ConnectionRequest};
use std::time::{SystemTime, UNIX_EPOCH};
fn request() -> ConnectionRequest {
    ConnectionRequest {
        profile: ConnectionProfile::fixture(),
        password: "dbide_fixture_only".into(),
    }
}
fn scalar(report: &dbide_pg_spike::ExecutionReport) -> String {
    report
        .events
        .iter()
        .find_map(|event| match event {
            Event::Row { values } => values.first().cloned().flatten(),
            _ => None,
        })
        .unwrap()
}
async fn sql(session: &Session, sql: &str) -> dbide_pg_spike::ExecutionReport {
    let report = session.execute(sql, Policy::Auto).await.unwrap();
    assert_eq!(report.outcome, Outcome::Completed, "{report:?}");
    report
}
async fn oid(session: &Session, qualified: &str) -> u32 {
    scalar(
        &sql(
            session,
            &format!("SELECT '{qualified}'::regclass::oid::text"),
        )
        .await,
    )
    .parse()
    .unwrap()
}
fn insert(qualified: &str, id: &str) -> Vec<ParameterizedStatement> {
    vec![ParameterizedStatement {
        sql: format!("INSERT INTO {qualified}(id,v) VALUES($1::text::integer,$2::text)"),
        parameters: vec![Some(id.into()), Some("guarded 한글🙂".into())],
        expected_affected_rows: Some(1),
    }]
}
#[tokio::test]
#[ignore = "requires explicitly provisioned project-owned PostgreSQL fixture at127.0.0.1:55439"]
async fn captured_oid_guard_rejects_recreated_tables_and_preserves_outer_transaction() {
    let session = Session::connect_request(&request(), Limits::default())
        .await
        .unwrap();
    let observer = Session::connect_request(&request(), Limits::default())
        .await
        .unwrap();
    let schema = format!(
        "dbide_guard_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let target = format!("\"{schema}\".\"target\"");
    let prior = format!("\"{schema}\".\"prior\"");
    sql(&session,&format!("CREATE SCHEMA \"{schema}\"; CREATE TABLE {target}(id integer PRIMARY KEY,v text); CREATE TABLE {prior}(id integer PRIMARY KEY)")).await;
    let old_oid = oid(&session, &target).await;
    let old_guard = TableIdentityGuard {
        schema: schema.clone(),
        table: "target".into(),
        oid: old_oid,
        metadata_fingerprint: None,
    };
    sql(
        &session,
        &format!("DROP TABLE {target}; CREATE TABLE {target}(id integer PRIMARY KEY,v text)"),
    )
    .await;
    let new_oid = oid(&session, &target).await;
    assert_ne!(new_oid, old_oid);
    let stale = session
        .execute_guarded_batch(old_guard.clone(), insert(&target, "1"), Policy::Auto)
        .await
        .unwrap();
    assert_eq!(stale.outcome, Outcome::Failed);
    assert_eq!(stale.transaction_state, Tx::Idle);
    assert!(!stale.connection_retired);
    assert!(
        stale
            .terminal_error
            .unwrap()
            .message
            .starts_with("TableIdentityChanged:")
    );
    assert_eq!(
        scalar(&sql(&observer, &format!("SELECT count(*) FROM {target}")).await),
        "0",
        "stale OID wrote into the new same-name table"
    );
    let current = TableIdentityGuard {
        oid: new_oid,
        ..old_guard.clone()
    };
    let applied = session
        .execute_guarded_batch(current.clone(), insert(&target, "1"), Policy::Auto)
        .await
        .unwrap();
    assert_eq!(applied.outcome, Outcome::Completed);
    assert_eq!(applied.transaction_state, Tx::Idle);
    assert_eq!(
        scalar(&sql(&observer, &format!("SELECT count(*) FROM {target}")).await),
        "1"
    );
    let pending = session
        .execute(
            &format!("INSERT INTO {prior}(id) VALUES(99)"),
            Policy::Manual,
        )
        .await
        .unwrap();
    assert_eq!(pending.transaction_state, Tx::Active);
    let pid = scalar(&sql(&session, "SELECT pg_backend_pid()").await);
    let stale = session
        .execute_guarded_batch(old_guard, insert(&target, "2"), Policy::Auto)
        .await
        .unwrap();
    assert_eq!(stale.outcome, Outcome::Failed);
    assert_eq!(stale.transaction_state, Tx::Active);
    assert_eq!(
        scalar(
            &sql(
                &session,
                &format!("SELECT count(*) FROM {prior} WHERE id=99")
            )
            .await
        ),
        "1"
    );
    assert_eq!(
        scalar(
            &sql(
                &observer,
                &format!("SELECT count(*) FROM {prior} WHERE id=99")
            )
            .await
        ),
        "0"
    );
    assert_eq!(scalar(&sql(&observer,&format!("SELECT count(*) FROM pg_locks WHERE pid={pid} AND relation={new_oid} AND mode='RowExclusiveLock' AND granted")).await),"0","failed guard leaked its savepoint lock");
    sql(
        &observer,
        &format!("BEGIN; LOCK TABLE {target} IN ACCESS EXCLUSIVE MODE NOWAIT; ROLLBACK"),
    )
    .await;
    let applied = session
        .execute_guarded_batch(current, insert(&target, "2"), Policy::Auto)
        .await
        .unwrap();
    assert_eq!(applied.outcome, Outcome::Completed);
    assert_eq!(applied.transaction_state, Tx::Active);
    assert_eq!(
        scalar(
            &sql(
                &observer,
                &format!("SELECT count(*) FROM {target} WHERE id=2")
            )
            .await
        ),
        "0",
        "guarded batch committed the caller's transaction"
    );
    assert_eq!(scalar(&sql(&observer,&format!("SELECT count(*) FROM pg_locks WHERE pid={pid} AND relation={new_oid} AND mode='RowExclusiveLock' AND granted")).await),"1","guard lock was released before the outer transaction finished");
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
                &format!("SELECT count(*) FROM {prior} WHERE id=99")
            )
            .await
        ),
        "1"
    );
    assert_eq!(
        scalar(
            &sql(
                &observer,
                &format!("SELECT count(*) FROM {target} WHERE id=2")
            )
            .await
        ),
        "1"
    );
    assert_eq!(scalar(&sql(&observer,&format!("SELECT count(*) FROM pg_locks WHERE pid={pid} AND relation={new_oid} AND mode='RowExclusiveLock' AND granted")).await),"0");
    let view = format!("\"{schema}\".\"target_view\"");
    sql(
        &session,
        &format!("CREATE VIEW {view} AS SELECT id,v FROM {target}"),
    )
    .await;
    let view_guard = TableIdentityGuard {
        schema: schema.clone(),
        table: "target_view".into(),
        oid: oid(&session, &view).await,
        metadata_fingerprint: None,
    };
    let refused = session
        .execute_guarded_batch(view_guard, insert(&view, "3"), Policy::Auto)
        .await
        .unwrap();
    assert_eq!(refused.outcome, Outcome::Failed);
    assert_eq!(refused.transaction_state, Tx::Idle);
    assert!(
        refused
            .terminal_error
            .unwrap()
            .message
            .starts_with("TableIdentityChanged:")
    );
    assert_eq!(
        scalar(&sql(&observer, &format!("SELECT count(*) FROM {target}")).await),
        "2"
    );
    let unusual_name = "quoted\"; DROP TABLE unrelated;--";
    let unusual = format!("\"{schema}\".\"{}\"", unusual_name.replace('"', "\"\""));
    sql(
        &session,
        &format!("CREATE TABLE {unusual}(id integer PRIMARY KEY,v text)"),
    )
    .await;
    let unusual_guard = TableIdentityGuard {
        schema: schema.clone(),
        table: unusual_name.into(),
        oid: oid(&session, &unusual).await,
        metadata_fingerprint: None,
    };
    let quoted = session
        .execute_guarded_batch(unusual_guard, insert(&unusual, "1"), Policy::Auto)
        .await
        .unwrap();
    assert_eq!(quoted.outcome, Outcome::Completed);
    assert_eq!(quoted.transaction_state, Tx::Idle);
    assert_eq!(
        scalar(&sql(&observer, &format!("SELECT count(*) FROM {unusual}")).await),
        "1"
    );
    assert_eq!(
        scalar(
            &sql(
                &observer,
                &format!("SELECT count(*) FROM {prior} WHERE id=99")
            )
            .await
        ),
        "1"
    );
    sql(&observer, &format!("DROP SCHEMA \"{schema}\" CASCADE")).await;
    session.close();
    observer.close();
}
