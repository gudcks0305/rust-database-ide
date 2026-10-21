use dbide_schema::{ddl::*, er::*, explain::*, *};

fn database(identity: &str) -> DatabaseIdentity {
    DatabaseIdentity {
        engine: "postgresql".into(),
        identity: identity.into(),
        name: "fixture".into(),
        server_version: Some("18".into()),
    }
}
fn snapshot(sql: &str) -> SchemaSnapshot {
    import_postgres_ddl(sql, database("fixture-owned"), "public", "test fixture").unwrap()
}
fn empty_like(s: &SchemaSnapshot) -> SchemaSnapshot {
    SchemaSnapshot::empty(
        database("target-owned"),
        Provenance::Live {
            captured_at: "2026-10-21T00:00:00Z".into(),
            generation: 1,
        },
        s.scope.clone(),
    )
}
fn table_mut(s: &mut SchemaSnapshot) -> &mut Table {
    &mut s.schemas[0].tables[0]
}

#[test]
fn json_roundtrip_preserves_unknown_capabilities_and_version_rejected() {
    let mut s =
        snapshot("CREATE TABLE public.t (id bigint PRIMARY KEY, value text DEFAULT '한글');");
    s.schemas[0]
        .unsupported_objects
        .push("sequence public.seq".into());
    table_mut(&mut s)
        .unsupported_features
        .push("row security".into());
    table_mut(&mut s).indexes.push(Index {
        name: "idx".into(),
        columns: vec!["value".into()],
        unique: false,
        method: "btree".into(),
        predicate: Some("value IS NOT NULL".into()),
        expressions: vec![],
        include: vec![],
        constraint_owned: false,
        unsupported_features: vec![],
    });
    assert_eq!(SchemaSnapshot::from_json(&s.to_json().unwrap()).unwrap(), s);
    let invalid = s
        .to_json()
        .unwrap()
        .replace("\"format_version\": 1", "\"format_version\": 2");
    assert!(SchemaSnapshot::from_json(&invalid).is_err());
    let invalid = s.to_json().unwrap().replace(
        "\"complete\": true",
        "\"complete\": true, \"silent_new_feature\": 1",
    );
    assert!(SchemaSnapshot::from_json(&invalid).is_err());
}

#[test]
fn exact_sql_quotes_injection_identifiers_unicode_and_schema_distinctions() {
    let source = snapshot(
        r#"CREATE TABLE "고객"."x"";DROP TABLE t;--" ("id""" bigint NOT NULL, "이름" text DEFAULT 'a''b', CONSTRAINT "PK" PRIMARY KEY ("id""")); CREATE TABLE public."x"";DROP TABLE t;--" (n int);"#,
    );
    let target = empty_like(&source);
    let p = diff(&source, &target).unwrap();
    assert!(!p.has_unsupported());
    assert_eq!(
        p.preview_sql(),
        "CREATE SCHEMA \"public\";\nCREATE SCHEMA \"고객\";\nCREATE TABLE \"public\".\"x\"\";DROP TABLE t;--\" (\"n\" INT);\nCREATE TABLE \"고객\".\"x\"\";DROP TABLE t;--\" (\"id\"\"\" BIGINT NOT NULL, \"이름\" TEXT DEFAULT 'a''b', CONSTRAINT \"PK\" PRIMARY KEY (\"id\"\"\"));"
    );
    assert!(p.verify_against(&source, &target).is_ok());
    assert!(source.schemas[0].tables[0].name.contains("DROP"));
    assert_eq!(quote_ident("a\"b"), "\"a\"\"b\"");
    assert_ne!(
        QualifiedName::new("a.b", "c").id(),
        QualifiedName::new("a", "b.c").id()
    );
}

#[test]
fn type_and_default_fields_cannot_smuggle_statements_or_column_options() {
    for value in [
        "int); DROP TABLE x; --",
        "int, secret text",
        "int DEFAULT 4",
        "int NOT NULL",
    ] {
        assert!(canonical_type(value).is_err(), "{value}");
    }
    for value in ["1; DROP TABLE x", "1 FROM t", "1, 2", "1 AS x"] {
        assert!(canonical_expression(value).is_err(), "{value}");
    }
    assert_eq!(canonical_expression("'a;b'::text").unwrap(), "'a;b'::TEXT");
    assert!(
        canonical_type("numeric(30, 8)[]")
            .unwrap()
            .contains("NUMERIC(30,8)")
    );
}

#[test]
fn diff_idempotent_fresh_recapture_accepted_and_structural_staleness_rejected() {
    let source =
        snapshot("CREATE TABLE public.t (id integer PRIMARY KEY, title text DEFAULT 'x');");
    assert!(diff(&source, &source).unwrap().operations.is_empty());
    let target = empty_like(&source);
    let p = diff(&source, &target).unwrap();
    let mut new_capture = target.clone();
    new_capture.provenance = Provenance::Live {
        captured_at: "later".into(),
        generation: 2,
    };
    assert!(p.verify_target(&new_capture).is_ok());
    assert!(p.verify_against(&source, &new_capture).is_ok());
    assert_ne!(
        target.fingerprint().unwrap(),
        new_capture.fingerprint().unwrap()
    );
    assert_eq!(
        target.content_fingerprint().unwrap(),
        new_capture.content_fingerprint().unwrap()
    );
    new_capture.database.identity = "different-database".into();
    assert!(p.verify_target(&new_capture).is_err());
    let structural = source.clone();
    assert!(p.verify_target(&structural).is_err());
    let mut modified = p.clone();
    modified.operations[0].sql = Some("SELECT 1;".into());
    assert!(modified.verify_against(&source, &target).is_err());
    let mut partial = target.clone();
    partial.complete = false;
    assert!(diff(&source, &partial).is_err());
    let mut wrong_scope = target.clone();
    wrong_scope.scope.push("secret".into());
    assert!(diff(&source, &wrong_scope).is_err());
}

#[test]
fn additive_columns_order_defaults_nullable_destructive_drops_and_type_fail_closed() {
    let target = snapshot(
        "CREATE TABLE public.t (id int PRIMARY KEY, obsolete text, value text NOT NULL DEFAULT 'old');",
    );
    let source = snapshot(
        "CREATE TABLE public.t (id int PRIMARY KEY, value text, last1 bigint DEFAULT 42, last2 text);",
    );
    let p = diff(&source, &target).unwrap();
    assert!(!p.has_unsupported(), "{}", p.preview_sql());
    assert!(p.is_destructive());
    assert!(p.preview_sql().contains("DROP COLUMN \"obsolete\";"));
    assert!(
        p.preview_sql()
            .contains("ALTER COLUMN \"value\" DROP NOT NULL;")
    );
    assert!(
        p.preview_sql()
            .contains("ALTER COLUMN \"value\" DROP DEFAULT;")
    );
    let adds = p
        .operations
        .iter()
        .filter(|o| o.kind == migration::OperationKind::AddColumn)
        .collect::<Vec<_>>();
    assert_eq!(adds.len(), 2);
    assert!(adds[0].sql.as_ref().unwrap().contains("\"last1\""));
    assert!(adds[1].sql.as_ref().unwrap().contains("\"last2\""));
    let mut changed = source.clone();
    table_mut(&mut changed).columns[1].data_type = "integer".into();
    let blocked = diff(&changed, &source).unwrap();
    assert!(blocked.has_unsupported());
    assert!(!blocked.preview_sql().contains(" TYPE "));
    assert!(blocked.verify_against(&changed, &source).is_err());
}

#[test]
fn cycles_detach_before_table_drops_and_create_after_all_keys() {
    let source = snapshot(
        "CREATE TABLE public.a (id int PRIMARY KEY, b int REFERENCES public.b(id)); CREATE TABLE public.b (id int PRIMARY KEY, a int REFERENCES public.a(id));",
    );
    let target = empty_like(&source);
    let p = diff(&source, &target).unwrap();
    assert!(!p.has_unsupported(), "{}", p.preview_sql());
    let last_table = p
        .operations
        .iter()
        .rposition(|o| o.kind == migration::OperationKind::CreateTable)
        .unwrap();
    let first_fk = p
        .operations
        .iter()
        .position(|o| o.kind == migration::OperationKind::AddForeignKey)
        .unwrap();
    assert!(last_table < first_fk);
    let mut actual = source.clone();
    for t in &mut actual.schemas[0].tables {
        for (i, f) in t.foreign_keys.iter_mut().enumerate() {
            f.name = Some(format!("{}_fk_{i}", t.name));
        }
    }
    let drop = diff(&empty_like(&actual), &actual).unwrap();
    assert!(!drop.has_unsupported());
    let last_fk = drop
        .operations
        .iter()
        .rposition(|o| o.kind == migration::OperationKind::DropForeignKey)
        .unwrap();
    let first_table = drop
        .operations
        .iter()
        .position(|o| o.kind == migration::OperationKind::DropTable)
        .unwrap();
    assert!(last_fk < first_table);
    assert!(!drop.preview_sql().contains("CASCADE"));
    let missing_name = diff(&empty_like(&source), &source).unwrap();
    assert!(missing_name.has_unsupported());
}

#[test]
fn ddl_roundtrip_with_cyclic_foreign_keys_and_index() {
    let s = snapshot(
        r#"CREATE SCHEMA "업무"; CREATE TABLE "업무".a (id bigint CONSTRAINT a_pk PRIMARY KEY, b bigint); CREATE TABLE "업무".b (id bigint PRIMARY KEY, a bigint); CREATE UNIQUE INDEX b_a ON "업무".b (a); ALTER TABLE "업무".a ADD CONSTRAINT a_b FOREIGN KEY (b) REFERENCES "업무".b (id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED; ALTER TABLE "업무".b ADD CONSTRAINT b_a_fk FOREIGN KEY (a) REFERENCES "업무".a (id);"#,
    );
    let sql = export_postgres_ddl(&s).unwrap();
    let round = import_postgres_ddl(&sql, s.database.clone(), "public", "test fixture").unwrap();
    assert_eq!(s, round);
    assert!(diff(&round, &s).unwrap().operations.is_empty());
}

#[test]
fn ddl_unsupported_clauses_abort_instead_of_silent_metadata_loss() {
    for sql in [
        "CREATE TABLE t (n int CHECK (n>0));",
        "CREATE TABLE t (n int UNIQUE);",
        "CREATE TEMP TABLE t (n int);",
        "CREATE TABLE t (n int GENERATED ALWAYS AS IDENTITY);",
        "CREATE TABLE t (n int) PARTITION BY RANGE(n);",
        "CREATE TABLE t AS SELECT 1;",
        "CREATE SCHEMA IF NOT EXISTS s;",
        "CREATE TABLE t (n int); CREATE INDEX i ON t (n DESC);",
        "CREATE VIEW v AS SELECT 1;",
        "SET search_path = secret; CREATE TABLE t (n int);",
    ] {
        assert!(
            import_postgres_ddl(sql, database("fixture"), "public", "unsupported").is_err(),
            "{sql}"
        );
    }
}

#[test]
fn er_svg_escape_ids_cycles_and_budgets() {
    let s = snapshot(
        r#"CREATE TABLE public."<script>&" ("<col>" int PRIMARY KEY, parent int REFERENCES public."<script>&"("<col>"));"#,
    );
    let g = graph(&s, ErLimits::default()).unwrap();
    assert_eq!(g.edges[0].from, g.nodes[0].id);
    assert_eq!(g.edges[0].to, g.nodes[0].id);
    assert_eq!(g.edges[0].column_pairs[0].0, g.nodes[0].columns[1].id);
    let svg = export_svg(&s, ErLimits::default()).unwrap();
    assert_eq!(svg, export_svg(&s, ErLimits::default()).unwrap());
    assert!(!svg.contains("<script>"));
    assert!(svg.contains("&lt;script&gt;&amp;"));
    assert!(svg.contains("data-from="));
    let limited = ErLimits {
        max_columns_per_table: 1,
        ..ErLimits::default()
    };
    assert!(graph(&s, limited).unwrap().truncated);
    assert!(export_svg(&s, limited).unwrap().contains("Truncated"));
    assert!(
        export_svg(
            &s,
            ErLimits {
                max_svg_bytes: 20,
                ..ErLimits::default()
            }
        )
        .is_err()
    );
}

#[test]
fn explain_unknown_fields_children_and_execution_semantics_preserved() {
    let raw = r#"[{"Plan":{"Node Type":"Nested Loop","Total Cost":19.5,"Future Node Field":{"x":1},"Plans":[{"Node Type":"Seq Scan","Schema":"업무","Relation Name":"t","Actual Rows":3,"Actual Loops":2}]},"Execution Time":0.12,"JIT":{"Future":true}}]"#;
    let d = parse_postgres_explain(raw, ExecutionMode::PlanOnly).unwrap();
    assert!(d.contains_actual_metrics);
    assert_eq!(d.mode, ExecutionMode::PlanOnly);
    assert_eq!(d.warnings.len(), 1);
    assert_eq!(d.statements[0].plan.children[0].actual_rows, Some(3.0));
    assert_eq!(d.statements[0].plan.raw["Future Node Field"]["x"], 1);
    assert!(d.statements[0].raw["JIT"]["Future"].as_bool().unwrap());
    let d = parse_postgres_explain(
        "[{\"Plan\":{\"Node Type\":\"Result\"}}]",
        ExecutionMode::AnalyzeExecuted,
    )
    .unwrap();
    assert!(!d.contains_actual_metrics);
    assert!(d.warnings[0].contains("executed"));
    for bad in [
        "{}",
        "[]",
        "[{\"Plan\":{}}]",
        "[{\"Plan\":{\"Node Type\":\"Result\",\"Actual Rows\":\"bad\"}}]",
        "[{\"Plan\":{\"Node Type\":\"Result\",\"Plans\":{}}}]",
    ] {
        assert!(parse_postgres_explain(bad, ExecutionMode::Unknown).is_err());
    }
}

#[test]
fn unspecified_desired_constraint_names_match_server_assigned_names_without_churn() {
    let desired = snapshot(
        "CREATE TABLE public.parent(id int PRIMARY KEY); CREATE TABLE public.child(id int PRIMARY KEY, parent_id int REFERENCES public.parent(id));",
    );
    let mut actual = desired.clone();
    for t in &mut actual.schemas[0].tables {
        if let Some(pk) = &mut t.primary_key {
            pk.name = Some(format!("{}_pkey", t.name));
        }
        for fk in &mut t.foreign_keys {
            fk.name = Some(format!("{}_parent_fk", t.name));
        }
    }
    assert!(diff(&desired, &actual).unwrap().operations.is_empty());
    let mut named = desired.clone();
    named.schemas[0].tables[0]
        .primary_key
        .as_mut()
        .unwrap()
        .name = Some("explicit_different_pk".into());
    assert!(!diff(&named, &actual).unwrap().operations.is_empty());
}

#[test]
fn unique_constraint_owned_index_and_unmodeled_object_deletion_fail_closed() {
    let mut desired = snapshot("CREATE TABLE public.t (id int);");
    table_mut(&mut desired).indexes.push(Index {
        name: "t_unique".into(),
        columns: vec!["id".into()],
        unique: true,
        method: "btree".into(),
        predicate: None,
        expressions: vec![],
        include: vec![],
        constraint_owned: true,
        unsupported_features: vec![],
    });
    assert!(
        diff(&desired, &empty_like(&desired))
            .unwrap()
            .has_unsupported()
    );
    let mut actual = desired.clone();
    actual.schemas[0]
        .unsupported_objects
        .push("sequence seq".into());
    assert!(diff(&desired, &actual).unwrap().has_unsupported());
}

#[test]
fn identifier_truncation_and_invalid_catalog_rejected() {
    let mut s = snapshot("CREATE TABLE public.t (id int);");
    table_mut(&mut s).name = "x".repeat(64);
    assert!(s.validate().is_err());
    let mut s = snapshot("CREATE TABLE public.t (id int);");
    table_mut(&mut s)
        .columns
        .push(table_mut(&mut snapshot("CREATE TABLE public.t (id int);")).columns[0].clone());
    assert!(s.validate().is_err());
}

#[test]
fn renamed_unique_index_detaches_its_foreign_key_before_drop() {
    let desired = snapshot(
        "CREATE TABLE public.parent (id int); CREATE UNIQUE INDEX uq_new ON public.parent (id); CREATE TABLE public.child (id int, parent_id int, CONSTRAINT child_parent_fk FOREIGN KEY (parent_id) REFERENCES public.parent(id));",
    );
    let actual = snapshot(
        "CREATE TABLE public.parent (id int); CREATE UNIQUE INDEX uq_old ON public.parent (id); CREATE TABLE public.child (id int, parent_id int, CONSTRAINT child_parent_fk FOREIGN KEY (parent_id) REFERENCES public.parent(id));",
    );
    let p = diff(&desired, &actual).unwrap();
    assert!(!p.has_unsupported(), "{}", p.preview_sql());
    let phase = |kind| p.operations.iter().position(|o| o.kind == kind).unwrap();
    assert!(
        phase(migration::OperationKind::DropForeignKey)
            < phase(migration::OperationKind::DropIndex)
    );
    assert!(
        phase(migration::OperationKind::DropIndex) < phase(migration::OperationKind::CreateIndex)
    );
    assert!(
        phase(migration::OperationKind::CreateIndex)
            < phase(migration::OperationKind::AddForeignKey)
    );
}

#[test]
fn unqualified_fk_uses_explicit_search_path_default_not_owning_table_schema() {
    let s = snapshot(
        "CREATE TABLE public.parent (id int PRIMARY KEY); CREATE TABLE private.child (id int, p int REFERENCES parent(id));",
    );
    let child = s
        .schemas
        .iter()
        .find(|s| s.name == "private")
        .unwrap()
        .tables
        .first()
        .unwrap();
    assert_eq!(
        child.foreign_keys[0].references,
        QualifiedName::new("public", "parent")
    );
    assert!(
        import_postgres_ddl(
            "CREATE TABLE private.t(id int); CREATE INDEX private.i ON private.t(id)",
            database("fixture"),
            "public",
            "test"
        )
        .is_err()
    );
}

#[test]
fn custom_primary_key_backing_index_is_not_silently_generated_as_plain_pk() {
    let mut source = snapshot("CREATE TABLE public.t (id int CONSTRAINT t_pk PRIMARY KEY);");
    table_mut(&mut source).indexes.push(Index {
        name: "t_pk".into(),
        columns: vec!["id".into()],
        unique: true,
        method: "btree".into(),
        predicate: None,
        expressions: vec![],
        include: vec![],
        constraint_owned: true,
        unsupported_features: vec!["custom operator class".into()],
    });
    assert!(
        diff(&source, &empty_like(&source))
            .unwrap()
            .has_unsupported()
    );
}

#[test]
fn fresh_actual_recapture_validates_but_same_database_column_or_scope_changes_reject() {
    let actual = snapshot("CREATE TABLE public.t (id bigint PRIMARY KEY);");
    let mut desired = actual.clone();
    table_mut(&mut desired).columns.push(Column {
        name: "added".into(),
        data_type: "TEXT".into(),
        nullable: true,
        default: None,
        generated: None,
    });
    let plan = diff(&desired, &actual).unwrap();
    let mut fresh = actual.clone();
    fresh.provenance = Provenance::Live {
        captured_at: "fresh-read".into(),
        generation: 99,
    };
    assert!(plan.verify_target(&fresh).is_ok());
    assert!(plan.verify_against(&desired, &fresh).is_ok());
    table_mut(&mut fresh).columns[0].data_type = "numeric(20,0)".into();
    assert!(plan.verify_target(&fresh).is_err());
    let mut changed_scope = actual.clone();
    changed_scope.scope.push("another".into());
    assert!(plan.verify_target(&changed_scope).is_err());
    let mut changed_complete = actual.clone();
    changed_complete.complete = false;
    assert!(plan.verify_target(&changed_complete).is_err());
}

#[test]
fn fresh_collection_order_does_not_change_semantics_or_plan_validation() {
    let source = snapshot(
        "CREATE TABLE z.parent(id int PRIMARY KEY); CREATE TABLE a.child(id int REFERENCES z.parent(id));",
    );
    let target = empty_like(&source);
    let plan = diff(&source, &target).unwrap();
    let mut reordered = source.clone();
    reordered.schemas.reverse();
    reordered.scope.reverse();
    assert_eq!(
        source.content_fingerprint().unwrap(),
        reordered.content_fingerprint().unwrap()
    );
    assert_eq!(
        plan.preview_sql(),
        diff(&reordered, &target).unwrap().preview_sql()
    );
    assert!(plan.verify_against(&reordered, &target).is_ok());
}
