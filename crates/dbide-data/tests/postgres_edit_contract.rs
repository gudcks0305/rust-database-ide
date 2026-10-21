//! Opt-in writes only to a unique schema in the project's disposable loopback fixture.
//! No profile files, external URLs or caller-selected database endpoints are read.
use dbide_data::{
    Cell, Edit, EditColumn, IdentityKind, InsertValue, Limits, PgType, RowIdentity, TableSpec,
    TransactionPlan, plan_edits,
};
use dbide_pg_spike::{
    CommitPolicy, Event, ExecutionReport, Outcome, ParameterizedStatement, ServerTransactionState,
    Session,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const FIXTURE_URL: &str = "postgresql://postgres:dbide_fixture_only@127.0.0.1:55439/dbide_fixture";
fn opt_in() {
    assert_eq!(
        std::env::var("DBIDE_DATA_FIXTURE").as_deref(),
        Ok("owned-loopback"),
        "set DBIDE_DATA_FIXTURE=owned-loopback explicitly; endpoint is fixed to project fixture127.0.0.1:55439"
    );
}
fn limits() -> Limits {
    Limits {
        max_rows: 100,
        max_bytes: 2 * 1024 * 1024,
    }
}
fn text(value: &str) -> Cell {
    Cell::Text(value.into())
}
fn value(value: &str) -> InsertValue {
    InsertValue::Value(text(value))
}
fn statements(plan: TransactionPlan) -> Vec<ParameterizedStatement> {
    assert!(plan.requires_transaction && plan.requires_explicit_submit);
    plan.statements
        .into_iter()
        .map(|statement| ParameterizedStatement {
            sql: statement.sql,
            parameters: statement
                .parameters
                .into_iter()
                .map(|parameter| parameter.value)
                .collect(),
            expected_affected_rows: Some(statement.expected_affected_rows),
        })
        .collect()
}
async fn execute(session: &Session, plan: TransactionPlan) -> ExecutionReport {
    session
        .execute_batch(statements(plan), CommitPolicy::Auto)
        .await
        .unwrap()
}
fn completed(report: &ExecutionReport) {
    assert_eq!(report.outcome, Outcome::Completed, "{report:?}");
    assert_eq!(
        report.transaction_state,
        ServerTransactionState::Idle,
        "{report:?}"
    );
    assert!(report.terminal_error.is_none(), "{report:?}");
    assert!(!report.truncated);
}
async fn sql(session: &Session, query: &str) -> ExecutionReport {
    let report = session.execute(query, CommitPolicy::Auto).await.unwrap();
    completed(&report);
    report
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
fn table(schema: &str) -> TableSpec {
    let columns = [
        ("id", PgType::BigInt, false),
        ("small", PgType::SmallInt, false),
        ("regular", PgType::Integer, false),
        ("big", PgType::BigInt, false),
        ("enabled", PgType::Boolean, false),
        ("amount", PgType::Numeric, false),
        ("payload", PgType::Bytea, false),
        ("uid", PgType::Uuid, false),
        ("day", PgType::Date, false),
        ("moment", PgType::Timestamp, false),
        ("zoned", PgType::TimestampTz, false),
        ("document", PgType::Json, false),
        ("binarydoc", PgType::Jsonb, false),
        ("approx", PgType::DoublePrecision, false),
        ("note", PgType::Text, false),
        ("optional", PgType::Text, true),
        ("empty", PgType::Text, false),
    ]
    .into_iter()
    .map(|(name, pg_type, nullable)| EditColumn {
        name: name.into(),
        pg_type,
        nullable,
    })
    .collect();
    TableSpec {
        schema: schema.into(),
        name: "typed_rows".into(),
        columns,
        identity: Some(RowIdentity {
            kind: IdentityKind::PrimaryKey,
            columns: vec![0],
        }),
    }
}
fn insert(note: InsertValue) -> Edit {
    Edit::InsertDefaulted {
        values: vec![
            InsertValue::Default,
            value("-32768"),
            value("2147483647"),
            value("9223372036854775807"),
            value("t"),
            value("999999999999999999999.0000000000000001"),
            InsertValue::Value(Cell::Binary(vec![0, 1, 255])),
            value("A0EEBC99-9C0B-4EF8-BB6D-6BB9BD380A11"),
            value("2024-02-29"),
            value("2026-10-21 12:34:56.123456"),
            value("2026-10-21T12:34:56.123456+09:00"),
            value(" {\"정확\":123456789012345678901234567890.00001} "),
            value("{\"x\":1,\"z\":\"한글\"}"),
            value("1.2345678901234567"),
            note,
            InsertValue::Value(Cell::Null),
            value(""),
        ],
    }
}
async fn read_rows(observer: &Session, table: &TableSpec) -> Vec<Vec<Cell>> {
    let projection = table
        .columns
        .iter()
        .map(|column| format!("\"{}\"::text", column.name))
        .collect::<Vec<_>>()
        .join(",");
    let query = format!(
        "SELECT {projection} FROM \"{}\".typed_rows ORDER BY id",
        table.schema
    );
    rows(&sql(observer, &query).await)
        .into_iter()
        .map(|row| {
            row.into_iter()
                .enumerate()
                .map(|(index, value)| {
                    if index == 6 {
                        assert_eq!(value.as_deref(), Some("\\x0001ff"));
                        Cell::Binary(vec![0, 1, 255])
                    } else {
                        value.map_or(Cell::Null, Cell::Text)
                    }
                })
                .collect()
        })
        .collect()
}
fn updated(original: &[Cell], note: &str) -> Edit {
    let mut replacement = original.to_vec();
    replacement[14] = text(note);
    Edit::Update {
        original: original.to_vec(),
        replacement,
    }
}
fn conflict(report: &ExecutionReport) {
    assert_eq!(report.outcome, Outcome::Failed, "{report:?}");
    assert_eq!(
        report.transaction_state,
        ServerTransactionState::Idle,
        "{report:?}"
    );
    let error = report.terminal_error.as_ref().unwrap();
    assert!(error.message.starts_with("Conflict:"), "{error:?}");
    assert!(error.sqlstate.is_none());
    assert_eq!(
        report
            .events
            .iter()
            .filter_map(|event| match event {
                Event::CommandComplete { affected_rows } => Some(*affected_rows),
                _ => None,
            })
            .collect::<Vec<_>>(),
        [1, 0]
    );
}
async fn contract(schema: &str, session: &Session, observer: &Session) {
    let table = table(schema);
    sql(
        session,
        &format!(
            "CREATE TABLE \"{schema}\".typed_rows (
        id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
        small smallint NOT NULL, regular integer NOT NULL, big bigint NOT NULL,
        enabled boolean NOT NULL, amount numeric NOT NULL, payload bytea NOT NULL,
        uid uuid NOT NULL, day date NOT NULL, moment timestamp NOT NULL,
        zoned timestamptz NOT NULL, document json NOT NULL, binarydoc jsonb NOT NULL,
        approx double precision NOT NULL, note text NOT NULL DEFAULT 'catalog default',
        optional text, empty text NOT NULL)"
        ),
    )
    .await;
    let plan = plan_edits(
        &table,
        &[insert(InsertValue::Default), insert(value("second"))],
        limits(),
    )
    .unwrap();
    assert!(plan.statements[0].sql.contains("VALUES (DEFAULT, $1::"));
    completed(&execute(session, plan).await);
    let original = read_rows(observer, &table).await;
    assert_eq!(original.len(), 2);
    assert_eq!(original[0][0], text("1"));
    assert_eq!(original[0][1], text("-32768"));
    assert_eq!(original[0][2], text("2147483647"));
    assert_eq!(original[0][3], text("9223372036854775807"));
    assert_eq!(original[0][4], text("true"));
    assert_eq!(
        original[0][5],
        text("999999999999999999999.0000000000000001")
    );
    assert_eq!(original[0][7], text("a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11"));
    assert_eq!(original[0][8], text("2024-02-29"));
    assert_eq!(original[0][9], text("2026-10-21 12:34:56.123456"));
    assert_eq!(
        original[0][11],
        text(" {\"정확\":123456789012345678901234567890.00001} ")
    );
    assert_eq!(original[0][14], text("catalog default"));
    assert_eq!(original[0][15], Cell::Null);
    assert_eq!(original[0][16], text(""));
    let jsonb: serde_json::Value = serde_json::from_str(match &original[0][12] {
        Cell::Text(value) => value,
        _ => panic!("expected jsonb text"),
    })
    .unwrap();
    assert_eq!(jsonb["z"], "한글");
    let injection = "한글🙂'); DROP TABLE must_not_execute; --";
    let mut replacement = original[0].clone();
    replacement[14] = text(injection);
    replacement[15] = text("");
    replacement[11] = text(" {\"정확\":2} ");
    let update = plan_edits(
        &table,
        &[Edit::Update {
            original: original[0].clone(),
            replacement,
        }],
        limits(),
    )
    .unwrap();
    assert!(!update.statements[0].sql.contains(injection));
    assert!(
        update.statements[0]
            .parameters
            .iter()
            .any(|parameter| parameter.value.as_deref()
                == Some(" {\"정확\":123456789012345678901234567890.00001} "))
    );
    completed(&execute(session, update).await);
    let after = read_rows(observer, &table).await;
    assert_eq!(after[0][14], text(injection));
    assert_eq!(after[0][15], text(""));
    assert_eq!(after[0][11], text(" {\"정확\":2} "));
    completed(
        &execute(
            session,
            plan_edits(
                &table,
                &[Edit::Delete {
                    original: after[1].clone(),
                }],
                limits(),
            )
            .unwrap(),
        )
        .await,
    );
    completed(
        &execute(
            session,
            plan_edits(&table, &[insert(value("third"))], limits()).unwrap(),
        )
        .await,
    );
    let snapshots = read_rows(observer, &table).await;
    completed(
        &execute(
            observer,
            plan_edits(&table, &[updated(&snapshots[1], "concurrent")], limits()).unwrap(),
        )
        .await,
    );
    let stale = plan_edits(
        &table,
        &[
            updated(&snapshots[0], "must rollback"),
            updated(&snapshots[1], "stale"),
        ],
        limits(),
    )
    .unwrap();
    conflict(&execute(session, stale).await);
    let after_conflict = read_rows(observer, &table).await;
    assert_eq!(after_conflict[0][14], text(injection));
    assert_eq!(after_conflict[1][14], text("concurrent"));
    completed(
        &execute(
            observer,
            plan_edits(
                &table,
                &[updated(&after_conflict[1], "second concurrent")],
                limits(),
            )
            .unwrap(),
        )
        .await,
    );
    let stale_delete = plan_edits(
        &table,
        &after_conflict
            .iter()
            .cloned()
            .map(|original| Edit::Delete { original })
            .collect::<Vec<_>>(),
        limits(),
    )
    .unwrap();
    conflict(&execute(session, stale_delete).await);
    let after_delete_conflict = read_rows(observer, &table).await;
    assert_eq!(after_delete_conflict.len(), 2);
    assert_eq!(after_delete_conflict[0][14], text(injection));
    assert_eq!(after_delete_conflict[1][14], text("second concurrent"));
    completed(
        &execute(
            session,
            plan_edits(
                &table,
                &after_delete_conflict
                    .into_iter()
                    .map(|original| Edit::Delete { original })
                    .collect::<Vec<_>>(),
                limits(),
            )
            .unwrap(),
        )
        .await,
    );
    assert!(read_rows(observer, &table).await.is_empty());

    // These cannot even produce a dispatchable plan, independent of server contents.
    let mut unverified = table.clone();
    unverified.identity = None;
    assert!(plan_edits(&unverified, &[insert(InsertValue::Default)], limits()).is_err());
    for pg_type in [PgType::DoublePrecision, PgType::TimestampTz, PgType::Jsonb] {
        let mut unsupported = table.clone();
        unsupported.columns[0].pg_type = pg_type;
        assert!(
            plan_edits(&unsupported, &[insert(InsertValue::Default)], limits())
                .unwrap_err()
                .to_string()
                .contains("requires PostgreSQL-equivalent key normalization")
        );
    }
    let mut numeric = TableSpec {
        schema: schema.into(),
        name: "identity_only".into(),
        columns: vec![
            EditColumn {
                name: "id".into(),
                pg_type: PgType::Numeric,
                nullable: false,
            },
            EditColumn {
                name: "note".into(),
                pg_type: PgType::Text,
                nullable: false,
            },
        ],
        identity: Some(RowIdentity {
            kind: IdentityKind::PrimaryKey,
            columns: vec![0],
        }),
    };
    let aliases = [
        Edit::Delete {
            original: vec![text("1"), text("a")],
        },
        Edit::Delete {
            original: vec![text("1.00e0"), text("b")],
        },
    ];
    assert!(
        plan_edits(&numeric, &aliases, limits())
            .unwrap_err()
            .to_string()
            .contains("multiple edits target")
    );
    numeric.columns[0].pg_type = PgType::Uuid;
    let uuid_aliases = [
        Edit::Delete {
            original: vec![text("A0EEBC99-9C0B-4EF8-BB6D-6BB9BD380A11"), text("a")],
        },
        Edit::Delete {
            original: vec![text("a0eebc999c0b4ef8bb6d6bb9bd380a11"), text("b")],
        },
    ];
    assert!(
        plan_edits(&numeric, &uuid_aliases, limits())
            .unwrap_err()
            .to_string()
            .contains("multiple edits target")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicit DBIDE_DATA_FIXTURE=owned-loopback and owned127.0.0.1:55439 fixture"]
async fn generated_edit_plans_match_postgres_scalar_default_and_conflict_contracts() {
    opt_in();
    let owner = Session::connect(FIXTURE_URL, dbide_pg_spike::Limits::default())
        .await
        .unwrap();
    let session = Session::connect(FIXTURE_URL, dbide_pg_spike::Limits::default())
        .await
        .unwrap();
    let observer = Session::connect(FIXTURE_URL, dbide_pg_spike::Limits::default())
        .await
        .unwrap();
    let schema = format!(
        "dbide_data_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    assert!(schema.len() <= 63 && schema.starts_with("dbide_data_"));
    sql(&owner, &format!("CREATE SCHEMA \"{schema}\"")).await;
    let task_schema = schema.clone();
    // Capture panics in a task so cleanup still runs after a contract assertion fails.
    let task_session = session.clone();
    let task_observer = observer.clone();
    let mut worker = tokio::spawn(async move {
        contract(&task_schema, &task_session, &task_observer).await;
    });
    let result = tokio::time::timeout(Duration::from_secs(30), &mut worker).await;
    if result.is_err() {
        worker.abort();
        let _ = worker.await;
    }
    session.close();
    observer.close();
    sql(&owner, &format!("DROP SCHEMA \"{schema}\" CASCADE")).await;
    owner.close();
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        other => panic!("fixture task did not complete: {other:?}"),
    }
}
