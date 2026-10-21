use dbide_pg_spike::{
    CommitPolicy as Policy, Event, ExecutionReport, Limits, Outcome, ParameterizedStatement,
    ServerTransactionState as Tx, Session, TableIdentityGuard, table_metadata_fingerprint,
    table_metadata_query,
};
use dbide_profiles::{ConnectionProfile, ConnectionRequest};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static IDS: AtomicU64 = AtomicU64::new(1);
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
async fn canonical(session: &Session, oid: u32) -> Option<String> {
    let report = sql(session, &table_metadata_query(oid)).await;
    assert!(!report.truncated);
    report
        .events
        .into_iter()
        .find_map(|e| match e {
            Event::Row { values } => Some(values.first().cloned().flatten()),
            _ => None,
        })
        .unwrap()
}
struct Fixture {
    session: Session,
    observer: Session,
    schema: String,
    target: String,
    prior: String,
    oid: u32,
}
impl Fixture {
    async fn new() -> Self {
        let request = || ConnectionRequest {
            profile: ConnectionProfile::fixture(),
            password: "dbide_fixture_only".into(),
        };
        let session = Session::connect_request(&request(), Limits::default())
            .await
            .unwrap();
        let observer = Session::connect_request(&request(), Limits::default())
            .await
            .unwrap();
        let schema = format!(
            "dbide_mg_{}_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            IDS.fetch_add(1, Ordering::Relaxed)
        );
        let target = format!("\"{schema}\".target");
        let prior = format!("\"{schema}\".prior");
        sql(&session,&format!("CREATE SCHEMA \"{schema}\"; CREATE TABLE {target}(id integer PRIMARY KEY,v text,CONSTRAINT target_positive CHECK(id>0)); CREATE TABLE {prior}(id integer PRIMARY KEY)")).await;
        let oid = scalar(&sql(&session, &format!("SELECT '{target}'::regclass::oid::text")).await)
            .parse()
            .unwrap();
        Self {
            session,
            observer,
            schema,
            target,
            prior,
            oid,
        }
    }
    async fn guard(&self) -> TableIdentityGuard {
        TableIdentityGuard {
            schema: self.schema.clone(),
            table: "target".into(),
            oid: self.oid,
            metadata_fingerprint: Some(table_metadata_fingerprint(
                &canonical(&self.session, self.oid).await.unwrap(),
            )),
        }
    }
    fn insert(&self, id: &str) -> Vec<ParameterizedStatement> {
        vec![ParameterizedStatement {
            sql: format!(
                "INSERT INTO {}(id,v) VALUES($1::text::integer,$2::text)",
                self.target
            ),
            parameters: vec![Some(id.into()), Some("metadata guard 한글🙂".into())],
            expected_affected_rows: Some(1),
        }]
    }
    async fn reject(
        &self,
        guard: TableIdentityGuard,
        statements: Vec<ParameterizedStatement>,
        state: Tx,
    ) {
        let report = self
            .session
            .execute_guarded_batch(guard, statements, Policy::Auto)
            .await
            .unwrap();
        assert_eq!(report.outcome, Outcome::Failed);
        assert_eq!(report.transaction_state, state);
        assert!(!report.connection_retired);
        assert!(
            report
                .terminal_error
                .as_ref()
                .unwrap()
                .message
                .starts_with("TableMetadataChanged:")
        );
        assert!(report.terminal_error.unwrap().sqlstate.is_none());
        assert!(
            !report
                .events
                .iter()
                .any(|e| matches!(e, Event::CommandComplete { .. })),
            "mutation executed before metadata rejection"
        );
    }
    async fn close(self) {
        self.session.rollback(Policy::Auto).await.unwrap();
        self.session.close();
        sql(
            &self.observer,
            &format!("DROP SCHEMA \"{}\" CASCADE", self.schema),
        )
        .await;
        self.observer.close();
    }
}
#[tokio::test]
#[ignore = "requires explicitly provisioned owned PostgreSQL fixture at127.0.0.1:55439"]
async fn same_oid_column_trigger_and_incoming_fk_changes_fail_before_mutation() {
    let fixture = Fixture::new().await;
    let baseline = fixture.guard().await;
    assert_eq!(
        fixture
            .session
            .execute_guarded_batch(baseline.clone(), fixture.insert("1"), Policy::Auto)
            .await
            .unwrap()
            .outcome,
        Outcome::Completed
    );
    assert_eq!(
        fixture.guard().await.metadata_fingerprint,
        baseline.metadata_fingerprint,
        "normal INSERT changed structural metadata"
    );
    sql(
        &fixture.session,
        &format!("UPDATE {} SET v='data-only' WHERE id=1", fixture.target),
    )
    .await;
    sql(&fixture.session, &format!("ANALYZE {}", fixture.target)).await;
    assert_eq!(
        fixture.guard().await.metadata_fingerprint,
        baseline.metadata_fingerprint,
        "normal UPDATE/ANALYZE changed structural metadata"
    );
    sql(&fixture.session, "SET search_path=pg_catalog").await;
    assert_eq!(
        fixture.guard().await.metadata_fingerprint,
        baseline.metadata_fingerprint,
        "search_path changed canonical metadata"
    );
    sql(&fixture.session, "SET search_path=public").await;
    sql(
        &fixture.observer,
        &format!(
            "ALTER TABLE {} ADD COLUMN added integer DEFAULT 7",
            fixture.target
        ),
    )
    .await;
    fixture
        .reject(baseline, fixture.insert("2"), Tx::Idle)
        .await;
    assert_eq!(
        scalar(
            &sql(
                &fixture.observer,
                &format!("SELECT count(*) FROM {} WHERE id=2", fixture.target)
            )
            .await
        ),
        "0"
    );
    let columns = fixture.guard().await;
    assert_eq!(
        fixture
            .session
            .execute_guarded_batch(columns.clone(), fixture.insert("2"), Policy::Auto)
            .await
            .unwrap()
            .outcome,
        Outcome::Completed
    );
    sql(&fixture.observer,&format!("CREATE TABLE \"{}\".audit(n integer); INSERT INTO \"{}\".audit VALUES(0); CREATE FUNCTION \"{}\".audit_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN UPDATE \"{}\".audit SET n=n+1; RETURN NEW; END $$; CREATE TRIGGER audit_write BEFORE INSERT OR UPDATE ON {} FOR EACH ROW EXECUTE FUNCTION \"{}\".audit_write()",fixture.schema,fixture.schema,fixture.schema,fixture.schema,fixture.target,fixture.schema)).await;
    fixture.reject(columns, fixture.insert("3"), Tx::Idle).await;
    assert_eq!(
        scalar(
            &sql(
                &fixture.observer,
                &format!("SELECT n FROM \"{}\".audit", fixture.schema)
            )
            .await
        ),
        "0",
        "stale metadata dispatched a new trigger"
    );
    assert_eq!(
        fixture
            .session
            .execute_guarded_batch(fixture.guard().await, fixture.insert("3"), Policy::Auto)
            .await
            .unwrap()
            .outcome,
        Outcome::Completed
    );
    assert_eq!(
        scalar(
            &sql(
                &fixture.observer,
                &format!("SELECT n FROM \"{}\".audit", fixture.schema)
            )
            .await
        ),
        "1"
    );
    sql(&fixture.observer,&format!("CREATE TABLE \"{}\".child(id integer PRIMARY KEY,target_id integer); INSERT INTO \"{}\".child VALUES(1,1)",fixture.schema,fixture.schema)).await;
    let before_fk = fixture.guard().await;
    assert_eq!(
        fixture
            .session
            .execute(
                &format!("INSERT INTO {} VALUES(99)", fixture.prior),
                Policy::Manual
            )
            .await
            .unwrap()
            .transaction_state,
        Tx::Active
    );
    sql(&fixture.observer,&format!("ALTER TABLE \"{}\".child ADD CONSTRAINT incoming_fk FOREIGN KEY(target_id) REFERENCES {}(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED",fixture.schema,fixture.target)).await;
    let delete = || {
        vec![ParameterizedStatement {
            sql: format!("DELETE FROM {} WHERE id=$1::text::integer", fixture.target),
            parameters: vec![Some("1".into())],
            expected_affected_rows: Some(1),
        }]
    };
    fixture.reject(before_fk, delete(), Tx::Active).await;
    assert_eq!(
        scalar(
            &sql(
                &fixture.session,
                &format!("SELECT count(*) FROM {} WHERE id=99", fixture.prior)
            )
            .await
        ),
        "1"
    );
    assert_eq!(
        scalar(
            &sql(
                &fixture.observer,
                &format!("SELECT count(*) FROM {} WHERE id=99", fixture.prior)
            )
            .await
        ),
        "0"
    );
    assert_eq!(
        scalar(
            &sql(
                &fixture.observer,
                &format!("SELECT count(*) FROM \"{}\".child", fixture.schema)
            )
            .await
        ),
        "1",
        "stale FK metadata cascaded a delete"
    );
    assert_eq!(
        fixture
            .session
            .commit(Policy::Manual)
            .await
            .unwrap()
            .transaction_state,
        Tx::Idle
    );
    assert_eq!(
        fixture
            .session
            .execute_guarded_batch(fixture.guard().await, delete(), Policy::Auto)
            .await
            .unwrap()
            .outcome,
        Outcome::Completed
    );
    assert_eq!(
        scalar(
            &sql(
                &fixture.observer,
                &format!("SELECT count(*) FROM \"{}\".child", fixture.schema)
            )
            .await
        ),
        "0"
    );
    assert_eq!(
        scalar(
            &sql(
                &fixture.observer,
                &format!("SELECT count(*) FROM {} WHERE id=99", fixture.prior)
            )
            .await
        ),
        "1"
    );
    assert_eq!(
        scalar(
            &sql(
                &fixture.session,
                &format!("SELECT '{}'::regclass::oid::text", fixture.target)
            )
            .await
        ),
        fixture.oid.to_string(),
        "test recreated relation instead of exercising same OID"
    );
    fixture.close().await;
}
#[tokio::test]
#[ignore = "requires explicitly provisioned owned PostgreSQL fixture at127.0.0.1:55439"]
async fn fingerprint_lock_blocks_metadata_ddl_but_allows_ordinary_dml() {
    let fixture = Fixture::new().await;
    let guard = fixture.guard().await;
    let applied = fixture
        .session
        .execute_guarded_batch(guard.clone(), fixture.insert("1"), Policy::Manual)
        .await
        .unwrap();
    assert_eq!(applied.outcome, Outcome::Completed);
    assert_eq!(applied.transaction_state, Tx::Active);
    let pid = scalar(&sql(&fixture.session, "SELECT pg_backend_pid()").await);
    assert_eq!(scalar(&sql(&fixture.observer,&format!("SELECT count(*) FROM pg_locks WHERE pid={pid} AND relation={} AND mode='ShareUpdateExclusiveLock' AND granted",fixture.oid)).await),"1");
    let competing = fixture
        .observer
        .execute(
            &format!(
                "BEGIN; LOCK TABLE {} IN SHARE UPDATE EXCLUSIVE MODE NOWAIT",
                fixture.target
            ),
            Policy::Auto,
        )
        .await
        .unwrap();
    assert_eq!(
        competing.terminal_error.unwrap().sqlstate.as_deref(),
        Some("55P03")
    );
    fixture.observer.rollback(Policy::Auto).await.unwrap();
    sql(&fixture.observer, "SET lock_timeout='100ms'").await;
    for ddl in [
        format!(
            "COMMENT ON TABLE {} IS 'blocked by edit guard'",
            fixture.target
        ),
        format!(
            "CREATE INDEX CONCURRENTLY target_concurrent ON {}(v)",
            fixture.target
        ),
        format!(
            "ALTER TABLE {} VALIDATE CONSTRAINT target_positive",
            fixture.target
        ),
    ] {
        let blocked = fixture.observer.execute(&ddl, Policy::Auto).await.unwrap();
        assert_eq!(blocked.outcome, Outcome::Failed, "{blocked:?}");
        assert_eq!(blocked.transaction_state, Tx::Idle);
        assert_eq!(
            blocked.terminal_error.unwrap().sqlstate.as_deref(),
            Some("55P03"),
            "DDL was not blocked: {ddl}"
        );
    }
    sql(&fixture.observer, "SET lock_timeout=0").await;
    sql(
        &fixture.observer,
        &format!(
            "INSERT INTO {}(id,v) VALUES(50,'concurrent DML')",
            fixture.target
        ),
    )
    .await;
    assert_eq!(
        fixture.guard().await.metadata_fingerprint,
        guard.metadata_fingerprint,
        "DML or failed DDL altered metadata witness"
    );
    assert_eq!(
        fixture
            .session
            .commit(Policy::Manual)
            .await
            .unwrap()
            .transaction_state,
        Tx::Idle
    );
    sql(
        &fixture.observer,
        &format!("COMMENT ON TABLE {} IS 'after commit'", fixture.target),
    )
    .await;
    assert_ne!(
        fixture.guard().await.metadata_fingerprint,
        guard.metadata_fingerprint
    );
    fixture.close().await;
}
#[tokio::test]
#[ignore = "requires explicitly provisioned owned PostgreSQL fixture at127.0.0.1:55439"]
async fn oversized_metadata_returns_null_and_rejects_without_transferring_large_cell() {
    let fixture = Fixture::new().await;
    let guard = fixture.guard().await;
    sql(&fixture.observer,&format!("DO $$ BEGIN EXECUTE format('COMMENT ON TABLE %I.target IS %L','{}',repeat('x',1100000)); END $$",fixture.schema)).await;
    let report = sql(&fixture.session, &table_metadata_query(fixture.oid)).await;
    assert!(!report.truncated);
    assert!(report.retained_bytes < 2048);
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e,Event::Row{values} if values==&vec![None]))
    );
    fixture
        .reject(guard.clone(), fixture.insert("1"), Tx::Idle)
        .await;
    sql(
        &fixture.observer,
        &format!("COMMENT ON TABLE {} IS NULL", fixture.target),
    )
    .await;
    assert_eq!(
        fixture.guard().await.metadata_fingerprint,
        guard.metadata_fingerprint
    );
    assert_eq!(
        fixture
            .session
            .execute_guarded_batch(guard, fixture.insert("1"), Policy::Auto)
            .await
            .unwrap()
            .outcome,
        Outcome::Completed
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires explicitly provisioned owned PostgreSQL fixture at127.0.0.1:55439"]
async fn server_bounded_guard_lock_timeout_restores_caller_settings_and_work() {
    use std::time::{Duration, Instant};
    for (lock, statement, ceiling) in [
        ("500ms", "7s", Duration::from_secs(2)),
        ("0", "0", Duration::from_secs(6)),
    ] {
        let fixture = Fixture::new().await;
        let guard = fixture.guard().await;
        sql(
            &fixture.session,
            &format!("SET lock_timeout='{lock}'; SET statement_timeout='{statement}'"),
        )
        .await;
        assert_eq!(
            fixture
                .session
                .execute(
                    &format!("INSERT INTO {} VALUES(99)", fixture.prior),
                    Policy::Manual
                )
                .await
                .unwrap()
                .transaction_state,
            Tx::Active
        );
        let pid = scalar(&sql(&fixture.session, "SELECT pg_backend_pid()").await);
        sql(
            &fixture.observer,
            &format!(
                "BEGIN; LOCK TABLE {} IN SHARE UPDATE EXCLUSIVE MODE",
                fixture.target
            ),
        )
        .await;
        let started = Instant::now();
        let blocked = fixture
            .session
            .execute_guarded_batch(guard.clone(), fixture.insert("1"), Policy::Auto)
            .await
            .unwrap();
        assert!(
            started.elapsed() < ceiling,
            "guard failed to honor server lock timeout"
        );
        let code = blocked.terminal_error.as_ref().unwrap().sqlstate.as_deref();
        // At equal four-second caps either server timer can fire first; both
        // are acknowledged server failures, unlike a client-only Unknown.
        assert!(matches!(
            (blocked.outcome, code),
            (Outcome::Failed, Some("55P03")) | (Outcome::Cancelled, Some("57014"))
        ));
        if lock == "500ms" {
            assert_eq!(code, Some("55P03"));
        }
        assert_eq!(blocked.transaction_state, Tx::Active);
        assert!(!blocked.connection_retired);
        assert_eq!(
            scalar(&sql(&fixture.session, "SHOW lock_timeout").await),
            lock
        );
        assert_eq!(
            scalar(&sql(&fixture.session, "SHOW statement_timeout").await),
            statement
        );
        assert_eq!(
            scalar(
                &sql(
                    &fixture.session,
                    &format!("SELECT count(*) FROM {} WHERE id=99", fixture.prior)
                )
                .await
            ),
            "1"
        );
        assert_eq!(scalar(&sql(&fixture.observer,&format!("SELECT count(*) FROM pg_stat_activity WHERE pid={pid} AND wait_event_type='Lock'")).await),"0","server backend remained waiting after client returned");
        fixture.observer.rollback(Policy::Auto).await.unwrap();
        let applied = fixture
            .session
            .execute_guarded_batch(guard, fixture.insert("1"), Policy::Auto)
            .await
            .unwrap();
        assert_eq!(applied.outcome, Outcome::Completed);
        assert_eq!(applied.transaction_state, Tx::Active);
        assert_eq!(
            scalar(&sql(&fixture.session, "SHOW lock_timeout").await),
            lock
        );
        assert_eq!(
            scalar(&sql(&fixture.session, "SHOW statement_timeout").await),
            statement
        );
        assert_eq!(
            fixture
                .session
                .commit(Policy::Manual)
                .await
                .unwrap()
                .transaction_state,
            Tx::Idle
        );
        assert_eq!(
            scalar(
                &sql(
                    &fixture.observer,
                    &format!("SELECT count(*) FROM {} WHERE id=99", fixture.prior)
                )
                .await
            ),
            "1"
        );
        fixture.close().await;
    }
}

#[tokio::test]
#[ignore = "requires explicitly provisioned owned PostgreSQL fixture at127.0.0.1:55439"]
async fn stale_repeatable_read_and_serializable_catalog_snapshots_are_rejected() {
    for isolation in ["REPEATABLE READ", "SERIALIZABLE"] {
        let fixture = Fixture::new().await;
        sql(
            &fixture.session,
            &format!(
                "BEGIN ISOLATION LEVEL {isolation}; INSERT INTO {} VALUES(99)",
                fixture.prior
            ),
        )
        .await;
        let old = fixture.guard().await;
        sql(&fixture.observer,&format!("CREATE TABLE \"{}\".audit(n integer); INSERT INTO \"{}\".audit VALUES(0); CREATE FUNCTION \"{}\".new_trigger() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN UPDATE \"{}\".audit SET n=n+1; RETURN NEW; END $$; CREATE TRIGGER new_trigger BEFORE INSERT ON {} FOR EACH ROW EXECUTE FUNCTION \"{}\".new_trigger()",fixture.schema,fixture.schema,fixture.schema,fixture.schema,fixture.target,fixture.schema)).await;
        // This is the original exploit precondition: MVCC still reports the OLD
        // metadata hash even though the live relation now has a new trigger.
        assert_eq!(
            fixture.guard().await.metadata_fingerprint,
            old.metadata_fingerprint
        );
        let rejected = fixture
            .session
            .execute_guarded_batch(old, fixture.insert("1"), Policy::Auto)
            .await
            .unwrap();
        assert_eq!(rejected.outcome, Outcome::Rejected);
        assert_eq!(rejected.transaction_state, Tx::Active);
        assert!(!rejected.connection_retired);
        assert!(
            rejected
                .terminal_error
                .unwrap()
                .message
                .starts_with("UnsupportedGuardIsolation:")
        );
        assert_eq!(
            scalar(&sql(&fixture.session, "SHOW transaction_isolation").await),
            isolation.to_ascii_lowercase()
        );
        assert_eq!(
            scalar(
                &sql(
                    &fixture.session,
                    &format!("SELECT count(*) FROM {} WHERE id=99", fixture.prior)
                )
                .await
            ),
            "1"
        );
        assert_eq!(
            scalar(
                &sql(
                    &fixture.observer,
                    &format!("SELECT count(*) FROM {}", fixture.target)
                )
                .await
            ),
            "0"
        );
        assert_eq!(
            scalar(
                &sql(
                    &fixture.observer,
                    &format!("SELECT n FROM \"{}\".audit", fixture.schema)
                )
                .await
            ),
            "0",
            "stale catalog snapshot executed a newly installed trigger"
        );
        fixture.session.rollback(Policy::Auto).await.unwrap();
        assert_eq!(
            scalar(
                &sql(
                    &fixture.observer,
                    &format!("SELECT count(*) FROM {} WHERE id=99", fixture.prior)
                )
                .await
            ),
            "0"
        );
        sql(
            &fixture.session,
            &format!("SET default_transaction_isolation='{isolation}'"),
        )
        .await;
        let fresh = fixture.guard().await;
        let rejected = fixture
            .session
            .execute_guarded_batch(fresh, fixture.insert("1"), Policy::Auto)
            .await
            .unwrap();
        assert_eq!(rejected.outcome, Outcome::Rejected);
        assert_eq!(rejected.transaction_state, Tx::Idle);
        assert!(
            rejected
                .terminal_error
                .unwrap()
                .message
                .starts_with("UnsupportedGuardIsolation:")
        );
        assert_eq!(
            scalar(&sql(&fixture.session, "SHOW default_transaction_isolation").await),
            isolation.to_ascii_lowercase(),
            "new scope silently changed the caller's isolation default"
        );
        sql(
            &fixture.session,
            "SET default_transaction_isolation='READ COMMITTED'",
        )
        .await;
        assert_eq!(
            fixture
                .session
                .execute_guarded_batch(fixture.guard().await, fixture.insert("1"), Policy::Auto)
                .await
                .unwrap()
                .outcome,
            Outcome::Completed
        );
        assert_eq!(
            scalar(
                &sql(
                    &fixture.observer,
                    &format!("SELECT n FROM \"{}\".audit", fixture.schema)
                )
                .await
            ),
            "1"
        );
        fixture.close().await;
    }
}
