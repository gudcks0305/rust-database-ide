use dbide_profiles::{ConnectionProfile, ConnectionRequest};
use dbide_workflows::{CaptureLimits, capture_schema, compare_snapshots, explain_plan};

fn fixture() -> ConnectionRequest {
    let url =
        std::env::var("DBIDE_WORKFLOW_TEST_URL").expect("set explicit owned loopback fixture URL");
    assert_eq!(
        url, "postgresql://postgres:dbide_fixture_only@127.0.0.1:55439/dbide_fixture",
        "workflow live tests only permit the project-owned fixture"
    );
    ConnectionRequest {
        profile: ConnectionProfile::fixture(),
        password: "dbide_fixture_only".into(),
    }
}
#[tokio::test]
#[ignore = "requires explicitly selected project-owned PostgreSQL fixture; performs catalog reads and plan-only EXPLAIN"]
async fn live_capture_recapture_and_explain_without_execution() {
    let request = fixture();
    let scope = vec!["dbide_benchmark".into()];
    let first = capture_schema(&request, &scope, CaptureLimits::default())
        .await
        .unwrap();
    let second = capture_schema(&request, &scope, CaptureLimits::default())
        .await
        .unwrap();
    assert_eq!(
        first.content_fingerprint().unwrap(),
        second.content_fingerprint().unwrap()
    );
    assert_ne!(first.fingerprint().unwrap(), second.fingerprint().unwrap());
    assert!(
        compare_snapshots(&first, &second)
            .unwrap()
            .operations
            .is_empty()
    );
    let (_, customers) = first
        .tables()
        .find(|(name, _)| name.name == "customers")
        .expect("run baseline.sql first");
    assert!(
        customers
            .columns
            .iter()
            .any(|c| c.name == "id" && !c.nullable)
    );
    assert_eq!(customers.primary_key.as_ref().unwrap().columns, ["id"]);
    let plan = explain_plan(
        &request,
        "SELECT id FROM dbide_benchmark.customers WHERE id = 1",
        false,
        CaptureLimits::default(),
    )
    .await
    .unwrap();
    assert!(!plan.contains_actual_metrics);
    assert_eq!(plan.mode, dbide_schema::explain::ExecutionMode::PlanOnly);
    assert!(!plan.statements[0].plan.node_type.is_empty());
    assert!(
        explain_plan(
            &request,
            "DELETE FROM dbide_benchmark.customers",
            true,
            CaptureLimits::default()
        )
        .await
        .is_err()
    );
}

#[tokio::test]
#[ignore = "requires parent to explicitly load crates/dbide-workflows/fixtures/catalog_features.sql into the owned fixture"]
async fn live_composite_fk_and_unsupported_metadata_are_preserved() {
    let snapshot = capture_schema(
        &fixture(),
        &["dbide_workflow_features".into()],
        CaptureLimits::default(),
    )
    .await
    .unwrap();
    let parent = snapshot
        .tables()
        .find(|(n, _)| n.name == "parent")
        .expect("load optional catalog_features.sql")
        .1;
    assert_eq!(parent.primary_key.as_ref().unwrap().columns, ["id", "code"]);
    let child = snapshot
        .tables()
        .find(|(n, _)| n.name == "child")
        .unwrap()
        .1;
    let fk = &child.foreign_keys[0];
    assert_eq!(fk.name.as_deref(), Some("child_parent_fk"));
    assert_eq!(fk.columns, ["parent_id", "parent_code"]);
    assert_eq!(fk.referenced_columns, ["id", "code"]);
    assert!(fk.deferrable && fk.initially_deferred);
    assert_eq!(fk.on_delete, dbide_schema::ReferentialAction::Cascade);
    assert_eq!(fk.on_update, dbide_schema::ReferentialAction::Restrict);
    let special = snapshot
        .tables()
        .find(|(n, _)| n.name == "special")
        .unwrap()
        .1;
    assert!(!special.unsupported_features.is_empty());
    use dbide_workflows::metadata_features::{
        COLUMN_CUSTOM_COLLATION, COLUMN_GENERATED, COLUMN_IDENTITY,
        COLUMN_NONDETERMINISTIC_COLLATION, CONSTRAINT_CHECK,
    };
    assert!(
        special
            .unsupported_features
            .iter()
            .any(|m| m == COLUMN_IDENTITY)
    );
    assert!(
        special
            .unsupported_features
            .iter()
            .any(|m| m == COLUMN_GENERATED)
    );
    assert!(
        special
            .unsupported_features
            .iter()
            .any(|m| m == CONSTRAINT_CHECK)
    );
    assert!(
        !special
            .unsupported_features
            .iter()
            .any(|m| m == COLUMN_CUSTOM_COLLATION || m == COLUMN_NONDETERMINISTIC_COLLATION)
    );
    assert!(
        special
            .columns
            .iter()
            .any(|c| c.generated.as_deref() == Some("identity:a"))
    );
    assert!(special.columns.iter().any(|c| {
        c.generated
            .as_ref()
            .is_some_and(|g| g.starts_with("generated:s:"))
    }));
    assert!(
        special
            .indexes
            .iter()
            .any(|i| !i.expressions.is_empty() && i.predicate.is_some())
    );
    assert!(special.indexes.iter().any(|i| {
        !i.include.is_empty()
            && i.unsupported_features
                .iter()
                .any(|f| f == "index sort/null order")
    }));
    assert!(
        snapshot.schemas[0]
            .views
            .iter()
            .any(|v| v.name == "parent_view")
    );
    assert!(
        snapshot.schemas[0]
            .unsupported_objects
            .iter()
            .any(|o| o.contains("sequence") || o.contains("relation S:"))
    );
}

#[tokio::test]
#[ignore = "requires parent to explicitly load owned catalog_fidelity.sql; ICU is required for nondeterministic collation fixture"]
async fn live_precise_collation_comment_and_constraint_metadata() {
    use dbide_workflows::metadata_features::*;
    let snapshot = capture_schema(
        &fixture(),
        &["dbide_workflow_features".into()],
        CaptureLimits::default(),
    )
    .await
    .unwrap();
    let features = |name: &str| {
        &snapshot
            .tables()
            .find(|(n, _)| n.name == name)
            .expect("load catalog_fidelity.sql first")
            .1
            .unsupported_features
    };
    let has = |name: &str, marker: &str| features(name).iter().any(|f| f == marker);
    assert!(has("fidelity_comments", COLUMN_COMMENT));
    assert!(has("fidelity_comments", CONSTRAINT_CHECK));
    assert!(!has("fidelity_comments", COLUMN_CUSTOM_COLLATION));
    assert!(!has("fidelity_comments", COLUMN_NONDETERMINISTIC_COLLATION));
    assert!(!has("fidelity_comments", PRIMARY_KEY_DEFERRABLE));
    assert!(!has("fidelity_comments", CONSTRAINT_UNVALIDATED));
    assert!(has("fidelity_custom_collation", COLUMN_CUSTOM_COLLATION));
    assert!(!has(
        "fidelity_custom_collation",
        COLUMN_NONDETERMINISTIC_COLLATION
    ));
    assert!(has("fidelity_nondeterministic", COLUMN_CUSTOM_COLLATION));
    assert!(has(
        "fidelity_nondeterministic",
        COLUMN_NONDETERMINISTIC_COLLATION
    ));
    assert!(has("fidelity_deferred", PRIMARY_KEY_DEFERRABLE));
    assert!(has("fidelity_deferred", CONSTRAINT_UNIQUE));
    assert!(has("fidelity_deferred", CONSTRAINT_DEFERRABLE));
    assert!(has("fidelity_unvalidated", CONSTRAINT_CHECK));
    assert!(has("fidelity_unvalidated", CONSTRAINT_UNVALIDATED));
    assert!(!has("fidelity_unvalidated", CONSTRAINT_NOT_ENFORCED));
}

#[tokio::test]
#[ignore = "requires parent/fixture runner to load idempotent catalog_fidelity.sql; reads the dedicated empty table only"]
async fn live_edit_identity_capture_works_for_empty_table_and_rejects_view() {
    use dbide_workflows::capture_table_for_edit;
    let request = fixture();
    let capture = capture_table_for_edit(
        &request,
        "dbide_workflow_features",
        "fidelity_empty_edit",
        CaptureLimits::default(),
    )
    .await
    .unwrap();
    assert!(capture.relation_oid > 0);
    assert!(
        capture
            .snapshot
            .tables()
            .any(|(n, t)| n.schema == "dbide_workflow_features"
                && n.name == "fidelity_empty_edit"
                && t.primary_key.is_some())
    );
    let second = capture_table_for_edit(
        &request,
        "dbide_workflow_features",
        "fidelity_empty_edit",
        CaptureLimits::default(),
    )
    .await
    .unwrap();
    assert_eq!(capture.relation_oid, second.relation_oid);
    assert_eq!(capture.metadata_fingerprint.len(), 64);
    assert!(
        capture
            .metadata_fingerprint
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
    assert_eq!(capture.metadata_fingerprint, second.metadata_fingerprint);
    assert_eq!(
        capture.snapshot.content_fingerprint().unwrap(),
        second.snapshot.content_fingerprint().unwrap()
    );
    assert!(
        capture_table_for_edit(
            &request,
            "dbide_workflow_features",
            "parent_view",
            CaptureLimits::default()
        )
        .await
        .is_err()
    );
    assert!(
        capture_table_for_edit(
            &request,
            "dbide_workflow_features",
            "missing_table",
            CaptureLimits::default()
        )
        .await
        .is_err()
    );
    let session =
        dbide_pg_spike::Session::connect_request(&request, dbide_pg_spike::Limits::default())
            .await
            .unwrap();
    let count=session.execute("BEGIN READ ONLY; SELECT COUNT(*) FROM dbide_workflow_features.fidelity_empty_edit; ROLLBACK;",dbide_pg_spike::CommitPolicy::Auto).await.unwrap();
    session.close();
    assert_eq!(count.outcome, dbide_pg_spike::Outcome::Completed);
    let rows = count
        .events
        .iter()
        .filter_map(|e| {
            if let dbide_pg_spike::Event::Row { values } = e {
                Some(values)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0].as_deref(), Some("0"));
}

#[tokio::test]
#[ignore = "requires existing owned rich/fidelity fixtures; read-only targeted capture under one-relation cap"]
async fn live_target_capture_bypasses_unrelated_schema_caps_without_claiming_full_snapshot() {
    use dbide_workflows::{
        WorkflowError, capture_table_for_edit, capture_target_table_for_edit,
        metadata_features::INCOMING_MUTATING_FOREIGN_KEY,
    };
    let request = fixture();
    let one_relation = CaptureLimits {
        max_relations: 1,
        ..CaptureLimits::default()
    };
    assert!(matches!(
        capture_table_for_edit(
            &request,
            "dbide_workflow_features",
            "fidelity_empty_edit",
            one_relation
        )
        .await,
        Err(WorkflowError::Truncated)
    ));
    let target = capture_target_table_for_edit(
        &request,
        "dbide_workflow_features",
        "fidelity_empty_edit",
        CaptureLimits {
            max_relations: 1,
            max_columns: 2,
            max_records: 8,
            ..CaptureLimits::default()
        },
    )
    .await
    .unwrap();
    assert!(!target.snapshot.complete);
    assert_eq!(target.snapshot.tables().count(), 1);
    assert_eq!(
        target.snapshot.schemas[0].tables[0].name,
        "fidelity_empty_edit"
    );
    assert!(target.snapshot.schemas[0].views.is_empty());
    assert!(target.snapshot.schemas[0].unsupported_objects.is_empty());
    assert!(target.relation_oid > 0 && target.metadata_fingerprint.len() == 64);
    assert!(compare_snapshots(&target.snapshot, &target.snapshot).is_err());
    let full = capture_table_for_edit(
        &request,
        "dbide_workflow_features",
        "fidelity_empty_edit",
        CaptureLimits::default(),
    )
    .await
    .unwrap();
    assert!(full.snapshot.complete);
    assert!(full.snapshot.tables().count() > 1);
    assert_eq!(target.relation_oid, full.relation_oid);
    assert_eq!(target.metadata_fingerprint, full.metadata_fingerprint);
    let parent = capture_target_table_for_edit(
        &request,
        "dbide_workflow_features",
        "parent",
        CaptureLimits {
            max_relations: 1,
            ..CaptureLimits::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(parent.snapshot.tables().count(), 1);
    assert!(
        parent
            .snapshot
            .tables()
            .next()
            .unwrap()
            .1
            .unsupported_features
            .iter()
            .any(|f| f == INCOMING_MUTATING_FOREIGN_KEY)
    );
    let child = capture_target_table_for_edit(
        &request,
        "dbide_workflow_features",
        "child",
        CaptureLimits {
            max_relations: 1,
            ..CaptureLimits::default()
        },
    )
    .await
    .unwrap();
    let fk = &child.snapshot.tables().next().unwrap().1.foreign_keys[0];
    assert_eq!(fk.columns, ["parent_id", "parent_code"]);
    assert_eq!(
        fk.references,
        dbide_schema::QualifiedName::new("dbide_workflow_features", "parent")
    );
    assert_eq!(fk.on_delete, dbide_schema::ReferentialAction::Cascade);
    assert!(fk.deferrable && fk.initially_deferred);
}
