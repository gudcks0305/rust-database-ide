//! PostgreSQL object SQL drafts. No connections and no execution.
use dbide_schema::{Index, QualifiedName, SchemaSnapshot, Table, View, quote_ident};
use sqlparser::{
    ast::{ObjectType, Query, Select, Statement, Visit, Visitor},
    dialect::PostgreSqlDialect,
    parser::Parser,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SqlDraft {
    pub sql: String,
    pub destructive: bool,
    pub description: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    Table(QualifiedName),
    View(QualifiedName),
}
impl Selection {
    pub fn name(&self) -> &QualifiedName {
        match self {
            Self::Table(n) | Self::View(n) => n,
        }
    }
}
fn ident(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 63 || value.chars().any(char::is_control) {
        return Err("Identifiers require 1–63 UTF-8 bytes and no control characters".into());
    }
    Ok(())
}
fn statement(sql: &str) -> Result<Statement, String> {
    if sql.len() > 64_000 {
        return Err("Object SQL draft exceeds 64 KB".into());
    }
    let mut statements = Parser::parse_sql(&PostgreSqlDialect {}, sql)
        .map_err(|_| "Object draft must parse as PostgreSQL SQL".to_string())?;
    if statements.len() != 1 {
        return Err("Object draft requires exactly one statement".into());
    }
    Ok(statements.remove(0))
}
fn draft(sql: String, destructive: bool, description: &str) -> Result<SqlDraft, String> {
    let ast = statement(&sql)?;
    if !matches!(
        ast,
        Statement::CreateIndex(_)
            | Statement::CreateView(_)
            | Statement::Drop {
                object_type: ObjectType::Index | ObjectType::View,
                ..
            }
    ) {
        return Err("Unsupported object draft statement".into());
    }
    Ok(SqlDraft {
        sql,
        destructive,
        description: description.into(),
    })
}
pub fn table<'a>(snapshot: &'a SchemaSnapshot, name: &QualifiedName) -> Result<&'a Table, String> {
    snapshot
        .schemas
        .iter()
        .find(|s| s.name == name.schema)
        .and_then(|s| s.tables.iter().find(|t| t.name == name.name))
        .ok_or_else(|| {
            "Table no longer exists in the captured snapshot; select a table again".into()
        })
}
pub fn view<'a>(snapshot: &'a SchemaSnapshot, name: &QualifiedName) -> Result<&'a View, String> {
    snapshot
        .schemas
        .iter()
        .find(|s| s.name == name.schema)
        .and_then(|s| s.views.iter().find(|v| v.name == name.name))
        .ok_or_else(|| "View no longer exists in the captured snapshot; select a view again".into())
}
fn relation_exists(snapshot: &SchemaSnapshot, schema: &str, name: &str) -> bool {
    snapshot
        .schemas
        .iter()
        .filter(|s| s.name == schema)
        .any(|s| {
            s.tables
                .iter()
                .any(|t| t.name == name || t.indexes.iter().any(|i| i.name == name))
                || s.views.iter().any(|v| v.name == name)
        })
}
pub fn create_index(
    snapshot: &SchemaSnapshot,
    target: &QualifiedName,
    name: &str,
    columns: &str,
    unique: bool,
) -> Result<SqlDraft, String> {
    ident(name)?;
    if relation_exists(snapshot, &target.schema, name) {
        return Err("Index name conflicts with a relation in the captured schema".into());
    }
    let table = table(snapshot, target)?;
    let columns = columns.split(',').map(str::trim).collect::<Vec<_>>();
    let mut seen = std::collections::BTreeSet::new();
    if columns.is_empty()
        || columns
            .iter()
            .any(|c| !seen.insert(*c) || !table.columns.iter().any(|column| column.name == *c))
    {
        return Err("Choose distinct exact captured column names, comma separated; expressions/predicates/INCLUDE are unsupported".into());
    }
    // PostgreSQL puts an unqualified CREATE INDEX name in its table's schema.
    let sql = format!(
        "CREATE {}INDEX {} ON {} USING btree ({});",
        if unique { "UNIQUE " } else { "" },
        quote_ident(name),
        target.sql(),
        columns
            .iter()
            .map(|c| quote_ident(c))
            .collect::<Vec<_>>()
            .join(", ")
    );
    draft(
        sql,
        false,
        "Create ordinary btree index · review locking, uniqueness validation and name conflicts",
    )
}
pub fn drop_index(schema: &str, index: &Index) -> Result<SqlDraft, String> {
    if index.constraint_owned {
        return Err("Constraint-owned index cannot be dropped independently; manage its constraint separately".into());
    }
    ident(schema)?;
    ident(&index.name)?;
    draft(
        format!(
            "DROP INDEX {} RESTRICT;",
            QualifiedName::new(schema, &index.name).sql()
        ),
        true,
        "DESTRUCTIVE · remove index · RESTRICT prevents dependent-object cascade",
    )
}
struct PlainViewQuery;
impl Visitor for PlainViewQuery {
    type Break = ();
    fn pre_visit_statement(&mut self, statement: &Statement) -> std::ops::ControlFlow<()> {
        if matches!(statement, Statement::Query(_)) {
            std::ops::ControlFlow::Continue(())
        } else {
            std::ops::ControlFlow::Break(())
        }
    }
    fn pre_visit_query(&mut self, query: &Query) -> std::ops::ControlFlow<()> {
        if query.locks.is_empty() {
            std::ops::ControlFlow::Continue(())
        } else {
            std::ops::ControlFlow::Break(())
        }
    }
    fn pre_visit_select(&mut self, select: &Select) -> std::ops::ControlFlow<()> {
        if select.into.is_none() {
            std::ops::ControlFlow::Continue(())
        } else {
            std::ops::ControlFlow::Break(())
        }
    }
}
pub fn create_view(
    snapshot: &SchemaSnapshot,
    schema: &str,
    name: &str,
    query: &str,
    replace: bool,
) -> Result<SqlDraft, String> {
    ident(schema)?;
    ident(name)?;
    if !snapshot.scope.iter().any(|s| s == schema) {
        return Err("View schema must belong to the explicit captured scope".into());
    }
    let target = QualifiedName::new(schema, name);
    if !replace && relation_exists(snapshot, schema, name) {
        return Err("View name conflicts with a captured relation; choose a new name or explicit Replace for an ordinary view".into());
    }
    if let Ok(existing) = view(snapshot, &target) {
        if existing.materialized {
            return Err(
                "Materialized view management is unsupported; no ordinary view draft generated"
                    .into(),
            );
        }
        if !replace {
            return Err("Captured view already exists; choose explicit Replace view draft".into());
        }
    } else if replace {
        return Err("Replace requires an existing ordinary view in the captured snapshot".into());
    }
    let ast = statement(query)?;
    if !matches!(ast, Statement::Query(_)) || ast.visit(&mut PlainViewQuery).is_break() {
        return Err("View definition must be exactly one SELECT/query statement".into());
    }
    let sql = format!(
        "CREATE {}VIEW {} AS {};",
        if replace { "OR REPLACE " } else { "" },
        target.sql(),
        ast
    );
    draft(
        sql,
        replace,
        if replace {
            "DESTRUCTIVE · replace ordinary view definition · review column/dependency compatibility"
        } else {
            "Create ordinary view · query functions and dependencies require review"
        },
    )
}
pub fn drop_view(snapshot: &SchemaSnapshot, target: &QualifiedName) -> Result<SqlDraft, String> {
    let value = view(snapshot, target)?;
    if value.materialized {
        return Err("Materialized view management is unsupported".into());
    }
    draft(
        format!("DROP VIEW {} RESTRICT;", target.sql()),
        true,
        "DESTRUCTIVE · remove ordinary view · RESTRICT prevents dependent-object cascade",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbide_schema::{Column, DatabaseIdentity, Provenance, Schema};
    fn fixture() -> SchemaSnapshot {
        let mut s = SchemaSnapshot::empty(
            DatabaseIdentity {
                engine: "postgresql".into(),
                identity: "owned-test".into(),
                name: "fixture".into(),
                server_version: None,
            },
            Provenance::Imported {
                label: "test".into(),
            },
            vec!["public".into()],
        );
        s.schemas.push(Schema {
            name: "public".into(),
            tables: vec![Table {
                name: "order\"items".into(),
                columns: vec![Column {
                    name: "id".into(),
                    data_type: "integer".into(),
                    nullable: false,
                    default: None,
                    generated: None,
                }],
                primary_key: None,
                foreign_keys: vec![],
                indexes: vec![],
                unsupported_features: vec![],
            }],
            views: vec![View {
                name: "v".into(),
                definition: "SELECT 1".into(),
                materialized: false,
            }],
            unsupported_objects: vec![],
        });
        s
    }
    #[test]
    fn quoted_create_index_is_one_statement() {
        let sql = create_index(
            &fixture(),
            &QualifiedName::new("public", "order\"items"),
            "idx\"; DROP TABLE users; --",
            "id",
            false,
        )
        .unwrap()
        .sql;
        assert!(sql.contains("\"order\"\"items\""));
        assert!(matches!(
            statement(&sql).unwrap(),
            Statement::CreateIndex(_)
        ));
    }
    #[test]
    fn index_columns_are_exact_and_constraint_drop_is_blocked() {
        assert!(
            create_index(
                &fixture(),
                &QualifiedName::new("public", "order\"items"),
                "idx",
                "id); DROP TABLE x; --",
                false
            )
            .is_err()
        );
        let index = Index {
            name: "idx".into(),
            columns: vec!["id".into()],
            unique: true,
            method: "btree".into(),
            predicate: None,
            expressions: vec![],
            include: vec![],
            constraint_owned: true,
            unsupported_features: vec![],
        };
        assert!(drop_index("public", &index).is_err());
    }
    #[test]
    fn view_draft_rejects_stacked_and_non_query_sql() {
        for query in [
            "SELECT 1; DROP TABLE users",
            "DROP TABLE users",
            "WITH d AS (DELETE FROM users RETURNING *) SELECT * FROM d",
            "SELECT * FROM users FOR UPDATE",
            "SELECT * INTO copied FROM users",
            "",
        ] {
            assert!(create_view(&fixture(), "public", "new_view", query, false).is_err());
        }
        assert!(create_view(&fixture(), "other", "new_view", "SELECT 1", false).is_err());
    }
    #[test]
    fn replace_drop_are_explicitly_destructive() {
        let s = fixture();
        assert!(
            create_view(&s, "public", "v", "SELECT 2", true)
                .unwrap()
                .destructive
        );
        assert!(create_view(&s, "public", "v", "SELECT 2", false).is_err());
        assert!(
            drop_view(&s, &QualifiedName::new("public", "v"))
                .unwrap()
                .sql
                .ends_with("RESTRICT;")
        );
        assert!(create_view(&s, "public", "missing", "SELECT 2", true).is_err());
    }
    #[test]
    fn captured_relation_name_conflicts_are_diagnosed() {
        let s = fixture();
        assert!(
            create_index(
                &s,
                &QualifiedName::new("public", "order\"items"),
                "v",
                "id",
                false
            )
            .is_err()
        );
        assert!(create_view(&s, "public", "order\"items", "SELECT 1", false).is_err());
    }
    #[test]
    fn materialized_view_drafts_are_refused() {
        let mut s = fixture();
        s.schemas[0].views[0].materialized = true;
        assert!(drop_view(&s, &QualifiedName::new("public", "v")).is_err());
        assert!(create_view(&s, "public", "v", "SELECT 1", true).is_err());
    }
}
