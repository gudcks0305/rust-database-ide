//! Conservative AST policy. Parse failures and unsupported syntax fail closed.
use dbide_profiles::DatabaseEngine;
use sqlparser::{
    ast::{
        BinaryOperator, Expr, LimitClause, Query, Select, SetExpr, Statement, TableFactor,
        UnaryOperator, Value, Visit, Visitor,
    },
    dialect::{Dialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect},
    parser::Parser,
};
use std::ops::ControlFlow;

pub const MAX_SQL_BYTES: usize = 64 * 1024;

pub fn dialect(engine: DatabaseEngine) -> Box<dyn Dialect> {
    match engine {
        DatabaseEngine::PostgreSql => Box::new(PostgreSqlDialect {}),
        DatabaseEngine::Sqlite => Box::new(SQLiteDialect {}),
        DatabaseEngine::MySql => Box::new(MySqlDialect {}),
    }
}

/// Return normalized SQL, never the unparsed original (including executable comments).
pub fn read_query(sql: &str, engine: DatabaseEngine) -> Result<String, &'static str> {
    serialize_checked(accepted_query(sql, engine)?.as_ref(), engine)
}

fn accepted_query(sql: &str, engine: DatabaseEngine) -> Result<Box<Query>, &'static str> {
    if sql.is_empty() || sql.len() > MAX_SQL_BYTES {
        return Err("SQL must contain 1..65536 bytes");
    }
    if engine == DatabaseEngine::MySql && sql.contains('\\') {
        return Err(
            "MySQL SQL containing backslashes is unsupported because server SQL modes can change escaping",
        );
    }
    let statements = Parser::parse_sql(dialect(engine).as_ref(), sql)
        .map_err(|_| "SQL could not be parsed in the selected engine dialect")?;
    if statements.len() != 1 || !matches!(statements[0], Statement::Query(_)) {
        return Err("Exactly one SELECT query is required");
    }
    if statements.visit(&mut ReadOnly).is_break() {
        return Err("SQL contains unsupported or potentially side-effecting syntax");
    }
    match statements.into_iter().next().unwrap() {
        Statement::Query(query) => Ok(query),
        _ => unreachable!("single query checked above"),
    }
}

/// Apply the hard row lookahead cap to the root AST without introducing a derived
/// table: derived tables reject/rename duplicate labels on MySQL/SQLite. Nested
/// limits, projection, ORDER BY, set operations and offsets remain untouched.
pub fn limited_read_query(
    sql: &str,
    engine: DatabaseEngine,
    cap: usize,
) -> Result<String, &'static str> {
    let mut query = accepted_query(sql, engine)?;
    if let Some(fetch) = &mut query.fetch {
        if engine != DatabaseEngine::PostgreSql || fetch.percent || fetch.with_ties {
            return Err(
                "query.read supports PostgreSQL FETCH ROWS ONLY, without PERCENT or WITH TIES",
            );
        }
        if matches!(
            &query.limit_clause,
            Some(LimitClause::LimitOffset { limit: Some(_), .. })
                | Some(LimitClause::OffsetCommaLimit { .. })
        ) {
            return Err("Cannot combine LIMIT and FETCH in query.read");
        }
        if let Some(quantity) = &mut fetch.quantity {
            cap_literal(quantity, engine, cap)?;
        }
    } else {
        match &mut query.limit_clause {
            Some(LimitClause::LimitOffset {
                limit, limit_by, ..
            }) => {
                if !limit_by.is_empty() {
                    return Err("LIMIT BY is unsupported in query.read");
                }
                match limit {
                    Some(limit) => cap_literal(limit, engine, cap)?,
                    None => *limit = Some(cap_expr(cap)),
                }
            }
            Some(LimitClause::OffsetCommaLimit { limit, .. }) => cap_literal(limit, engine, cap)?,
            None => {
                query.limit_clause = Some(LimitClause::LimitOffset {
                    limit: Some(cap_expr(cap)),
                    offset: None,
                    limit_by: Vec::new(),
                })
            }
        }
    }
    serialize_checked(&query, engine)
}
/// sqlparser's Display is not a general dialect-aware escaping API. Fail closed
/// unless the exact emitted SQL remains one accepted query with equal semantics.
/// Value spans/attached tokens are intentionally ignored by sqlparser AST Eq.
fn serialize_checked(query: &Query, engine: DatabaseEngine) -> Result<String, &'static str> {
    let sql = query.to_string();
    let reparsed = accepted_query(&sql, engine)
        .map_err(|_| "SQL cannot be serialized safely for the selected dialect")?;
    if query != reparsed.as_ref() {
        return Err("SQL serialization changed query semantics; this syntax is unsupported");
    }
    Ok(sql)
}

fn cap_expr(cap: usize) -> Expr {
    Expr::Value(Value::Number(cap.to_string(), false).into())
}
fn cap_literal(expr: &mut Expr, engine: DatabaseEngine, cap: usize) -> Result<(), &'static str> {
    match expr {
        Expr::Nested(inner)
        | Expr::UnaryOp {
            op: UnaryOperator::Plus,
            expr: inner,
        } => cap_literal(inner, engine, cap),
        Expr::Value(value) => match &value.value {
            Value::Number(number, false) if number.bytes().all(|b| b.is_ascii_digit()) => {
                let number = number
                    .parse::<u128>()
                    .map_err(|_| "LIMIT/FETCH integer is too large")?;
                if number > cap as u128 {
                    *expr = cap_expr(cap);
                }
                Ok(())
            }
            Value::Null if engine == DatabaseEngine::PostgreSql => {
                *expr = cap_expr(cap);
                Ok(())
            }
            _ => Err(
                "query.read LIMIT/FETCH must be an integer literal; expressions are unsupported",
            ),
        },
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr: inner,
        } if engine == DatabaseEngine::Sqlite => {
            if let Expr::Value(value) = inner.as_ref()
                && let Value::Number(number, false) = &value.value
                && !number.is_empty()
                && number.bytes().all(|b| b.is_ascii_digit())
            {
                let number = number
                    .parse::<u128>()
                    .map_err(|_| "LIMIT integer is too large")?;
                *expr = cap_expr(if number == 0 { 0 } else { cap });
                return Ok(());
            }
            Err("query.read LIMIT must be an integer literal; expressions are unsupported")
        }
        _ => Err("query.read LIMIT/FETCH must be an integer literal; expressions are unsupported"),
    }
}

struct ReadOnly;
fn permitted_body(body: &SetExpr) -> bool {
    match body {
        SetExpr::Select(_) | SetExpr::Query(_) => true,
        SetExpr::SetOperation { left, right, .. } => permitted_body(left) && permitted_body(right),
        _ => false,
    }
}
impl Visitor for ReadOnly {
    type Break = ();
    fn pre_visit_statement(&mut self, s: &Statement) -> ControlFlow<()> {
        if matches!(s, Statement::Query(_)) {
            ControlFlow::Continue(())
        } else {
            ControlFlow::Break(())
        }
    }
    fn pre_visit_query(&mut self, q: &Query) -> ControlFlow<()> {
        if !permitted_body(&q.body)
            || !q.locks.is_empty()
            || q.for_clause.is_some()
            || q.settings.is_some()
            || q.format_clause.is_some()
            || !q.pipe_operators.is_empty()
        {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
    fn pre_visit_select(&mut self, s: &Select) -> ControlFlow<()> {
        if s.into.is_some() || !s.optimizer_hints.is_empty() || s.select_modifiers.is_some() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
    fn pre_visit_table_factor(&mut self, t: &TableFactor) -> ControlFlow<()> {
        match t {
            TableFactor::Table { args: None, .. }
            | TableFactor::Derived { .. }
            | TableFactor::NestedJoin { .. } => ControlFlow::Continue(()),
            _ => ControlFlow::Break(()),
        }
    }
    fn pre_visit_expr(&mut self, e: &Expr) -> ControlFlow<()> {
        if let Expr::Function(f) = e {
            // No UDFs, schema-qualified functions, file readers, sleep, session setters,
            // extension loaders, advisory locks, or stored procedures. SQL read-only
            // transactions alone do not constrain those external side effects.
            let name = f.name.to_string().to_ascii_lowercase();
            if !matches!(
                name.as_str(),
                "count"
                    | "sum"
                    | "avg"
                    | "min"
                    | "max"
                    | "coalesce"
                    | "nullif"
                    | "lower"
                    | "upper"
                    | "length"
                    | "char_length"
                    | "abs"
                    | "round"
                    | "trim"
                    | "ltrim"
                    | "rtrim"
                    | "substring"
                    | "substr"
                    | "replace"
                    | "concat"
                    | "row_number"
                    | "rank"
                    | "dense_rank"
                    | "lag"
                    | "lead"
            ) {
                return ControlFlow::Break(());
            }
        }
        // MySQL user-variable assignment is represented as a binary operator.
        if let Expr::BinaryOp { op, .. } = e
            && matches!(
                op,
                BinaryOperator::Assignment | BinaryOperator::PGCustomBinaryOperator(_)
            )
        {
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cap_changes_root_limit_only_and_preserves_projection_and_offsets() {
        for engine in [
            DatabaseEngine::PostgreSql,
            DatabaseEngine::Sqlite,
            DatabaseEngine::MySql,
        ] {
            for (sql, suffix) in [
                ("SELECT 1 AS x, 2 AS x", "LIMIT 129"),
                (
                    "SELECT 1 AS x, 2 AS x UNION ALL SELECT 3, 4 ORDER BY 1",
                    "LIMIT 129",
                ),
                (
                    "SELECT id FROM t ORDER BY id LIMIT 2 OFFSET 1",
                    "LIMIT 2 OFFSET 1",
                ),
                ("SELECT id FROM t ORDER BY id LIMIT 128", "LIMIT 128"),
                (
                    "SELECT id FROM t ORDER BY id LIMIT 300 OFFSET 2",
                    "LIMIT 129 OFFSET 2",
                ),
                ("SELECT id FROM t LIMIT 0", "LIMIT 0"),
                (
                    "SELECT * FROM (SELECT id FROM t LIMIT 2) AS inner_query",
                    "LIMIT 129",
                ),
            ] {
                let emitted = limited_read_query(sql, engine, 129).unwrap();
                assert!(emitted.ends_with(suffix), "{engine:?}: {emitted}");
                assert!(!emitted.contains("dbide_mcp_result"));
            }
            assert!(limited_read_query("SELECT id FROM t LIMIT (1 + 2)", engine, 129).is_err());
        }
        for engine in [DatabaseEngine::Sqlite, DatabaseEngine::MySql] {
            assert!(
                limited_read_query("SELECT id FROM t LIMIT 1, 300", engine, 129)
                    .unwrap()
                    .ends_with("LIMIT 1, 129")
            );
        }
        assert!(
            limited_read_query(
                "SELECT id FROM t LIMIT -1 OFFSET 2",
                DatabaseEngine::Sqlite,
                129
            )
            .unwrap()
            .ends_with("LIMIT 129 OFFSET 2")
        );
        assert!(
            limited_read_query(
                "SELECT id FROM t LIMIT ALL OFFSET 2",
                DatabaseEngine::PostgreSql,
                129
            )
            .unwrap()
            .ends_with("LIMIT 129 OFFSET 2")
        );
        assert!(
            limited_read_query(
                "SELECT id FROM t OFFSET 2 ROWS FETCH FIRST 300 ROWS ONLY",
                DatabaseEngine::PostgreSql,
                129
            )
            .unwrap()
            .ends_with("OFFSET 2 ROWS FETCH FIRST 129 ROWS ONLY")
        );
        assert!(
            limited_read_query(
                "SELECT id FROM t FETCH FIRST 2 ROWS WITH TIES",
                DatabaseEngine::PostgreSql,
                129
            )
            .is_err()
        );
    }
    #[test]
    fn emitted_sql_must_round_trip_and_mysql_backslash_modes_fail_closed() {
        // Exact formerly accepted one-SELECT payload from independent review.
        // Never submit it to a DB: the original AST is read-only, but MySQL
        // backslash unescaping + Display used to break the emitted wrapper.
        let breakout = r"SELECT '\\' AS a, ') AS dbide_mcp_result; COMMIT; SET SESSION TRANSACTION READ WRITE; DELETE FROM secrets; -- ' AS b";
        let original = Parser::parse_sql(&MySqlDialect {}, breakout).unwrap();
        assert_eq!(original.len(), 1);
        assert!(original.visit(&mut ReadOnly).is_continue());
        assert!(read_query(breakout, DatabaseEngine::MySql).is_err());
        assert!(limited_read_query(breakout, DatabaseEngine::MySql, 129).is_err());
        for sql in [
            r"SELECT 'a\b'",
            r"SELECT '\\'; COMMIT; SET SESSION TRANSACTION READ WRITE; DELETE FROM t; --'",
            r"SELECT 1 /* \ */",
        ] {
            assert!(read_query(sql, DatabaseEngine::MySql).is_err());
            assert!(limited_read_query(sql, DatabaseEngine::MySql, 129).is_err());
        }
        for sql in [
            "SELECT 'it''s; COMMIT; DELETE FROM t' AS harmless",
            "SELECT '한글' AS label",
            "SELECT 1 AS x, 2 AS x",
        ] {
            assert!(read_query(sql, DatabaseEngine::MySql).is_ok(), "{sql}");
            assert!(
                limited_read_query(sql, DatabaseEngine::MySql, 129).is_ok(),
                "{sql}"
            );
        }
        for sql in [
            r"SELECT 'a\b'",
            r"SELECT E'a\\b'",
            "SELECT 'it''s'",
            "SELECT $$quotes';COMMIT;$$",
        ] {
            // Serialization may conservatively reject unsupported syntax, but any
            // returned output must independently reparse to the identical AST.
            let original = accepted_query(sql, DatabaseEngine::PostgreSql).unwrap();
            if let Ok(emitted) = read_query(sql, DatabaseEngine::PostgreSql) {
                assert_eq!(
                    original,
                    accepted_query(&emitted, DatabaseEngine::PostgreSql).unwrap()
                );
            }
        }
    }
    #[test]
    fn recursive_policy_rejects_writes_locks_functions_and_batches() {
        for engine in [
            DatabaseEngine::PostgreSql,
            DatabaseEngine::Sqlite,
            DatabaseEngine::MySql,
        ] {
            for sql in [
                "DELETE FROM t",
                "SELECT 1; SELECT 2",
                "SELECT * INTO new_table FROM t",
                "WITH x AS (DELETE FROM t RETURNING *) SELECT * FROM x",
                "SELECT * FROM t FOR UPDATE",
                "COPY t TO '/tmp/x'",
                "CALL proc()",
                "SELECT load_extension('x')",
                "SELECT pg_advisory_lock(1)",
                "SELECT * FROM dangerous()",
                "SELECT (SELECT evil())",
                "SELECT 1 UNION SELECT evil()",
                "SELECT 1 INTO OUTFILE '/tmp/x'",
                "SELECT @a := 1",
            ] {
                assert!(
                    read_query(sql, engine).is_err(),
                    "accepted {engine:?}: {sql}"
                );
            }
            for sql in [
                "SELECT 1",
                "WITH x AS (SELECT 1 AS n) SELECT n FROM x",
                "SELECT count(*) FROM t",
                "SELECT n FROM t UNION SELECT n FROM u",
                "SELECT 'DELETE; COPY' AS harmless",
            ] {
                assert!(
                    read_query(sql, engine).is_ok(),
                    "rejected {engine:?}: {sql}"
                );
            }
        }
    }
}
