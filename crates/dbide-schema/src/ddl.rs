//! Strict offline DDL subset. Unsupported clauses return errors rather than disappearing.
use crate::{SchemaError, model::*};
use sqlparser::{
    ast::{
        self, ColumnOption, Expr, Ident, ObjectName, ObjectNamePart, SelectItem, SetExpr,
        Statement, TableConstraint,
    },
    dialect::PostgreSqlDialect,
    parser::Parser,
};

fn parse(sql: &str) -> Result<Vec<Statement>, SchemaError> {
    Parser::parse_sql(&PostgreSqlDialect {}, sql).map_err(|e| SchemaError(e.to_string()))
}
fn one(sql: &str) -> Result<Statement, SchemaError> {
    let mut statements = parse(sql)?;
    if statements.len() != 1 {
        return Err(SchemaError("expected exactly one SQL statement".into()));
    }
    Ok(statements.remove(0))
}
fn plain_create() -> ast::CreateTable {
    match one("CREATE TABLE x ()").expect("constant SQL") {
        Statement::CreateTable(c) => c,
        _ => unreachable!(),
    }
}
fn plain_outer(c: &ast::CreateTable) -> Result<(), SchemaError> {
    let mut allowed = plain_create();
    allowed.name = c.name.clone();
    allowed.columns = c.columns.clone();
    allowed.constraints = c.constraints.clone();
    if allowed.to_string() != c.to_string() {
        return Err(SchemaError(
            "unsupported CREATE TABLE clause (temporary/identity/partition/options/etc.)".into(),
        ));
    }
    Ok(())
}
fn ident(i: &Ident) -> String {
    if i.quote_style.is_some() {
        i.value.clone()
    } else {
        i.value.to_ascii_lowercase()
    }
}
fn name_parts(n: &ObjectName) -> Result<Vec<String>, SchemaError> {
    n.0.iter()
        .map(|p| match p {
            ObjectNamePart::Identifier(i) => Ok(ident(i)),
            _ => Err(SchemaError("unsupported object name".into())),
        })
        .collect()
}
fn qualified(n: &ObjectName, default_schema: &str) -> Result<QualifiedName, SchemaError> {
    let parts = name_parts(n)?;
    match parts.as_slice() {
        [name] => Ok(QualifiedName::new(default_schema, name)),
        [schema, name] => Ok(QualifiedName::new(schema, name)),
        _ => Err(SchemaError(
            "database-qualified names are unsupported".into(),
        )),
    }
}

pub fn canonical_type(value: &str) -> Result<String, SchemaError> {
    let Statement::CreateTable(c) = one(&format!("CREATE TABLE x (c {value})"))? else {
        return Err(SchemaError("invalid data type".into()));
    };
    plain_outer(&c)?;
    if c.columns.len() != 1
        || !c.constraints.is_empty()
        || !c.columns[0].options.is_empty()
        || c.columns[0].name.value != "c"
        || c.columns[0].data_type == ast::DataType::Unspecified
    {
        return Err(SchemaError("data type contains extra SQL".into()));
    }
    Ok(c.columns[0].data_type.to_string())
}
pub fn canonical_expression(value: &str) -> Result<String, SchemaError> {
    let Statement::Query(q) = one(&format!("SELECT {value}"))? else {
        return Err(SchemaError("invalid default expression".into()));
    };
    let SetExpr::Select(s) = q.body.as_ref() else {
        return Err(SchemaError("expected a scalar default expression".into()));
    };
    let [SelectItem::UnnamedExpr(expr)] = s.projection.as_slice() else {
        return Err(SchemaError("default is not one expression".into()));
    };
    let Statement::Query(mut allowed) = one("SELECT NULL")? else {
        unreachable!()
    };
    let SetExpr::Select(select) = allowed.body.as_mut() else {
        unreachable!()
    };
    select.projection = vec![SelectItem::UnnamedExpr(expr.clone())];
    if allowed.to_string() != q.to_string() {
        return Err(SchemaError("default contains query clauses".into()));
    }
    Ok(expr.to_string())
}
pub fn column_sql(column: &Column) -> Result<String, SchemaError> {
    // Generated metadata is preserved but can never be emitted as an ordinary column.
    let mut sql = format!(
        "{} {}",
        quote_ident(&column.name),
        canonical_type(&column.data_type)?
    );
    if let Some(default) = &column.default {
        sql.push_str(&format!(" DEFAULT {}", canonical_expression(default)?));
    }
    if !column.nullable {
        sql.push_str(" NOT NULL");
    }
    Ok(sql)
}
pub fn primary_key_sql(pk: &PrimaryKey) -> String {
    format!(
        "{}PRIMARY KEY ({})",
        pk.name
            .as_ref()
            .map(|n| format!("CONSTRAINT {} ", quote_ident(n)))
            .unwrap_or_default(),
        quote_columns(&pk.columns)
    )
}
pub fn foreign_key_sql(fk: &ForeignKey) -> String {
    format!(
        "{}FOREIGN KEY ({}) REFERENCES {} ({}) ON DELETE {} ON UPDATE {}{}",
        fk.name
            .as_ref()
            .map(|n| format!("CONSTRAINT {} ", quote_ident(n)))
            .unwrap_or_default(),
        quote_columns(&fk.columns),
        fk.references.sql(),
        quote_columns(&fk.referenced_columns),
        fk.on_delete.sql(),
        fk.on_update.sql(),
        if fk.deferrable {
            if fk.initially_deferred {
                " DEFERRABLE INITIALLY DEFERRED"
            } else {
                " DEFERRABLE INITIALLY IMMEDIATE"
            }
        } else {
            " NOT DEFERRABLE"
        }
    )
}
pub fn create_table_sql(name: &QualifiedName, table: &Table) -> Result<String, SchemaError> {
    if !table.unsupported_features.is_empty() || table.columns.iter().any(|c| c.generated.is_some())
    {
        return Err(SchemaError(format!(
            "cannot generate incomplete DDL for {}",
            name.sql()
        )));
    }
    let mut parts = table
        .columns
        .iter()
        .map(column_sql)
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(pk) = &table.primary_key {
        parts.push(primary_key_sql(pk));
    }
    // FKs are emitted after ALL tables; this supports cycles without CASCADE.
    Ok(format!(
        "CREATE TABLE {} ({});",
        name.sql(),
        parts.join(", ")
    ))
}
fn simple_pk(
    pk: &ast::PrimaryKeyConstraint,
    inline: Option<&str>,
) -> Result<PrimaryKey, SchemaError> {
    if pk.index_name.is_some()
        || pk.index_type.is_some()
        || !pk.include.is_empty()
        || !pk.index_options.is_empty()
        || pk.characteristics.is_some()
    {
        return Err(SchemaError("unsupported primary key options".into()));
    }
    let columns = if let Some(column) = inline {
        vec![column.into()]
    } else {
        pk.columns
            .iter()
            .map(|c| match &c.column.expr {
                Expr::Identifier(i)
                    if c.operator_class.is_none() && c.column.to_string() == i.to_string() =>
                {
                    Ok(ident(i))
                }
                _ => Err(SchemaError(
                    "unsupported primary key expression/order".into(),
                )),
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    Ok(PrimaryKey {
        name: pk.name.as_ref().map(ident),
        columns,
    })
}
fn action(a: Option<ast::ReferentialAction>) -> ReferentialAction {
    match a {
        None | Some(ast::ReferentialAction::NoAction) => ReferentialAction::NoAction,
        Some(ast::ReferentialAction::Restrict) => ReferentialAction::Restrict,
        Some(ast::ReferentialAction::Cascade) => ReferentialAction::Cascade,
        Some(ast::ReferentialAction::SetNull) => ReferentialAction::SetNull,
        Some(ast::ReferentialAction::SetDefault) => ReferentialAction::SetDefault,
    }
}
fn fk_model(
    f: &ast::ForeignKeyConstraint,
    default_schema: &str,
    inline: Option<&str>,
) -> Result<ForeignKey, SchemaError> {
    if f.index_name.is_some()
        || f.match_kind.is_some()
        || f.characteristics
            .as_ref()
            .is_some_and(|c| c.enforced.is_some())
    {
        return Err(SchemaError("unsupported foreign key options".into()));
    }
    Ok(ForeignKey {
        name: f.name.as_ref().map(ident),
        columns: inline
            .map(|s| vec![s.into()])
            .unwrap_or_else(|| f.columns.iter().map(ident).collect()),
        references: qualified(&f.foreign_table, default_schema)?,
        referenced_columns: f.referred_columns.iter().map(ident).collect(),
        on_delete: action(f.on_delete),
        on_update: action(f.on_update),
        deferrable: f
            .characteristics
            .as_ref()
            .and_then(|c| c.deferrable)
            .unwrap_or(false),
        initially_deferred: f
            .characteristics
            .as_ref()
            .is_some_and(|c| c.initially == Some(ast::DeferrableInitial::Deferred)),
    })
}
/// Imports only ordinary CREATE TABLE and plain CREATE SCHEMA. Search path is explicit.
/// Any unsupported statement/clause aborts the whole import (no misleading partial snapshot).
pub fn import_postgres_ddl(
    sql: &str,
    database: DatabaseIdentity,
    default_schema: &str,
    label: &str,
) -> Result<SchemaSnapshot, SchemaError> {
    let mut snapshot = SchemaSnapshot::empty(
        database,
        Provenance::OfflineDdl {
            label: label.into(),
        },
        vec![],
    );
    if sql.len() > 16_000_000 {
        return Err(SchemaError("offline DDL exceeds 16 MB input cap".into()));
    }
    for statement in parse(sql)? {
        let original = statement.to_string();
        match statement {
            Statement::CreateSchema {
                schema_name: ast::SchemaName::Simple(name),
                ..
            } => {
                // Canonical reconstruction rejects authorization/options/IF NOT EXISTS.
                if original != format!("CREATE SCHEMA {name}") {
                    return Err(SchemaError("unsupported schema options".into()));
                }
                let parts = name_parts(&name)?;
                let [name] = parts.as_slice() else {
                    return Err(SchemaError("invalid schema name".into()));
                };
                ensure_schema(&mut snapshot, name);
            }
            Statement::CreateTable(c) => {
                plain_outer(&c)?;
                let q = qualified(&c.name, default_schema)?;
                let mut table = Table {
                    name: q.name.clone(),
                    columns: vec![],
                    primary_key: None,
                    foreign_keys: vec![],
                    indexes: vec![],
                    unsupported_features: vec![],
                };
                for c in &c.columns {
                    if matches!(
                        c.data_type.to_string().to_ascii_uppercase().as_str(),
                        "SERIAL" | "BIGSERIAL" | "SMALLSERIAL" | "SERIAL2" | "SERIAL4" | "SERIAL8"
                    ) {
                        return Err(SchemaError("SERIAL creates an unmodeled sequence; use explicit integer and sequence metadata".into()));
                    }
                    let name = ident(&c.name);
                    let mut col = Column {
                        name: name.clone(),
                        data_type: c.data_type.to_string(),
                        nullable: true,
                        default: None,
                        generated: None,
                    };
                    let mut seen_null = false;
                    for option in &c.options {
                        match &option.option {
                            ColumnOption::Null | ColumnOption::NotNull
                                if option.name.is_none() && !seen_null =>
                            {
                                col.nullable = matches!(option.option, ColumnOption::Null);
                                seen_null = true;
                            }
                            ColumnOption::Default(expr)
                                if option.name.is_none() && col.default.is_none() =>
                            {
                                col.default = Some(expr.to_string());
                            }
                            ColumnOption::PrimaryKey(pk) if table.primary_key.is_none() => {
                                let mut pk = simple_pk(pk, Some(&name))?;
                                pk.name = option.name.as_ref().map(ident).or(pk.name);
                                table.primary_key = Some(pk);
                            }
                            ColumnOption::ForeignKey(fk) => {
                                let mut f = fk_model(fk, default_schema, Some(&name))?;
                                f.name = option.name.as_ref().map(ident).or(f.name);
                                table.foreign_keys.push(f);
                            }
                            _ => {
                                return Err(SchemaError(format!(
                                    "unsupported/duplicate column option: {option}"
                                )));
                            }
                        }
                    }
                    table.columns.push(col);
                }
                for constraint in &c.constraints {
                    match constraint {
                        TableConstraint::PrimaryKey(pk) if table.primary_key.is_none() => {
                            table.primary_key = Some(simple_pk(pk, None)?)
                        }
                        TableConstraint::ForeignKey(fk) => {
                            table.foreign_keys.push(fk_model(fk, default_schema, None)?)
                        }
                        _ => {
                            return Err(SchemaError(format!(
                                "unsupported/duplicate table constraint: {constraint}"
                            )));
                        }
                    }
                }
                if let Some(pk) = &table.primary_key {
                    for col in &mut table.columns {
                        if pk.columns.contains(&col.name) {
                            col.nullable = false;
                        }
                    }
                }
                ensure_schema(&mut snapshot, &q.schema).tables.push(table);
            }
            Statement::AlterTable(a) => {
                if a.if_exists
                    || a.only
                    || a.location.is_some()
                    || a.on_cluster.is_some()
                    || a.table_type.is_some()
                    || a.operations.len() != 1
                {
                    return Err(SchemaError("unsupported ALTER TABLE options".into()));
                }
                let q = qualified(&a.name, default_schema)?;
                let table = snapshot
                    .schemas
                    .iter_mut()
                    .find(|s| s.name == q.schema)
                    .and_then(|s| s.tables.iter_mut().find(|t| t.name == q.name))
                    .ok_or_else(|| SchemaError("ALTER references table not yet defined".into()))?;
                match &a.operations[0] {
                    ast::AlterTableOperation::AddConstraint {
                        constraint: TableConstraint::ForeignKey(f),
                        not_valid: false,
                    } => table.foreign_keys.push(fk_model(f, default_schema, None)?),
                    _ => {
                        return Err(SchemaError(
                            "offline ALTER supports ADD FOREIGN KEY only".into(),
                        ));
                    }
                }
            }
            Statement::CreateIndex(i) => {
                if i.concurrently
                    || i.r#async
                    || i.if_not_exists
                    || !i.include.is_empty()
                    || i.nulls_distinct.is_some()
                    || !i.with.is_empty()
                    || i.predicate.is_some()
                    || !i.index_options.is_empty()
                    || !i.alter_options.is_empty()
                    || i.using
                        .as_ref()
                        .is_some_and(|v| !v.to_string().eq_ignore_ascii_case("btree"))
                {
                    return Err(SchemaError("unsupported CREATE INDEX options".into()));
                }
                let q = qualified(&i.table_name, default_schema)?;
                if i.name.as_ref().is_some_and(|n| n.0.len() != 1) {
                    return Err(SchemaError(
                        "PostgreSQL CREATE INDEX names must not be schema-qualified".into(),
                    ));
                }
                let iname = qualified(
                    i.name
                        .as_ref()
                        .ok_or_else(|| SchemaError("offline indexes require a name".into()))?,
                    &q.schema,
                )?;
                if iname.schema != q.schema {
                    return Err(SchemaError("index schema must match table schema".into()));
                }
                let columns = i
                    .columns
                    .iter()
                    .map(|c| match &c.column.expr {
                        Expr::Identifier(i)
                            if c.operator_class.is_none()
                                && c.column.to_string() == i.to_string() =>
                        {
                            Ok(ident(i))
                        }
                        _ => Err(SchemaError("unsupported index expression/order".into())),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let table = snapshot
                    .schemas
                    .iter_mut()
                    .find(|s| s.name == q.schema)
                    .and_then(|s| s.tables.iter_mut().find(|t| t.name == q.name))
                    .ok_or_else(|| SchemaError("index references table not yet defined".into()))?;
                table.indexes.push(Index {
                    name: iname.name,
                    columns,
                    unique: i.unique,
                    method: "btree".into(),
                    predicate: None,
                    expressions: vec![],
                    include: vec![],
                    constraint_owned: false,
                    unsupported_features: vec![],
                });
            }
            _ => {
                return Err(SchemaError(format!(
                    "unsupported offline DDL statement: {statement}"
                )));
            }
        }
    }
    snapshot.validate()?;
    Ok(snapshot)
}
fn ensure_schema<'a>(snapshot: &'a mut SchemaSnapshot, name: &str) -> &'a mut Schema {
    if !snapshot.scope.iter().any(|s| s == name) {
        snapshot.scope.push(name.into());
    }
    if !snapshot.schemas.iter().any(|s| s.name == name) {
        snapshot.schemas.push(Schema {
            name: name.into(),
            tables: vec![],
            views: vec![],
            unsupported_objects: vec![],
        });
    }
    snapshot
        .schemas
        .iter_mut()
        .find(|s| s.name == name)
        .expect("just inserted")
}
/// Standalone DDL export refuses any metadata it cannot round-trip.
pub fn export_postgres_ddl(snapshot: &SchemaSnapshot) -> Result<String, SchemaError> {
    snapshot.validate()?;
    let mut result = vec![];
    let mut schemas = snapshot.schemas.iter().collect::<Vec<_>>();
    schemas.sort_by_key(|s| &s.name);
    for schema in schemas {
        if !schema.views.is_empty() || !schema.unsupported_objects.is_empty() {
            return Err(SchemaError(
                "DDL export of views/unsupported objects is unavailable; use JSON".into(),
            ));
        }
        result.push(format!("CREATE SCHEMA {};", quote_ident(&schema.name)));
        let mut tables = schema.tables.iter().collect::<Vec<_>>();
        tables.sort_by_key(|t| &t.name);
        for table in tables {
            result.push(create_table_sql(
                &QualifiedName::new(&schema.name, &table.name),
                table,
            )?);
        }
    }
    let mut tables = snapshot.tables().collect::<Vec<_>>();
    tables.sort_by(|(a, _), (b, _)| a.cmp(b));
    for (name, t) in &tables {
        for i in &t.indexes {
            if i.constraint_owned {
                return Err(SchemaError(
                    "constraint-owned index export requires its constraint model; use JSON".into(),
                ));
            }
            if !i.method.eq_ignore_ascii_case("btree")
                || i.predicate.is_some()
                || !i.expressions.is_empty()
                || !i.include.is_empty()
                || !i.unsupported_features.is_empty()
                || i.columns.is_empty()
            {
                return Err(SchemaError("unsupported index export; use JSON".into()));
            }
            result.push(format!(
                "CREATE {}INDEX {} ON {} USING btree ({});",
                if i.unique { "UNIQUE " } else { "" },
                quote_ident(&i.name),
                name.sql(),
                quote_columns(&i.columns)
            ));
        }
    }
    for (name, t) in tables {
        for fk in &t.foreign_keys {
            result.push(format!(
                "ALTER TABLE {} ADD {};",
                name.sql(),
                foreign_key_sql(fk)
            ));
        }
    }
    Ok(result.join("\n"))
}
