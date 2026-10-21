//! Executable assertions against a caller-provisioned disposable server.
use crate::*;
use std::{
    error::Error,
    io,
    time::{SystemTime, UNIX_EPOCH},
};
type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
#[derive(Serialize)]
pub struct ContractEvidence {
    pub status: &'static str,
    pub driver: &'static str,
    pub server: String,
    pub scenarios: Vec<&'static str>,
    pub limitations: Vec<&'static str>,
}
fn require(value: bool, why: &str) -> Result<()> {
    if value {
        Ok(())
    } else {
        Err(io::Error::other(why).into())
    }
}
fn text(report: &ExecutionReport) -> Vec<String> {
    report
        .events
        .iter()
        .filter_map(|event| match event {
            Event::Row { values } => Some(values.iter().flatten().cloned()),
            _ => None,
        })
        .flatten()
        .collect()
}
async fn sql(session: &Session, query: &str) -> Result<ExecutionReport> {
    let report = session.execute(query, CommitPolicy::Auto).await?;
    require(
        report.outcome == Outcome::Completed,
        &format!("unexpected query outcome: {:?}", report),
    )?;
    Ok(report)
}
async fn running(session: &Session, key: ExecutionKey) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !session.is_running(key).await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| io::Error::other("query never entered running state"))?;
    Ok(())
}
async fn server_sleeping(observer: &Session, pid: &str) -> Result<()> {
    let started = Instant::now();
    loop {
        let activity = sql(observer, &format!("SELECT count(*) FROM pg_stat_activity WHERE pid={pid} AND state='active' AND wait_event='PgSleep'")).await?;
        if text(&activity) == ["1"] {
            return Ok(());
        }
        require(
            started.elapsed() < Duration::from_secs(3),
            "target backend never entered pg_sleep",
        )?;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
/// Mutates only newly generated fixture objects. Run against a disposable database.
/// Never accepts or discovers saved user profiles.
pub async fn verify(url: &str) -> Result<ContractEvidence> {
    let session = Session::connect(url, Limits::default()).await?;
    let observer = Session::connect(url, Limits::default()).await?;
    let server = text(&sql(&session, "SELECT version()").await?).join("");
    let pid = text(&sql(&session, "SELECT pg_backend_pid()").await?)[0].clone();
    let mut scenarios = Vec::new();
    sql(
        &session,
        "CREATE TEMP TABLE dbide_pin(v text); INSERT INTO dbide_pin VALUES ('한글;🙂');",
    )
    .await?;
    require(
        text(&sql(&session, "SELECT v FROM dbide_pin").await?) == ["한글;🙂"],
        "pinned temp table lost",
    )?;
    let isolated = observer
        .execute("SELECT * FROM dbide_pin", CommitPolicy::Auto)
        .await?;
    require(
        isolated.outcome == Outcome::Failed
            && isolated
                .events
                .iter()
                .any(|e| matches!(e, Event::Error { sqlstate: Some(s), .. } if s == "42P01")),
        "temp table leaked to another session",
    )?;
    scenarios.push("pinned_temp_table_and_session_isolation");

    let table = format!(
        "dbide_contract_{}_{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    sql(&session, &format!("CREATE TABLE {table}(v integer)")).await?;
    let manual = session
        .execute(
            &format!("INSERT INTO {table} VALUES(1)"),
            CommitPolicy::Manual,
        )
        .await?;
    require(
        manual.outcome == Outcome::Completed
            && manual.commit_policy == CommitPolicy::Manual
            && manual.transaction_state == ServerTransactionState::Active,
        "manual policy/state contract",
    )?;
    require(
        text(&sql(&observer, &format!("SELECT count(*) FROM {table}")).await?) == ["0"],
        "uncommitted write visible",
    )?;
    let committed = session.commit(CommitPolicy::Manual).await?;
    require(
        committed.outcome == Outcome::Completed
            && committed.transaction_state == ServerTransactionState::Idle
            && committed.commit_policy == CommitPolicy::Manual,
        "dedicated manual COMMIT incorrectly auto-began or lost wire state",
    )?;
    let next_manual = session
        .execute(
            &format!("INSERT INTO {table} VALUES(2)"),
            CommitPolicy::Manual,
        )
        .await?;
    require(
        next_manual.outcome == Outcome::Completed
            && next_manual.transaction_state == ServerTransactionState::Active,
        "Manual query after COMMIT did not begin a fresh transaction",
    )?;
    let rolled = session.rollback(CommitPolicy::Manual).await?;
    require(
        rolled.outcome == Outcome::Completed
            && rolled.transaction_state == ServerTransactionState::Idle,
        "dedicated manual ROLLBACK incorrectly auto-began or lost wire state",
    )?;
    require(
        text(&sql(&observer, &format!("SELECT count(*) FROM {table}")).await?) == ["1"],
        "commit not visible",
    )?;
    sql(
        &session,
        &format!("BEGIN; INSERT INTO {table} VALUES(2); ROLLBACK"),
    )
    .await?;
    require(
        text(&sql(&observer, &format!("SELECT count(*) FROM {table}")).await?) == ["1"],
        "rollback persisted",
    )?;
    let direct = sql(&session, "BEGIN").await?;
    require(
        direct.commit_policy == CommitPolicy::Auto
            && direct.transaction_state == ServerTransactionState::Active,
        "raw BEGIN did not preserve client policy and report actual protocol status",
    )?;
    let actual = text(
        &sql(
            &observer,
            &format!("SELECT state FROM pg_stat_activity WHERE pid = {pid}"),
        )
        .await?,
    );
    require(
        actual == ["idle in transaction"],
        "server did not observe direct BEGIN",
    )?;
    let error = session.execute("SELECT 1 / 0", CommitPolicy::Auto).await?;
    require(
        error.outcome == Outcome::Failed
            && error.transaction_state == ServerTransactionState::Failed,
        "SQL error did not report failed transaction",
    )?;
    let rejected_commit = session.commit(CommitPolicy::Auto).await?;
    require(
        rejected_commit.outcome == Outcome::Rejected
            && rejected_commit.transaction_state == ServerTransactionState::Failed,
        "Failed transaction COMMIT was dispatched instead of requiring ROLLBACK",
    )?;
    let actual = text(
        &sql(
            &observer,
            &format!("SELECT state FROM pg_stat_activity WHERE pid = {pid}"),
        )
        .await?,
    );
    require(
        actual == ["idle in transaction (aborted)"],
        "server did not observe failed transaction",
    )?;
    sql(&session, "ROLLBACK").await?;
    require(
        text(&sql(&session, "SELECT 42").await?) == ["42"],
        "query not recovered after rollback",
    )?;
    scenarios.push("manual_commit_rollback_visibility_and_raw_begin_policy_distinction");
    scenarios.push("failed_transaction_and_sql_error_drain_recovery");

    let multi = sql(&session, &format!("SELECT 'a;b' AS first; DO $$ BEGIN RAISE NOTICE 'dbide_notice'; END $$; INSERT INTO {table} VALUES(3),(4); UPDATE {table} SET v=v+10 WHERE v=3; SELECT NULL AS n, '' AS empty, '한글🙂' AS unicode")).await?;
    require(
        multi
            .events
            .iter()
            .filter(|e| matches!(e, Event::ResultSetStarted { .. }))
            .count()
            == 2,
        "multi-result schemas lost",
    )?;
    let counts: Vec<u64> = multi
        .events
        .iter()
        .filter_map(|e| {
            if let Event::CommandComplete { affected_rows } = e {
                Some(*affected_rows)
            } else {
                None
            }
        })
        .collect();
    require(
        counts == [1, 0, 2, 1, 1],
        "update/result counts out of order",
    )?;
    require(
        multi.notices.iter().any(|n| n.message == "dbide_notice"),
        "notice lost",
    )?;
    require(multi.events.iter().any(|e| matches!(e, Event::Row { values } if values == &vec![None, Some("".into()), Some("한글🙂".into())])), "NULL/empty/unicode not preserved")?;
    let partial = session
        .execute(
            "SELECT 'before'; SELECT 1/0; SELECT 'after'",
            CommitPolicy::Auto,
        )
        .await?;
    require(
        partial.outcome == Outcome::Failed && text(&partial) == ["before"],
        "partial rows or middle error lost",
    )?;
    require(
        matches!(partial.events.last(), Some(Event::Error { sqlstate: Some(s), .. }) if s == "22012"),
        "middle SQLSTATE not preserved",
    )?;
    require(
        text(&sql(&session, "SELECT 'recovered'").await?) == ["recovered"],
        "SQL error left connection unusable",
    )?;
    scenarios.push("ordered_multi_results_counts_notice_partial_error_null_empty_unicode");

    let tiny = Session::connect(
        url,
        Limits {
            retained_bytes: 4096,
            events: 32,
            ..Limits::default()
        },
    )
    .await?;
    let bounded = sql(
        &tiny,
        "SELECT repeat('x',1000) FROM generate_series(1,10000)",
    )
    .await?;
    require(
        bounded.truncated && bounded.retained_bytes <= 4096 && bounded.events.len() <= 32,
        "retained result cap violated",
    )?;
    require(
        text(&sql(&tiny, "SELECT 'after-drain'").await?) == ["after-drain"],
        "truncation failed to drain",
    )?;
    let truncated_error = tiny
        .execute(
            "SELECT repeat('x',1000) FROM generate_series(1,100); SELECT 1/0",
            CommitPolicy::Auto,
        )
        .await?;
    require(
        truncated_error.truncated
            && truncated_error.outcome == Outcome::Failed
            && truncated_error
                .terminal_error
                .as_ref()
                .and_then(|e| e.sqlstate.as_deref())
                == Some("22012"),
        "terminal SQL error disappeared after row budget was exhausted",
    )?;
    scenarios.push("retained_byte_event_budget_and_truncated_stream_drain");

    let prior = sql(&session, "SELECT 1").await?;
    // The next query is already running when cancellation for completed query N arrives.
    let next = session.submit("SELECT pg_sleep(0.15), 'next'", CommitPolicy::Auto)?;
    running(&session, next.key).await?;
    require(
        session.cancel(prior.key).await == CancelResult::NotRunning,
        "stale cancel accepted",
    )?;
    let mut wrong_generation = next.key;
    wrong_generation.generation += 1;
    require(
        session.cancel(wrong_generation).await == CancelResult::NotRunning,
        "wrong generation accepted",
    )?;
    let mut wrong_session = next.key;
    wrong_session.session += 1;
    require(
        session.cancel(wrong_session).await == CancelResult::NotRunning,
        "wrong session accepted",
    )?;
    require(
        next.finish().await?.outcome == Outcome::Completed,
        "late cancel struck next query",
    )?;
    scenarios.push("completed_execution_cancel_cannot_hit_running_next_query");

    let cancel_session = Session::connect(url, Limits::default()).await?;
    let cancel_pid = text(&sql(&cancel_session, "SELECT pg_backend_pid()").await?)[0].clone();
    let slow = cancel_session.submit("SELECT pg_sleep(30)", CommitPolicy::Auto)?;
    running(&cancel_session, slow.key).await?;
    // Observe that pg_sleep actually started on the server before requesting cancellation.
    // Actor Running begins before socket transmission; a short delay alone is not proof.
    server_sleeping(&observer, &cancel_pid).await?;
    let accepted = cancel_session.cancel(slow.key).await;
    require(
        accepted == CancelResult::RequestedConnectionWillRetire,
        "cancel request failed",
    )?;
    let cancelled = tokio::time::timeout(Duration::from_secs(5), slow.finish()).await??;
    require(
        cancelled.outcome == Outcome::Cancelled
            && cancelled.connection_retired
            && cancelled.transaction_state == ServerTransactionState::Unknown,
        "cancel not confirmed and connection retired",
    )?;
    require(
        cancel_session
            .execute("SELECT 1", CommitPolicy::Auto)
            .await?
            .outcome
            == Outcome::Rejected,
        "cancelled backend reused",
    )?;
    scenarios.push("out_of_band_server_cancel_confirmed_and_backend_retired");

    let disconnected = Session::connect(url, Limits::default()).await?;
    let doomed_pid = text(&sql(&disconnected, "SELECT pg_backend_pid()").await?)[0].clone();
    let pending = disconnected.submit(
        &format!("BEGIN; INSERT INTO {table} VALUES(777); COMMIT; SELECT pg_sleep(30)"),
        CommitPolicy::Auto,
    )?;
    running(&disconnected, pending.key).await?;
    server_sleeping(&observer, &doomed_pid).await?;
    require(
        text(
            &sql(
                &observer,
                &format!("SELECT count(*) FROM {table} WHERE v=777"),
            )
            .await?,
        ) == ["1"],
        "fixture write was not committed before disconnect",
    )?;
    sql(
        &observer,
        &format!("SELECT pg_terminate_backend({doomed_pid})"),
    )
    .await?;
    let unknown = tokio::time::timeout(Duration::from_secs(5), pending.finish()).await??;
    require(
        unknown.outcome == Outcome::OutcomeUnknown
            && unknown.connection_retired
            && unknown.transaction_state == ServerTransactionState::Unknown,
        "disconnect outcome not unknown",
    )?;
    require(
        text(
            &sql(
                &observer,
                &format!("SELECT count(*) FROM {table} WHERE v=777"),
            )
            .await?,
        ) == ["1"],
        "committed write was lost or replayed",
    )?;
    require(
        disconnected
            .execute(
                &format!("INSERT INTO {table} VALUES(999)"),
                CommitPolicy::Auto,
            )
            .await?
            .outcome
            == Outcome::Rejected,
        "broken session accepted queued write",
    )?;
    require(
        text(
            &sql(
                &observer,
                &format!("SELECT count(*) FROM {table} WHERE v=999"),
            )
            .await?,
        ) == ["0"],
        "write replayed",
    )?;
    scenarios.push("disconnect_unknown_and_no_automatic_write_replay");
    sql(&session, &format!("DROP TABLE {table}")).await?;
    Ok(ContractEvidence {
        status: "passed",
        driver: "tokio-postgres 0.7 (exact version in Cargo.lock)",
        server,
        scenarios,
        limitations: vec![
            "transaction status uses a narrow locally patched tokio-postgres ReadyForQuery API",
            "every accepted cancel retires connection and loses temp/transaction state",
            "simple protocol text values, no type roundtrip claim",
            "retained result budget only; driver/transient/one-value memory not bounded",
            "notices separate and capped; no cross-channel ordering guarantee",
            "NoTls isolated local fixture only; no production credentials",
        ],
    })
}
