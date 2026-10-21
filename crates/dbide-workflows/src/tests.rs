use super::*;
use dbide_pg_spike::{ExecutionKey, ServerTransactionState};
use dbide_profiles::ConnectionProfile;
use serde_json::json;

fn request() -> ConnectionRequest {
    ConnectionRequest {
        profile: ConnectionProfile::fixture(),
        password: "fixture-password-not-for-output".into(),
    }
}
fn report(rows: Vec<serde_json::Value>) -> ExecutionReport {
    ExecutionReport {
        key: ExecutionKey {
            session: 1,
            generation: 1,
            execution: 1,
        },
        commit_policy: CommitPolicy::Auto,
        transaction_state: ServerTransactionState::Idle,
        outcome: Outcome::Completed,
        terminal_error: None,
        events: rows
            .into_iter()
            .map(|v| Event::Row {
                values: vec![Some(v.to_string())],
            })
            .collect(),
        notices: vec![],
        retained_bytes: 1000,
        truncated: false,
        connection_retired: false,
        elapsed_micros: 1,
    }
}
fn records() -> Vec<serde_json::Value> {
    vec![
        json!({"kind":"identity","database":"dbide_fixture","version":"18.6","version_num":180006}),
        json!({"kind":"schema","name":"test_schema"}),
        json!({"kind":"relation","schema":"test_schema","name":"parent","relation_kind":"r","relation_oid":1001,"ordinary":true,"definition":null,"features":[]}),
        json!({"kind":"relation","schema":"test_schema","name":"child","relation_kind":"r","relation_oid":1002,"ordinary":true,"definition":null,"features":[]}),
        json!({"kind":"column","schema":"test_schema","table":"parent","column":{"name":"id","data_type":"bigint","nullable":false,"default":null,"generated":null}}),
        json!({"kind":"column","schema":"test_schema","table":"child","column":{"name":"pid","data_type":"bigint","nullable":true,"default":null,"generated":null}}),
        json!({"kind":"primary_key","schema":"test_schema","table":"parent","key":{"name":"parent_pkey","columns":["id"]}}),
        json!({"kind":"foreign_key","schema":"test_schema","table":"child","key":{"name":"child_parent_fk","columns":["pid"],"references":{"schema":"test_schema","name":"parent"},"referenced_columns":["id"],"on_delete":"restrict","on_update":"no_action","deferrable":false,"initially_deferred":false}}),
        json!({"kind":"index","schema":"test_schema","table":"parent","index":{"name":"parent_pkey","columns":["id"],"unique":true,"method":"btree","predicate":null,"expressions":[],"include":[],"constraint_owned":true,"unsupported_features":[]}}),
    ]
}
#[test]
fn catalog_typed_capture_recapture_semantic_fingerprint_and_noop_diff() {
    let request = request();
    let scope = vec!["test_schema".into()];
    let a = snapshot_from_report(
        &request,
        &scope,
        CaptureLimits::default(),
        &report(records()),
    )
    .unwrap();
    let b = snapshot_from_report(
        &request,
        &scope,
        CaptureLimits::default(),
        &report(records()),
    )
    .unwrap();
    assert!(a.complete);
    assert_eq!(
        a.content_fingerprint().unwrap(),
        b.content_fingerprint().unwrap()
    );
    assert_ne!(a.fingerprint().unwrap(), b.fingerprint().unwrap());
    assert!(compare_snapshots(&a, &b).unwrap().operations.is_empty());
    let parent = a.tables().find(|(q, _)| q.name == "parent").unwrap().1;
    assert_eq!(parent.columns[0].data_type, "BIGINT");
    assert_eq!(parent.primary_key.as_ref().unwrap().columns, ["id"]);
    assert!(parent.indexes[0].constraint_owned);
    let child = a.tables().find(|(q, _)| q.name == "child").unwrap().1;
    assert_eq!(
        child.foreign_keys[0].references,
        QualifiedName::new("test_schema", "parent")
    );
    assert!(!a.to_json().unwrap().contains(&request.password));
}
#[test]
fn unsupported_catalog_semantics_are_preserved_and_block_changes() {
    let mut rows = records();
    rows.push(
        json!({"kind":"unsupported_object","schema":"test_schema","description":"sequence: seq"}),
    );
    rows.push(json!({"kind":"index","schema":"test_schema","table":"child","index":{"name":"special_index","columns":["pid"],"unique":false,"method":"btree","predicate":"pid > 0","expressions":[],"include":[],"constraint_owned":false,"unsupported_features":["index sort/null order"]}}));
    let a = snapshot_from_report(
        &request(),
        &["test_schema".into()],
        CaptureLimits::default(),
        &report(rows),
    )
    .unwrap();
    assert_eq!(a.schemas[0].unsupported_objects, ["sequence: seq"]);
    assert_eq!(
        a.schemas[0].tables[0].indexes[0].predicate.as_deref(),
        Some("pid > 0")
    );
    let mut changed = a.clone();
    changed.schemas[0].tables[0]
        .columns
        .push(dbide_schema::Column {
            name: "new_col".into(),
            data_type: "INT".into(),
            nullable: true,
            default: None,
            generated: None,
        });
    assert!(compare_snapshots(&changed, &a).unwrap().has_unsupported());
}
#[test]
fn incomplete_malformed_outofscope_and_caps_return_no_partial_snapshot() {
    let scope = ["test_schema".into()];
    let mut r = report(records());
    r.truncated = true;
    assert!(matches!(
        snapshot_from_report(&request(), &scope, CaptureLimits::default(), &r),
        Err(WorkflowError::Truncated)
    ));
    let l = CaptureLimits {
        max_records: 2,
        ..CaptureLimits::default()
    };
    assert!(matches!(
        snapshot_from_report(&request(), &scope, l, &report(records())),
        Err(WorkflowError::Truncated)
    ));
    let l = CaptureLimits {
        max_columns: 1,
        ..CaptureLimits::default()
    };
    assert!(matches!(
        snapshot_from_report(&request(), &scope, l, &report(records())),
        Err(WorkflowError::Truncated)
    ));
    let l = CaptureLimits {
        max_relations: 1,
        ..CaptureLimits::default()
    };
    assert!(matches!(
        snapshot_from_report(&request(), &scope, l, &report(records())),
        Err(WorkflowError::Truncated)
    ));
    assert!(
        snapshot_from_report(
            &request(),
            &["other".into()],
            CaptureLimits::default(),
            &report(records())
        )
        .is_err()
    );
    assert!(
        snapshot_from_report(
            &request(),
            &scope,
            CaptureLimits::default(),
            &report(vec![])
        )
        .is_err()
    );
    let mut unknown = records();
    unknown[0]["version_num"] = json!(130000);
    assert!(
        snapshot_from_report(
            &request(),
            &scope,
            CaptureLimits::default(),
            &report(unknown)
        )
        .is_err()
    );
}
#[test]
fn safe_scope_literals_and_catalog_script_never_interpolate_identifiers() {
    let scope = validated_scope(&["x'); DROP TABLE t;--\\".into(), "한글".into()]).unwrap();
    let sql = capture_sql(&scope, CaptureLimits::default());
    assert!(sql.contains("E'x''); DROP TABLE t;--\\\\'"));
    let statements = Parser::parse_sql(&PostgreSqlDialect {}, &sql).unwrap();
    assert!(
        !statements
            .iter()
            .any(|s| matches!(s, Statement::Drop { .. }))
    );
    assert!(sql.starts_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;"));
    assert!(sql.ends_with("ROLLBACK;\n"));
    for scope in [
        vec![],
        vec!["pg_catalog".into()],
        vec!["pg_user".into()],
        vec!["information_schema".into()],
        vec!["a".into(), "a".into()],
        vec!["\0".into()],
        vec!["a".repeat(64)],
    ] {
        assert!(validated_scope(&scope).is_err());
    }
}
#[test]
fn explain_canonical_single_statement_readonly_and_no_analyze() {
    let limits = CaptureLimits::default();
    let sql = plan_only_sql(
        "SELECT ';DROP TABLE t' AS quoted; -- trailing comment",
        false,
        limits,
    )
    .unwrap();
    let statements = Parser::parse_sql(&PostgreSqlDialect {}, &sql).unwrap();
    assert!(sql.contains("EXPLAIN (FORMAT JSON, COSTS TRUE, VERBOSE TRUE) SELECT"));
    assert!(!sql.contains("trailing comment"));
    assert_eq!(statements.len(), 7);
    assert!(plan_only_sql("SELECT 1; DROP TABLE t;", false, limits).is_err());
    for unsupported in [
        "BEGIN",
        "COPY t TO STDOUT",
        "CREATE TABLE t(id int)",
        "EXPLAIN ANALYZE SELECT 1",
        "CALL f()",
        "PREPARE p AS SELECT 1",
        "EXECUTE p",
        "",
    ] {
        assert!(
            plan_only_sql(unsupported, false, limits).is_err(),
            "{unsupported}"
        );
    }
    assert!(plan_only_sql("SELECT 1", true, limits).is_err());
    assert!(plan_only_sql("UPDATE t SET n=2", false, limits).is_ok());
    assert!(plan_only_sql("DELETE FROM t", false, limits).is_ok());
}
#[tokio::test]
async fn analyze_is_rejected_before_network_access() {
    let mut request = request();
    request.profile.host = "invalid.invalid".into();
    assert!(matches!(
        explain_plan(&request, "SELECT 1", true, CaptureLimits::default()).await,
        Err(WorkflowError::Configuration(_))
    ));
}

#[test]
fn precise_feature_markers_and_column_generation_modes_survive_capture_separately() {
    use metadata_features::*;
    let mut rows = records();
    rows[2]["features"] = json!([COLUMN_COMMENT, CONSTRAINT_CHECK, CONSTRAINT_NOT_NULL]);
    rows[3]["features"] = json!([
        COLUMN_CUSTOM_COLLATION,
        COLUMN_NONDETERMINISTIC_COLLATION,
        PRIMARY_KEY_DEFERRABLE,
        CONSTRAINT_UNVALIDATED,
        CONSTRAINT_NOT_ENFORCED
    ]);
    rows[4]["column"]["generated"] = json!("identity:d");
    rows[5]["column"]["generated"] = json!("generated:v:(1 + 1)");
    let snapshot = snapshot_from_report(
        &request(),
        &["test_schema".into()],
        CaptureLimits::default(),
        &report(rows),
    )
    .unwrap();
    let parent = snapshot
        .tables()
        .find(|(n, _)| n.name == "parent")
        .unwrap()
        .1;
    assert_eq!(parent.columns[0].generated.as_deref(), Some("identity:d"));
    assert!(
        parent
            .unsupported_features
            .iter()
            .any(|m| m == COLUMN_IDENTITY)
    );
    assert!(
        parent
            .unsupported_features
            .iter()
            .any(|m| m == COLUMN_COMMENT)
    );
    assert!(
        !parent
            .unsupported_features
            .iter()
            .any(|m| m == COLUMN_CUSTOM_COLLATION
                || m == COLUMN_NONDETERMINISTIC_COLLATION
                || m == CONSTRAINT_UNVALIDATED)
    );
    let child = snapshot
        .tables()
        .find(|(n, _)| n.name == "child")
        .unwrap()
        .1;
    assert_eq!(
        child.columns[0].generated.as_deref(),
        Some("generated:v:(1 + 1)")
    );
    assert!(
        child
            .unsupported_features
            .iter()
            .any(|m| m == COLUMN_GENERATED)
    );
    assert!(
        !child
            .unsupported_features
            .iter()
            .any(|m| m == COLUMN_IDENTITY || m == COLUMN_COMMENT || m == CONSTRAINT_CHECK)
    );
    assert!(
        child
            .unsupported_features
            .iter()
            .any(|m| m == PRIMARY_KEY_DEFERRABLE)
    );
    assert!(
        child
            .unsupported_features
            .iter()
            .any(|m| m == CONSTRAINT_NOT_ENFORCED)
    );
    assert!(
        child
            .unsupported_features
            .iter()
            .all(|m| !m.contains("identity/generated") && !m.contains("CHECK/UNIQUE"))
    );
}

#[test]
fn exact_edit_relation_identity_is_atomic_positive_and_outside_schema_fingerprint() {
    let scope = ["test_schema".into()];
    let capture = |rows| {
        captured_catalog_from_report(&request(), &scope, CaptureLimits::default(), &report(rows))
            .unwrap()
    };
    let first = capture(records())
        .for_edit(&QualifiedName::new("test_schema", "parent"))
        .unwrap();
    assert_eq!(first.relation_oid, 1001);
    assert!(
        first
            .snapshot
            .tables()
            .any(|(n, _)| n == QualifiedName::new("test_schema", "parent"))
    );
    let mut replaced = records();
    replaced[2]["relation_oid"] = json!(9001);
    let replacement = capture(replaced)
        .for_edit(&QualifiedName::new("test_schema", "parent"))
        .unwrap();
    assert_eq!(replacement.relation_oid, 9001);
    assert_eq!(
        first.snapshot.content_fingerprint().unwrap(),
        replacement.snapshot.content_fingerprint().unwrap()
    );
    assert!(
        capture(records())
            .for_edit(&QualifiedName::new("other_schema", "parent"))
            .is_err()
    );
    assert!(
        capture(records())
            .for_edit(&QualifiedName::new("test_schema", "missing"))
            .is_err()
    );
    let mut inherited = records();
    inherited[2]["ordinary"] = json!(false);
    assert!(
        capture(inherited)
            .for_edit(&QualifiedName::new("test_schema", "parent"))
            .is_err()
    );
    let mut foreign = records();
    foreign[2]["relation_kind"] = json!("f");
    assert!(
        capture(foreign)
            .for_edit(&QualifiedName::new("test_schema", "parent"))
            .is_err()
    );
    let mut zero = records();
    zero[2]["relation_oid"] = json!(0);
    assert!(
        captured_catalog_from_report(&request(), &scope, CaptureLimits::default(), &report(zero))
            .is_err()
    );
    let mut duplicated = records();
    duplicated[3]["relation_oid"] = json!(1001);
    assert!(
        captured_catalog_from_report(
            &request(),
            &scope,
            CaptureLimits::default(),
            &report(duplicated)
        )
        .is_err()
    );
}

#[tokio::test]
async fn invalid_edit_identity_names_are_rejected_before_network() {
    let mut request = request();
    request.profile.host = "invalid.invalid".into();
    for (schema, table) in [
        ("pg_catalog", "t"),
        ("test_schema", ""),
        ("test_schema", "bad\0table"),
    ] {
        assert!(matches!(
            capture_table_for_edit(&request, schema, table, CaptureLimits::default()).await,
            Err(WorkflowError::Configuration(_))
        ));
    }
}

#[test]
fn edit_metadata_hash_uses_shared_pg_helper_and_rejects_partial_or_bad_result() {
    let canonical =
        json!({"relation_oid":1001,"columns":[{"name":"id","type":"bigint"}],"constraints":[]});
    let metadata = report(vec![canonical.clone()]);
    let expected = dbide_pg_spike::table_metadata_fingerprint(&canonical.to_string());
    assert_eq!(
        metadata_fingerprint_from_report(&metadata).unwrap(),
        expected
    );
    let altered = report(vec![
        json!({"relation_oid":1001,"columns":[{"name":"id","type":"text"}],"constraints":[]}),
    ]);
    assert_ne!(
        metadata_fingerprint_from_report(&metadata).unwrap(),
        metadata_fingerprint_from_report(&altered).unwrap()
    );
    assert!(metadata_fingerprint_from_report(&report(vec![])).is_err());
    assert!(
        metadata_fingerprint_from_report(&report(vec![canonical.clone(), canonical.clone()]))
            .is_err()
    );
    assert!(metadata_fingerprint_from_report(&report(vec![json!([1, 2])])).is_err());
    let mut truncated = report(vec![canonical.clone()]);
    truncated.truncated = true;
    assert!(matches!(
        metadata_fingerprint_from_report(&truncated),
        Err(WorkflowError::Truncated)
    ));
    let mut missing = report(vec![canonical]);
    missing.events = vec![Event::Row { values: vec![None] }];
    assert!(metadata_fingerprint_from_report(&missing).is_err());
    let oversized = report(vec![
        json!({"large":"x".repeat(dbide_pg_spike::MAX_TABLE_METADATA_BYTES)}),
    ]);
    assert!(matches!(
        metadata_fingerprint_from_report(&oversized),
        Err(WorkflowError::Truncated)
    ));
}

#[test]
fn edit_catalog_phase_keeps_transaction_open_for_shared_metadata_query() {
    let scope = vec!["test_schema".into()];
    let full = capture_sql(&scope, CaptureLimits::default());
    let open = open_capture_sql(&scope, CaptureLimits::default()).unwrap();
    let full_statements = Parser::parse_sql(&PostgreSqlDialect {}, &full).unwrap();
    let open_statements = Parser::parse_sql(&PostgreSqlDialect {}, &open).unwrap();
    assert_eq!(full_statements.len(), open_statements.len() + 1);
    assert!(matches!(
        full_statements.last(),
        Some(Statement::Rollback { .. })
    ));
    assert!(!matches!(
        open_statements.last(),
        Some(Statement::Rollback { .. })
    ));
    assert!(open.starts_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;"));
}

#[test]
fn target_sql_is_exact_relation_bounded_and_keeps_all_namespace_incoming_checks() {
    let scope = vec!["schema__LIMIT__'\\;--".into()];
    let target = "table__SCOPE__'\\;DROP TABLE other;--";
    let sql = open_target_capture_sql(&scope, target, CaptureLimits::default()).unwrap();
    let statements = Parser::parse_sql(&PostgreSqlDialect {}, &sql).unwrap();
    assert_eq!(statements.len(), 12); // BEGIN + four SETs + seven metadata SELECTs.
    assert!(
        !statements
            .iter()
            .any(|s| matches!(s, Statement::Drop { .. }))
    );
    assert_eq!(sql.matches("AND (c.relname = ").count(), 5);
    assert!(sql.contains(&format!("c.relname = {}", sql_literal(target))));
    assert!(sql.contains(&sql_literal(&scope[0])));
    assert!(!sql.contains("'unsupported_object'"));
    assert!(sql.contains("CASE WHEN TRUE AND EXISTS(SELECT 1 FROM pg_catalog.pg_constraint fk WHERE fk.contype='f' AND fk.confrelid=c.oid"));
    assert!(sql.contains("fk.confdeltype IN ('c','n','d') OR fk.confupdtype IN ('c','n','d')"));
    assert!(sql.contains("'incoming foreign key outside scope'"));
    let full = capture_sql(&scope, CaptureLimits::default());
    assert!(full.contains("'unsupported_object'"));
    assert!(full.contains("CASE WHEN FALSE AND EXISTS"));
    assert!(full.contains(&sql_literal(&scope[0])));
    assert_eq!(
        Parser::parse_sql(&PostgreSqlDialect {}, &full)
            .unwrap()
            .len(),
        14
    );
}

#[test]
fn target_snapshot_is_explicitly_incomplete_schema_and_preserves_outgoing_fk() {
    let rows = records()
        .into_iter()
        .filter(|r| match r["kind"].as_str().unwrap() {
            "identity" | "schema" => true,
            "relation" => r["name"] == "child",
            _ => r["table"] == "child",
        })
        .collect::<Vec<_>>();
    let captured = captured_catalog_from_report_with_extent(
        &request(),
        &["test_schema".into()],
        CaptureLimits {
            max_relations: 1,
            max_columns: 1,
            max_records: 8,
            ..CaptureLimits::default()
        },
        &report(rows),
        CaptureExtent::TargetTable,
    )
    .unwrap()
    .for_edit(&QualifiedName::new("test_schema", "child"))
    .unwrap();
    assert!(!captured.snapshot.complete);
    assert_eq!(captured.snapshot.tables().count(), 1);
    let child = captured.snapshot.tables().next().unwrap().1;
    assert_eq!(
        child.foreign_keys[0].references,
        QualifiedName::new("test_schema", "parent")
    );
    assert_eq!(child.foreign_keys[0].columns, ["pid"]);
    assert_eq!(child.foreign_keys[0].referenced_columns, ["id"]);
    assert!(dbide_schema::diff(&captured.snapshot, &captured.snapshot).is_err());
    assert!(
        !SchemaSnapshot::from_json(&captured.snapshot.to_json().unwrap())
            .unwrap()
            .complete
    );
}
