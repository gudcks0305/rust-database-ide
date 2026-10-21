use dbide_sql::*;
const PG: Dialect = Dialect::PostgreSql;
fn catalog() -> Catalog {
    Catalog {
        tables: vec![
            CatalogTable {
                schema: "public".into(),
                name: "users".into(),
                columns: vec!["id".into(), "name".into()],
            },
            CatalogTable {
                schema: "public".into(),
                name: "orders".into(),
                columns: vec!["id".into(), "user_id".into()],
            },
        ],
    }
}
#[test]
fn boundaries_corpus_preserves_literals_and_comments() {
    for sql in [
        "SELECT ';'; SELECT 2;",
        "SELECT E'escaped\\\';semi'; SELECT 2;",
        "SELECT 1 /* ; /* nested ; */ */; -- ;\nSELECT 2;",
        "SELECT $tag$한글; 👋$tag$; SELECT 2;",
        "CREATE FUNCTION f() RETURNS int LANGUAGE plpgsql AS $$ BEGIN RETURN 1; END $$; SELECT f();",
        "CREATE PROCEDURE f() LANGUAGE SQL BEGIN ATOMIC SELECT 1; SELECT 2; END; SELECT 3;",
        "SELECT (SELECT ';'); SELECT 3;",
    ] {
        assert_eq!(statement_ranges(sql, PG).len(), 2, "{sql}");
    }
}
#[test]
fn incomplete_buffers_keep_the_full_tail() {
    for sql in [
        "SELECT 1; SELECT '미완성",
        "SELECT 1; SELECT $$unterminated;body",
        "SELECT 1; /* unfinished",
        "SELECT 1; SELECT (2;",
    ] {
        let ranges = statement_ranges(sql, PG);
        assert_eq!(ranges.len(), 2, "{sql}: {ranges:?}");
        assert_eq!(ranges.last().unwrap().end, sql.len());
        assert!(
            diagnostics(sql, PG, &catalog())
                .iter()
                .any(|d| d.kind == DiagnosticKind::Syntax)
        );
    }
}
#[test]
fn unicode_utf16_roundtrip_rejects_surrogate_midpoint() {
    let sql = "SELECT '한글😊';\nSELECT 2;";
    for (byte, _) in sql.char_indices().chain(std::iter::once((sql.len(), '\0'))) {
        assert_eq!(
            utf16_to_byte(sql, byte_to_utf16(sql, byte).unwrap()),
            Some(byte)
        );
    }
    let emoji = sql.find('😊').unwrap();
    assert_eq!(
        utf16_to_byte(sql, byte_to_utf16(sql, emoji).unwrap() + 1),
        None
    );
    assert_eq!(byte_to_utf16(sql, emoji + 1), None);
    assert_eq!(
        &sql[statement_at_cursor(sql, sql.len(), PG).unwrap()],
        "\nSELECT 2;"
    );
}
#[test]
fn format_preserves_dialect_and_rejects_comment_loss() {
    for dialect in [PG, Dialect::SQLite, Dialect::MySql] {
        assert_eq!(
            format_sql("select  id from users;", dialect).unwrap(),
            "SELECT id FROM users;"
        );
        assert!(format_sql("select /*+ hint */ id from users", dialect).is_err());
    }
}
#[test]
fn binding_distinguishes_ambiguous_tables_columns_and_syntax() {
    assert!(diagnostics("SELECT u.id FROM users u", PG, &catalog()).is_empty());
    assert!(
        diagnostics(
            "SELECT id FROM users u JOIN orders o ON u.id=o.user_id",
            PG,
            &catalog()
        )
        .iter()
        .any(|d| d.message.contains("Ambiguous column"))
    );
    let mut c = catalog();
    c.tables.push(CatalogTable {
        schema: "private".into(),
        name: "users".into(),
        columns: vec!["id".into()],
    });
    assert!(
        diagnostics("SELECT id FROM users", PG, &c)
            .iter()
            .any(|d| d.message.contains("Ambiguous table"))
    );
    assert!(diagnostics("SELECT id FROM public.users", PG, &c).is_empty());
}
#[test]
fn alias_rename_preserves_korean_emoji_comments_and_strings() {
    let sql = "SELECT '한글😊 u.name' AS label, u.id FROM users u -- u.id\nWHERE u.name = 'u.id'";
    let preview = rename(sql, sql.find("u.id").unwrap(), "고객😊", PG, &catalog(), 9).unwrap();
    assert_eq!(preview.edits.len(), 3);
    let output = preview.apply(sql, 9).unwrap();
    assert!(output.contains("'한글😊 u.name'"));
    assert!(output.contains("-- u.id"));
    assert!(output.contains("WHERE \"고객😊\".name = 'u.id'"));
    assert!(preview.apply(sql, 10).is_err());
    assert!(preview.apply(&format!("{sql} "), 9).is_err());
}
#[test]
fn alias_shadowing_rename_touches_only_resolved_scope() {
    let sql = "SELECT u.id FROM users u WHERE EXISTS (SELECT u.id FROM orders u WHERE u.id = 1) AND u.id=2";
    let p = rename(
        sql,
        sql.find("u.id").unwrap(),
        "outer_user",
        PG,
        &catalog(),
        0,
    )
    .unwrap();
    assert_eq!(p.edits.len(), 3);
    let result = p.apply(sql, 0).unwrap();
    assert!(result.contains("SELECT u.id FROM orders u WHERE u.id = 1"));
}
#[test]
fn nested_ctes_and_navigation_are_source_bound() {
    let sql = "WITH x AS (SELECT id FROM users), y AS (SELECT id FROM x) SELECT y.id FROM y";
    let a = analyze(sql, PG, &catalog());
    assert!(a.diagnostics.is_empty(), "{:?}", a.diagnostics);
    let x = a.symbol_at(sql.rfind(" x").unwrap() + 1).unwrap();
    assert_eq!(x.kind, SymbolKind::Cte);
    assert_eq!(&sql[x.declaration.clone().unwrap()], "x");
    let p = rename(
        sql,
        x.declaration.as_ref().unwrap().start,
        "source_rows",
        PG,
        &catalog(),
        1,
    )
    .unwrap();
    assert_eq!(p.edits.len(), 2);
}
#[test]
fn quoted_identifier_case_is_resolved_without_folding() {
    let c = Catalog {
        tables: vec![CatalogTable {
            schema: "public".into(),
            name: "Users".into(),
            columns: vec!["이름".into()],
        }],
    };
    let sql = "SELECT \"별명\".\"이름\" FROM public.\"Users\" AS \"별명\"";
    assert!(diagnostics(sql, PG, &c).is_empty());
    let p = rename(sql, sql.find("별명").unwrap(), "a\"b", PG, &c, 1).unwrap();
    assert!(p.apply(sql, 1).unwrap().contains("\"a\"\"b\".\"이름\""));
    assert!(!diagnostics("SELECT 이름 FROM Users", PG, &c).is_empty());
}
#[test]
fn completion_uses_alias_columns_and_replaces_partial_word() {
    let sql = "SELECT u.na FROM users u";
    let cursor = sql.find("na FROM").unwrap() + 2;
    let result = completions(sql, cursor, PG, &catalog());
    assert!(
        result.iter().any(|c| c.label == "name"
            && c.kind == CompletionKind::Column
            && &sql[c.replace.clone()] == "na"),
        "{result:?}"
    );
    assert!(
        completions("SELECT * FROM ", 14, PG, &catalog())
            .iter()
            .any(|c| c.kind == CompletionKind::Table)
    );
}
#[test]
fn completion_tolerates_trailing_dot_and_does_not_mutate_source() {
    let sql = String::from("SELECT id FROM users u WHERE u.");
    let before = sql.clone();
    let result = completions(&sql, sql.len(), PG, &catalog());
    assert!(result.iter().any(|c| c.label == "id"), "{result:?}");
    assert_eq!(sql, before);
}
#[test]
fn unsafe_rename_is_rejected_without_mutating_source() {
    for sql in [
        "SELECT id FROM users u JOIN orders o ON u.id=o.id",
        "SELECT u.id FROM missing u",
        "DO $$ BEGIN EXECUTE 'SELECT u.id'; END $$",
        "SELECT * FROM (SELECT id FROM users) u",
        "WITH RECURSIVE x AS (SELECT 1) SELECT * FROM x",
    ] {
        assert!(
            rename(sql, 7, "new_name", PG, &catalog(), 0).is_err(),
            "{sql}"
        );
    }
    assert!(
        rename(
            "SELECT u.id FROM users u JOIN orders o ON u.id=o.id",
            7,
            "o",
            PG,
            &catalog(),
            0
        )
        .is_err()
    );
}
#[test]
fn literals_and_comments_have_no_navigation_symbol() {
    let sql = "SELECT 'u.id' AS text, u.id FROM users u -- u.id";
    let a = analyze(sql, PG, &catalog());
    assert!(a.symbol_at(sql.find("u.id").unwrap()).is_none());
    assert!(a.symbol_at(sql.rfind("u.id").unwrap()).is_none());
    assert!(completions(sql, sql.find("u.id").unwrap() + 1, PG, &catalog()).is_empty());
}
#[test]
fn multiple_statements_bind_aliases_independently() {
    let sql = "SELECT u.id FROM users u; SELECT u.id FROM orders u;";
    let p = rename(sql, 7, "one", PG, &catalog(), 0).unwrap();
    assert_eq!(p.edits.len(), 2);
    assert!(
        p.apply(sql, 0)
            .unwrap()
            .ends_with("SELECT u.id FROM orders u;")
    );
}
#[test]
fn qualified_wildcards_follow_alias_rename() {
    let sql = "SELECT u.* FROM users u";
    let p = rename(sql, 7, "customer", PG, &catalog(), 0).unwrap();
    assert_eq!(p.edits.len(), 2);
    assert_eq!(
        p.apply(sql, 0).unwrap(),
        "SELECT \"customer\".* FROM users \"customer\""
    );
}
#[test]
fn recursive_forward_ctes_and_unbound_identifier_positions_reject_rename() {
    for sql in [
        "WITH x AS (SELECT id FROM x) SELECT x.id FROM x",
        "WITH x AS (SELECT id FROM y), y AS (SELECT id FROM users) SELECT id FROM x",
        "SELECT u.id FROM users u FOR UPDATE OF u",
        "SELECT u(id) FROM users u",
    ] {
        assert!(
            rename(
                sql,
                sql.rfind(" u").map(|i| i + 1).unwrap_or(5),
                "renamed",
                PG,
                &catalog(),
                1
            )
            .is_err(),
            "{sql}"
        );
    }
}
#[test]
fn conditional_procedure_keeps_safe_unsplit_tail() {
    let sql = "CREATE PROCEDURE p() BEGIN IF 1 THEN SELECT 1; END IF; SELECT 2; END; SELECT 3;";
    assert_eq!(statement_ranges(sql, Dialect::MySql), vec![0..sql.len()]);
    assert!(statement_at_cursor(sql, 0, Dialect::MySql).is_none());
}
#[test]
fn ambiguous_columns_are_not_offered_as_unqualified_completions() {
    let sql = "SELECT i FROM users u JOIN orders o ON u.id=o.user_id";
    let cs = completions(sql, 8, PG, &catalog());
    assert!(
        !cs.iter()
            .any(|c| c.kind == CompletionKind::Column && c.label == "id")
    );
}
#[test]
fn cte_completion_respects_query_scope() {
    let sql = "WITH x AS (SELECT id FROM users) SELECT * FROM ";
    assert!(
        completions(sql, sql.len(), PG, &catalog())
            .iter()
            .any(|c| c.label == "x")
    );
    let sql = "SELECT id FROM users WHERE EXISTS (WITH hidden AS (SELECT id FROM users) SELECT id FROM hidden); SELECT * FROM ";
    assert!(
        !completions(sql, sql.len(), PG, &catalog())
            .iter()
            .any(|c| c.label == "hidden")
    );
}
#[test]
fn nested_cte_shadowing_keeps_outer_rename_local() {
    let sql = "WITH x AS (SELECT id FROM users) SELECT x.id FROM x WHERE EXISTS (WITH x AS (SELECT user_id AS id FROM orders) SELECT x.id FROM x)";
    let a = analyze(sql, PG, &catalog());
    assert!(a.diagnostics.is_empty(), "{:?}", a.diagnostics);
    let p = rename(sql, 5, "outer_rows", PG, &catalog(), 0).unwrap();
    assert_eq!(p.edits.len(), 3);
    assert!(
        p.apply(sql, 0)
            .unwrap()
            .contains("WITH x AS (SELECT user_id AS id FROM orders) SELECT x.id FROM x")
    );
}
#[test]
fn qualified_table_completion_filters_on_table_prefix() {
    let sql = "SELECT * FROM us";
    assert!(
        completions(sql, sql.len(), PG, &catalog())
            .iter()
            .any(|c| c.kind == CompletionKind::Table && c.label.contains("users"))
    );
}
#[test]
fn duplicate_ctes_and_vendor_join_semantics_are_diagnostics() {
    for sql in [
        "WITH x AS (SELECT id FROM users), x AS (SELECT id FROM orders) SELECT id FROM x",
        "SELECT * FROM users u JOIN orders o USING(id)",
        "SELECT * FROM users u NATURAL JOIN orders o",
        "SELECT u.id FROM users u UNION SELECT o.id FROM orders o",
    ] {
        assert!(!diagnostics(sql, PG, &catalog()).is_empty(), "{sql}");
    }
}
#[test]
fn explain_trigger_never_exposes_body_delete_as_top_level_sql() {
    let sql = "EXPLAIN CREATE TRIGGER tr AFTER INSERT ON t BEGIN INSERT INTO audit VALUES(1); DELETE FROM secrets; END; SELECT 42;";
    let cursor = sql.find("DELETE").unwrap();
    assert!(statement_at_cursor(sql, cursor, Dialect::SQLite).is_none());
    let ranges = statement_ranges(sql, Dialect::SQLite);
    assert_eq!(ranges.len(), 2);
    assert!(sql[ranges[0].clone()].starts_with("EXPLAIN CREATE TRIGGER"));
    assert!(sql[ranges[0].clone()].contains("DELETE FROM secrets"));
    assert_eq!(
        sql[statement_at_cursor(sql, sql.find("SELECT 42").unwrap(), Dialect::SQLite).unwrap()]
            .trim(),
        "SELECT 42;"
    );
}
#[test]
fn mysql_event_and_explain_wrappers_never_expose_procedural_body() {
    for sql in [
        "CREATE EVENT e ON SCHEDULE EVERY 1 DAY DO BEGIN INSERT INTO audit VALUES(1); DELETE FROM secrets; END; SELECT 42;",
        "EXPLAIN CREATE EVENT e ON SCHEDULE EVERY 1 DAY DO BEGIN INSERT INTO audit VALUES(1); DELETE FROM secrets; END; SELECT 42;",
        "EXPLAIN QUERY PLAN CREATE TEMP TRIGGER tr AFTER INSERT ON t BEGIN INSERT INTO audit VALUES(1); DELETE FROM secrets; END; SELECT 42;",
        "EXECUTE BLOCK AS BEGIN INSERT INTO audit VALUES(1); DELETE FROM secrets; END; SELECT 42;",
    ] {
        assert!(
            statement_at_cursor(sql, sql.find("DELETE").unwrap(), Dialect::MySql).is_none(),
            "{sql}"
        );
        assert!(
            statement_ranges(sql, Dialect::MySql)[0].contains(&sql.find("DELETE").unwrap()),
            "{sql}"
        );
    }
}
#[test]
fn transaction_begin_is_not_confused_with_a_native_procedure_block() {
    for sql in [
        "BEGIN; SELECT 1; ROLLBACK;",
        "BEGIN TRANSACTION; SELECT 1; ROLLBACK;",
        "BEGIN ISOLATION LEVEL SERIALIZABLE; SELECT 1; ROLLBACK;",
    ] {
        assert_eq!(statement_ranges(sql, PG).len(), 3, "{sql}");
        assert_eq!(
            sql[statement_at_cursor(sql, sql.find("SELECT").unwrap(), PG).unwrap()].trim(),
            "SELECT 1;"
        );
    }
}
#[test]
fn unknown_or_anonymous_native_blocks_fail_closed_at_current_statement() {
    for sql in [
        "BEGIN INSERT INTO audit VALUES(1); DELETE FROM secrets; END;",
        "CREATE SOMETHING custom AS BEGIN INSERT INTO audit VALUES(1); DELETE FROM secrets; END;",
        "EXPLAIN CREATE TRIGGER tr AFTER INSERT ON t BEGIN CASE WHEN 1 THEN INSERT INTO audit VALUES(1); END CASE; DELETE FROM secrets; END;",
    ] {
        assert!(
            statement_at_cursor(sql, sql.find("DELETE").unwrap(), PG).is_none(),
            "{sql}"
        );
    }
}
