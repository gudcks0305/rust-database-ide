//! Opt-in owned-fixture checks for the UI-independent per-console session router.
//! Live writes are TEMP-only; no saved profiles, user databases or persistent DDL.
use dbide_bridge::{
    BridgeEvent, CancelTarget, CommitPolicy, OperationId, ServerTransactionState as Tx,
};
use dbide_pg_spike::{Event, ExecutionReport, Outcome};
use dbide_profiles::{ConnectionProfile, ConnectionRequest, StoredExecutionTarget};
use dbide_ui_spike::{
    console_session::{ConsoleEffect, ConsoleSession, SessionPhase},
    workbench_model::{ResultOwner, profile_target},
};
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(15);

fn request(database: &str) -> ConnectionRequest {
    assert!(matches!(database, "dbide_fixture" | "dbide_console_b"));
    let mut profile = ConnectionProfile::fixture();
    profile.database = database.into();
    profile.connect_timeout_secs = 3;
    assert_eq!(profile.host, "127.0.0.1");
    assert_eq!(profile.port, 55439);
    ConnectionRequest {
        profile,
        password: "dbide_fixture_only".into(),
    }
}

fn owner(session: &ConsoleSession, title: &str) -> ResultOwner {
    let profile = session.target().unwrap();
    ResultOwner {
        document_id: session.route().console_id,
        document_title: title.into(),
        profile_id: profile.id.clone(),
        profile_name: profile.name.clone(),
        target: profile_target(profile),
    }
}

struct Finished {
    owner: ResultOwner,
    sql: Option<String>,
    report: ExecutionReport,
    target: Option<StoredExecutionTarget>,
}

fn poll(session: &mut ConsoleSession, finished: &mut Vec<Finished>) -> bool {
    let Some(effects) = session.poll_one() else {
        return false;
    };
    for effect in effects {
        let ConsoleEffect::Finished {
            owner,
            sql,
            report,
            target,
        } = effect;
        finished.push(Finished {
            owner,
            sql,
            report,
            target,
        });
    }
    true
}

fn expired(start: Instant, a: &ConsoleSession, b: &ConsoleSession) {
    assert!(
        start.elapsed() < DEADLINE,
        "console deadline: A={:?}/{:?}, busy={}, status={}; B={:?}/{:?}, busy={}, status={}",
        a.phase,
        a.transaction_state,
        a.busy(),
        a.status,
        b.phase,
        b.transaction_state,
        b.busy(),
        b.status
    );
}

/// Poll inactive consoles too, and finish metadata/policy work after the SQL terminal event.
fn drain(a: &mut ConsoleSession, b: &mut ConsoleSession) -> (Vec<Finished>, Vec<Finished>) {
    let start = Instant::now();
    let (mut left, mut right) = (vec![], vec![]);
    loop {
        let progressed = poll(a, &mut left) | poll(b, &mut right);
        if !a.busy() && !b.busy() {
            return (left, right);
        }
        expired(start, a, b);
        if !progressed {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// Stop immediately at Executing, even if its terminal event is already queued.
fn running(session: &mut ConsoleSession) {
    let start = Instant::now();
    let mut premature = vec![];
    while session.active_execution.is_none() {
        poll(session, &mut premature);
        assert!(premature.is_empty(), "terminal delivered before Executing");
        assert!(
            start.elapsed() < DEADLINE,
            "execution did not start: {}",
            session.status
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn one(mut reports: Vec<Finished>) -> Finished {
    assert_eq!(
        reports.len(),
        1,
        "one effect per submitted SQL/control operation"
    );
    reports.pop().unwrap()
}

fn rows(report: &ExecutionReport) -> Vec<Vec<Option<String>>> {
    report
        .events
        .iter()
        .filter_map(|event| match event {
            Event::Row { values } => Some(values.clone()),
            _ => None,
        })
        .collect()
}

fn captured(result: &Finished, expected: &ResultOwner, sql: Option<&str>, database: &str) {
    assert_eq!(result.owner.document_id, expected.document_id);
    assert_eq!(result.owner.document_title, expected.document_title);
    assert_eq!(result.owner.profile_id, expected.profile_id);
    assert_eq!(result.owner.profile_name, expected.profile_name);
    assert_eq!(result.owner.target, expected.target);
    assert_eq!(result.sql.as_deref(), sql);
    let target = result
        .target
        .as_ref()
        .expect("history target must be captured at dispatch");
    assert_eq!(target.profile_id, expected.profile_id);
    assert_eq!(target.database, database);
    let profile = request(database).profile;
    assert_eq!(
        target,
        &StoredExecutionTarget::capture(&profile, database).unwrap()
    );
}

#[test]
fn live_transaction_states_and_old_generations_cannot_retarget_or_allocate() {
    let mut console = ConsoleSession::new(101);
    let obsolete = console.route();
    console.bind(obsolete, request("dbide_fixture")).unwrap();
    let bound = console.route();
    assert_ne!(bound, obsolete);
    assert!(!console.has_worker());
    assert!(
        console
            .execute(obsolete, "SELECT 1".into(), owner(&console, "old"), true)
            .is_err()
    );
    assert!(!console.has_worker());
    assert!(
        console.connect(bound, false).is_err(),
        "worker cap must fail before allocation"
    );
    assert!(!console.has_worker());
    for state in [Tx::Active, Tx::Failed, Tx::Unknown] {
        console.phase = SessionPhase::Connected;
        console.transaction_state = state;
        assert!(
            console.bind(bound, request("dbide_console_b")).is_err(),
            "{state:?}"
        );
        assert_eq!(console.route(), bound);
        assert_eq!(console.target().unwrap().database, "dbide_fixture");
        assert!(!console.has_worker());
    }
}

#[test]
fn equal_operation_ids_do_not_authorize_foreign_console_events_or_cancellation() {
    let mut a = ConsoleSession::new(101);
    let mut b = ConsoleSession::new(202);
    a.active_operation = Some(OperationId(2));
    b.active_operation = Some(OperationId(2));
    a.phase = SessionPhase::Executing;
    b.phase = SessionPhase::Executing;
    let key = dbide_pg_spike::ExecutionKey {
        session: 99,
        generation: 1,
        execution: 1,
    };
    assert!(
        b.handle_event(
            a.route(),
            BridgeEvent::Executing {
                operation: OperationId(2),
                execution: key
            }
        )
        .is_empty()
    );
    assert!(b.active_execution.is_none());
    assert!(
        b.cancel(a.route(), CancelTarget::Operation(OperationId(2)))
            .is_err()
    );
    assert!(
        a.cancel(b.route(), CancelTarget::Operation(OperationId(2)))
            .is_err()
    );
    assert!(!a.has_worker() && !b.has_worker());
}

#[test]
fn foreign_transaction_owner_is_rejected_before_bridge_access() {
    let mut console = ConsoleSession::new(101);
    console
        .bind(console.route(), request("dbide_fixture"))
        .unwrap();
    console.phase = SessionPhase::Connected;
    console.transaction_state = Tx::Active;
    let mut foreign = owner(&console, "Other console's transaction control");
    foreign.document_id = 202;
    for commit in [true, false] {
        let error = console
            .transaction(console.route(), commit, foreign.clone())
            .unwrap_err();
        assert_eq!(error, "Transaction owner does not match this console");
        assert_eq!(console.transaction_state, Tx::Active);
        assert_eq!(console.phase, SessionPhase::Connected);
        assert!(console.active_operation.is_none());
        assert!(!console.has_worker());
    }
}

#[test]
#[ignore = "requires DBIDE_BRIDGE_FIXTURES=owned-loopback and owned PostgreSQL databases dbide_fixture/dbide_console_b at127.0.0.1:55439"]
fn owned_postgres_consoles_keep_temp_transaction_result_and_cancel_contexts_separate() {
    assert_eq!(
        std::env::var("DBIDE_BRIDGE_FIXTURES").as_deref(),
        Ok("owned-loopback"),
        "explicit opt-in required; never use saved profiles or user databases"
    );
    let (mut a, mut b) = (ConsoleSession::new(101), ConsoleSession::new(202));
    a.bind(a.route(), request("dbide_fixture")).unwrap();
    b.bind(b.route(), request("dbide_fixture")).unwrap();
    assert!(!a.has_worker() && !b.has_worker());
    let (owner_a, owner_b) = (owner(&a, "Console A"), owner(&b, "Console B initial"));
    let setup = "CREATE TEMP TABLE dbide_console_temp(v text); INSERT INTO dbide_console_temp VALUES('committed'); SELECT current_database(), pg_backend_pid()::text";
    let initial_b = "SELECT current_database(), pg_backend_pid()::text";
    a.execute(a.route(), setup.into(), owner_a.clone(), true)
        .unwrap();
    b.execute(b.route(), initial_b.into(), owner_b.clone(), true)
        .unwrap();
    running(&mut a);
    running(&mut b);
    let (a_key, old_b_key) = (a.active_execution.unwrap(), b.active_execution.unwrap());
    let (a_op, old_b_op) = (a.active_operation.unwrap(), b.active_operation.unwrap());
    assert_eq!(
        a_op, old_b_op,
        "independent bridges deliberately reuse operation counters"
    );
    assert_ne!(a_key.session, old_b_key.session);
    assert!(
        b.handle_event(
            a.route(),
            BridgeEvent::Executing {
                operation: a_op,
                execution: a_key
            }
        )
        .is_empty()
    );
    assert_eq!(b.active_execution, Some(old_b_key));
    assert!(b.cancel(a.route(), CancelTarget::Operation(a_op)).is_err());
    assert!(b.cancel(b.route(), CancelTarget::Execution(a_key)).is_err());
    let (left, right) = drain(&mut a, &mut b);
    let (initial_a, initial_b_report) = (one(left), one(right));
    captured(&initial_a, &owner_a, Some(setup), "dbide_fixture");
    captured(
        &initial_b_report,
        &owner_b,
        Some(initial_b),
        "dbide_fixture",
    );
    assert_eq!(initial_a.report.outcome, Outcome::Completed);
    assert_eq!(initial_b_report.report.outcome, Outcome::Completed);
    let a_pid = rows(&initial_a.report)[0][1].clone().unwrap();
    let b_pid = rows(&initial_b_report.report)[0][1].clone().unwrap();
    assert_ne!(a_pid, b_pid);

    a.set_policy(a.route(), CommitPolicy::Manual).unwrap();
    let (left, right) = drain(&mut a, &mut b);
    assert!(left.is_empty() && right.is_empty());
    assert_eq!(a.commit_policy, CommitPolicy::Manual);
    let pending = "INSERT INTO dbide_console_temp VALUES('pending'); SELECT count(*)::text FROM dbide_console_temp";
    a.execute(a.route(), pending.into(), owner_a.clone(), true)
        .unwrap();
    let (left, right) = drain(&mut a, &mut b);
    assert!(right.is_empty());
    let pending_report = one(left);
    captured(&pending_report, &owner_a, Some(pending), "dbide_fixture");
    assert_eq!(pending_report.report.transaction_state, Tx::Active);
    assert_eq!(rows(&pending_report.report), vec![vec![Some("2".into())]]);
    assert!(a.bind(a.route(), request("dbide_console_b")).is_err());

    let old_b_route = b.route();
    b.bind(old_b_route, request("dbide_console_b")).unwrap();
    assert_ne!(b.route(), old_b_route);
    let owner_b_database = owner(&b, "Console B alternate database");
    assert!(
        b.execute(
            old_b_route,
            "SELECT 999".into(),
            owner_b_database.clone(),
            true
        )
        .is_err()
    );
    let a_read =
        "SELECT current_database(), pg_backend_pid()::text, count(*)::text FROM dbide_console_temp";
    let b_read = "SELECT current_database(), count(*)::text FROM pg_catalog.pg_class WHERE relnamespace=pg_my_temp_schema() AND relname='dbide_console_temp'";
    a.execute(a.route(), a_read.into(), owner_a.clone(), true)
        .unwrap();
    b.execute(b.route(), b_read.into(), owner_b_database.clone(), true)
        .unwrap();
    running(&mut b);
    assert_eq!(
        b.active_operation,
        Some(old_b_op),
        "rebinding creates another bridge with colliding counters"
    );
    let new_b_key = b.active_execution.unwrap();
    assert!(
        b.handle_event(
            old_b_route,
            BridgeEvent::Executing {
                operation: old_b_op,
                execution: old_b_key
            }
        )
        .is_empty()
    );
    assert_eq!(b.active_execution, Some(new_b_key));
    assert!(
        b.cancel(old_b_route, CancelTarget::Operation(old_b_op))
            .is_err()
    );
    assert!(
        b.cancel(b.route(), CancelTarget::Execution(old_b_key))
            .is_err()
    );
    // A has remained inactive/unpolled while B changed targets; its effects still go to A.
    let (left, right) = drain(&mut a, &mut b);
    let (inactive_a, alternate_b) = (one(left), one(right));
    captured(&inactive_a, &owner_a, Some(a_read), "dbide_fixture");
    captured(
        &alternate_b,
        &owner_b_database,
        Some(b_read),
        "dbide_console_b",
    );
    assert_eq!(
        rows(&inactive_a.report),
        vec![vec![
            Some("dbide_fixture".into()),
            Some(a_pid.clone()),
            Some("2".into())
        ]]
    );
    assert_eq!(
        rows(&alternate_b.report),
        vec![vec![Some("dbide_console_b".into()), Some("0".into())]]
    );
    assert_eq!(a.transaction_state, Tx::Active);

    let slow = "SELECT pg_sleep(30)";
    b.execute(b.route(), slow.into(), owner_b_database.clone(), true)
        .unwrap();
    running(&mut b);
    let target = b.cancel_target().unwrap();
    assert!(
        a.cancel(a.route(), target).is_err(),
        "B's key must not cancel A's physical session"
    );
    b.cancel(b.route(), target).unwrap();
    a.execute(a.route(), a_read.into(), owner_a.clone(), true)
        .unwrap();
    let (left, right) = drain(&mut a, &mut b);
    let (unaffected_a, cancelled_b) = (one(left), one(right));
    captured(&unaffected_a, &owner_a, Some(a_read), "dbide_fixture");
    captured(
        &cancelled_b,
        &owner_b_database,
        Some(slow),
        "dbide_console_b",
    );
    assert_eq!(unaffected_a.report.outcome, Outcome::Completed);
    assert_eq!(rows(&unaffected_a.report)[0][1], Some(a_pid.clone()));
    assert_eq!(a.transaction_state, Tx::Active);
    assert!(matches!(
        cancelled_b.report.outcome,
        Outcome::Cancelled | Outcome::OutcomeUnknown
    ));
    assert!(cancelled_b.report.connection_retired);
    assert_eq!(b.phase, SessionPhase::Disconnected);
    assert_eq!(a.target().unwrap().database, "dbide_fixture");
    assert_eq!(b.target().unwrap().database, "dbide_console_b");

    let failed = "SELECT 1/0";
    a.execute(a.route(), failed.into(), owner_a.clone(), true)
        .unwrap();
    let (left, right) = drain(&mut a, &mut b);
    assert!(right.is_empty());
    let failed_report = one(left);
    captured(&failed_report, &owner_a, Some(failed), "dbide_fixture");
    assert_eq!(failed_report.report.outcome, Outcome::Failed);
    assert_eq!(a.transaction_state, Tx::Failed);
    assert!(a.bind(a.route(), request("dbide_console_b")).is_err());
    a.transaction(a.route(), false, owner_a.clone()).unwrap();
    let (left, right) = drain(&mut a, &mut b);
    assert!(right.is_empty());
    let rollback = one(left);
    captured(&rollback, &owner_a, None, "dbide_fixture");
    assert_eq!(rollback.report.outcome, Outcome::Completed);
    assert_eq!(a.transaction_state, Tx::Idle);
    a.set_policy(a.route(), CommitPolicy::Auto).unwrap();
    drain(&mut a, &mut b);
    a.execute(a.route(), a_read.into(), owner_a.clone(), true)
        .unwrap();
    let (left, right) = drain(&mut a, &mut b);
    assert!(right.is_empty());
    let after_rollback = one(left);
    assert_eq!(
        rows(&after_rollback.report),
        vec![vec![
            Some("dbide_fixture".into()),
            Some(a_pid),
            Some("1".into())
        ]]
    );
    assert_eq!(after_rollback.report.outcome, Outcome::Completed);

    // A fresh bridge must honor the policy the console exposes after rebinding.
    // An intentional bind-time reset to Auto is also valid, provided it is explicit in state.
    a.set_policy(a.route(), CommitPolicy::Manual).unwrap();
    let (left, right) = drain(&mut a, &mut b);
    assert!(left.is_empty() && right.is_empty());
    assert_eq!(a.transaction_state, Tx::Idle);
    a.bind(a.route(), request("dbide_console_b")).unwrap();
    let declared_policy = a.commit_policy;
    let rebound_owner = owner(&a, "Console A explicitly rebound");
    let rebound_sql = "SELECT current_database()";
    a.execute(a.route(), rebound_sql.into(), rebound_owner.clone(), true)
        .unwrap();
    let (left, right) = drain(&mut a, &mut b);
    assert!(right.is_empty());
    let rebound = one(left);
    captured(
        &rebound,
        &rebound_owner,
        Some(rebound_sql),
        "dbide_console_b",
    );
    assert_eq!(
        rebound.report.commit_policy, declared_policy,
        "first Run on a rebound lazy bridge silently changed the advertised commit policy"
    );
    assert_eq!(
        rebound.report.transaction_state,
        if declared_policy == CommitPolicy::Manual {
            Tx::Active
        } else {
            Tx::Idle
        }
    );
    assert_eq!(
        rows(&rebound.report),
        vec![vec![Some("dbide_console_b".into())]]
    );
    if a.transaction_state == Tx::Active {
        a.transaction(a.route(), false, rebound_owner).unwrap();
        let (left, right) = drain(&mut a, &mut b);
        assert!(right.is_empty());
        assert_eq!(one(left).report.outcome, Outcome::Completed);
    }
    a.disconnect(a.route()).unwrap();
    b.disconnect(b.route()).unwrap();
    let (left, right) = drain(&mut a, &mut b);
    assert!(left.is_empty() && right.is_empty());
    assert!(!a.has_worker() && !b.has_worker());
    assert_eq!(a.phase, SessionPhase::Disconnected);
    assert_eq!(b.phase, SessionPhase::Disconnected);
}
