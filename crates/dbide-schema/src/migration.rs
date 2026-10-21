use crate::{SchemaError, ddl, model::*};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    CreateSchema,
    DropSchema,
    CreateTable,
    DropTable,
    AddColumn,
    DropColumn,
    AlterType,
    SetNullable,
    SetDefault,
    AddPrimaryKey,
    DropPrimaryKey,
    AddForeignKey,
    DropForeignKey,
    CreateIndex,
    DropIndex,
    Unsupported,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub kind: OperationKind,
    pub object: QualifiedName,
    pub detail: String,
    /// None means unsupported. Never convert unsupported changes into runnable SQL.
    pub sql: Option<String>,
    pub destructive: bool,
    pub requires_data_validation: bool,
    pub unsupported: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationPlan {
    pub format_version: u32,
    /// Always `source_desired -> target_actual`: apply operations TO target to make it like source.
    pub direction: String,
    pub source_database: DatabaseIdentity,
    pub target_database: DatabaseIdentity,
    pub source_fingerprint: String,
    pub target_fingerprint: String,
    /// Original preview capture evidence; fresh validation compares semantic fingerprints.
    pub source_capture_fingerprint: String,
    pub target_capture_fingerprint: String,
    pub operations: Vec<Operation>,
    pub warnings: Vec<String>,
    /// PostgreSQL ordinary DDL is transactional; this is not an execution/atomicity guarantee.
    pub implicit_commit: bool,
}
impl MigrationPlan {
    pub fn has_unsupported(&self) -> bool {
        self.operations.iter().any(|o| o.unsupported)
    }
    pub fn is_destructive(&self) -> bool {
        self.operations.iter().any(|o| o.destructive)
    }
    pub fn preview_sql(&self) -> String {
        self.operations
            .iter()
            .map(|o| {
                o.sql.clone().unwrap_or_else(|| {
                    format!("-- UNSUPPORTED: {}", o.detail.replace(['\n', '\r'], " "))
                })
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    /// Recompute against both captures immediately before a caller even considers execution.
    /// This library cannot execute and cannot certify remote schema freshness or permissions.
    pub fn verify_against(
        &self,
        source_desired: &SchemaSnapshot,
        target_actual: &SchemaSnapshot,
    ) -> Result<(), SchemaError> {
        if self.source_fingerprint != source_desired.content_fingerprint()?
            || self.target_fingerprint != target_actual.content_fingerprint()?
        {
            return Err(SchemaError(
                "stale migration plan: database identity/schema content changed".into(),
            ));
        }
        let mut recomputed = diff(source_desired, target_actual)?;
        // Preserve original preview evidence while accepting a fresh equivalent capture.
        recomputed.source_capture_fingerprint = self.source_capture_fingerprint.clone();
        recomputed.target_capture_fingerprint = self.target_capture_fingerprint.clone();
        if *self != recomputed {
            return Err(SchemaError(
                "migration plan was modified or belongs to a different planner version".into(),
            ));
        }
        if self.has_unsupported() {
            return Err(SchemaError(
                "migration contains unsupported operations".into(),
            ));
        }
        Ok(())
    }
    pub fn verify_target(&self, target_actual: &SchemaSnapshot) -> Result<(), SchemaError> {
        if self.target_fingerprint != target_actual.content_fingerprint()? {
            return Err(SchemaError("stale target snapshot".into()));
        }
        Ok(())
    }
}
struct Builder {
    operations: Vec<(u8, Operation)>,
    warnings: BTreeSet<String>,
}
impl Builder {
    fn op(
        &mut self,
        phase: u8,
        kind: OperationKind,
        name: &QualifiedName,
        detail: impl Into<String>,
        sql: String,
        flags: (bool, bool),
    ) {
        self.operations.push((
            phase,
            Operation {
                kind,
                object: name.clone(),
                detail: detail.into(),
                sql: Some(sql),
                destructive: flags.0,
                requires_data_validation: flags.1,
                unsupported: false,
            },
        ));
    }
    fn unsupported(&mut self, name: &QualifiedName, detail: impl Into<String>, destructive: bool) {
        self.operations.push((
            255,
            Operation {
                kind: OperationKind::Unsupported,
                object: name.clone(),
                detail: detail.into(),
                sql: None,
                destructive,
                requires_data_validation: false,
                unsupported: true,
            },
        ));
    }
}
fn same_fks(desired: &[ForeignKey], actual: &[ForeignKey]) -> bool {
    if desired.len() != actual.len() {
        return false;
    }
    let mut remaining = actual.iter().collect::<Vec<_>>();
    // Named desired constraints match first, preventing unnamed ones consuming their candidates.
    let mut desired = desired.iter().collect::<Vec<_>>();
    desired.sort_by_key(|f| f.name.is_none());
    for wanted in desired {
        let Some(i) = remaining.iter().position(|a| {
            let mut a = (*a).clone();
            if wanted.name.is_none() {
                a.name = None;
            }
            a == *wanted
        }) else {
            return false;
        };
        remaining.remove(i);
    }
    true
}
fn same_pk(desired: &Option<PrimaryKey>, actual: &Option<PrimaryKey>) -> bool {
    match (desired, actual) {
        (None, None) => true,
        (Some(d), Some(a)) => d.columns == a.columns && (d.name.is_none() || d.name == a.name),
        _ => false,
    }
}
fn primary_index(t: &Table, i: &Index) -> bool {
    i.constraint_owned
        && i.unique
        && i.method.eq_ignore_ascii_case("btree")
        && i.predicate.is_none()
        && i.expressions.is_empty()
        && i.include.is_empty()
        && i.unsupported_features.is_empty()
        && t.primary_key
            .as_ref()
            .is_some_and(|p| p.name.as_ref() == Some(&i.name) && p.columns == i.columns)
}
fn same_structure(desired: &Table, actual: &Table) -> bool {
    let mut d = desired.clone();
    let mut a = actual.clone();
    d.foreign_keys.clear();
    a.foreign_keys.clear();
    d.indexes.clear();
    a.indexes.clear();
    if same_pk(&d.primary_key, &a.primary_key) {
        a.primary_key = d.primary_key.clone();
    }
    d == a
}
fn normalized_column(c: &Column) -> Result<Column, SchemaError> {
    let mut c = c.clone();
    c.data_type = ddl::canonical_type(&c.data_type)?;
    c.default = c
        .default
        .as_deref()
        .map(ddl::canonical_expression)
        .transpose()?;
    Ok(c)
}
fn basic_index(index: &Index) -> bool {
    index.method.eq_ignore_ascii_case("btree")
        && index.expressions.is_empty()
        && index.predicate.is_none()
        && index.include.is_empty()
        && index.unsupported_features.is_empty()
        && !index.columns.is_empty()
        && !index.constraint_owned
}
fn index_sql(table: &QualifiedName, index: &Index) -> String {
    // PostgreSQL CREATE INDEX index name is unqualified and inherits the table schema.
    format!(
        "CREATE {}INDEX {} ON {} USING btree ({});",
        if index.unique { "UNIQUE " } else { "" },
        quote_ident(&index.name),
        table.sql(),
        quote_columns(&index.columns)
    )
}
fn fk_references_valid(fk: &ForeignKey, tables: &BTreeMap<QualifiedName, &Table>) -> bool {
    tables.get(&fk.references).is_some_and(|t| {
        let key = t
            .primary_key
            .as_ref()
            .is_some_and(|k| k.columns == fk.referenced_columns)
            || t.indexes
                .iter()
                .any(|i| i.unique && basic_index(i) && i.columns == fk.referenced_columns);
        key && fk
            .referenced_columns
            .iter()
            .all(|n| t.columns.iter().any(|c| &c.name == n))
    })
}
/// Direction is explicit: `source_desired` is the desired schema, `target_actual` is modified.
/// No renames are inferred. Unrepresented PostgreSQL features must be marked by the adapter.
pub fn diff(
    source_desired: &SchemaSnapshot,
    target_actual: &SchemaSnapshot,
) -> Result<MigrationPlan, SchemaError> {
    source_desired.validate()?;
    target_actual.validate()?;
    // Match content-fingerprint semantics: collection retrieval order is not a schema change.
    // Column/key column order remains semantic and is deliberately preserved.
    let canonical = |snapshot: &SchemaSnapshot| {
        let mut s = snapshot.clone();
        s.scope.sort();
        s.schemas.sort_by(|a, b| a.name.cmp(&b.name));
        for schema in &mut s.schemas {
            schema.tables.sort_by(|a, b| a.name.cmp(&b.name));
            schema.views.sort_by(|a, b| a.name.cmp(&b.name));
            schema.unsupported_objects.sort();
            for t in &mut schema.tables {
                t.foreign_keys
                    .sort_by_key(|f| serde_json::to_string(f).unwrap_or_default());
                t.indexes.sort_by(|a, b| a.name.cmp(&b.name));
                t.unsupported_features.sort();
            }
        }
        s
    };
    let source = canonical(source_desired);
    let target = canonical(target_actual);
    let source_desired = &source;
    let target_actual = &target;
    if !source_desired.complete || !target_actual.complete {
        return Err(SchemaError(
            "cannot diff incomplete/filtered snapshots".into(),
        ));
    }
    let scope = |s: &SchemaSnapshot| s.scope.iter().cloned().collect::<BTreeSet<_>>();
    if scope(source_desired) != scope(target_actual) {
        return Err(SchemaError(
            "source and target schema scopes must match explicitly".into(),
        ));
    }
    let desired = source_desired.tables().collect::<BTreeMap<_, _>>();
    let actual = target_actual.tables().collect::<BTreeMap<_, _>>();
    let mut b = Builder {
        operations: vec![],
        warnings: BTreeSet::new(),
    };
    b.warnings.insert("Preview only. Review locks, data validation, privileges, dependencies outside scope and transactional execution/partial failure policy before any application.".into());
    b.warnings.insert("No rename inference; an object rename appears as DROP plus CREATE. Expressions/types are SQL, not bound data.".into());
    let desired_schemas = source_desired
        .schemas
        .iter()
        .map(|s| (&s.name, s))
        .collect::<BTreeMap<_, _>>();
    let actual_schemas = target_actual
        .schemas
        .iter()
        .map(|s| (&s.name, s))
        .collect::<BTreeMap<_, _>>();
    for s in &source_desired.schemas {
        if !actual_schemas.contains_key(&s.name) {
            let n = QualifiedName::new(&s.name, "");
            b.op(
                0,
                OperationKind::CreateSchema,
                &n,
                "create schema",
                format!("CREATE SCHEMA {};", quote_ident(&s.name)),
                (false, false),
            );
        }
        if !s.unsupported_objects.is_empty()
            && actual_schemas.get(&s.name).map(|s| &s.unsupported_objects)
                != Some(&s.unsupported_objects)
        {
            b.unsupported(
                &QualifiedName::new(&s.name, ""),
                "schema has unsupported object changes",
                false,
            );
        }
    }
    // All FKs are detached before a table/key/column change and restored after every table/key exists.
    // This deliberately handles mutual references and self references without topological cycles.
    let shape_changed = desired.len() != actual.len()
        || desired
            .iter()
            .any(|(n, t)| actual.get(n).is_none_or(|a| !same_structure(t, a)));
    let fk_changed = desired.iter().any(|(n, t)| {
        actual
            .get(n)
            .is_none_or(|a| !same_fks(&t.foreign_keys, &a.foreign_keys))
    }) || actual
        .iter()
        .any(|(n, t)| !desired.contains_key(n) && !t.foreign_keys.is_empty());
    // Unique indexes can be FK dependency targets even when columns/PK/FKs are unchanged.
    // Compare non-PK indexes as a multiset so ordering/backing-PK capture details do not churn FKs.
    let indexes = |t: &Table| {
        let mut values = t
            .indexes
            .iter()
            .filter(|i| !primary_index(t, i))
            .map(|i| serde_json::to_string(i).unwrap_or_default())
            .collect::<Vec<_>>();
        values.sort();
        values
    };
    let indexes_changed = desired
        .iter()
        .any(|(n, t)| actual.get(n).is_none_or(|a| indexes(t) != indexes(a)));
    let rebuild_fks = shape_changed || fk_changed || indexes_changed;
    if rebuild_fks {
        for (name, t) in &actual {
            for fk in &t.foreign_keys {
                if !t.unsupported_features.is_empty() {
                    b.unsupported(
                        name,
                        "cannot detach FK from table with unmodeled dependency metadata",
                        true,
                    );
                }
                if let Some(constraint) = &fk.name {
                    b.op(
                        10,
                        OperationKind::DropForeignKey,
                        name,
                        "detach foreign key",
                        format!(
                            "ALTER TABLE {} DROP CONSTRAINT {};",
                            name.sql(),
                            quote_ident(constraint)
                        ),
                        (true, false),
                    );
                } else {
                    b.unsupported(
                        name,
                        "actual foreign key has no stable name; cannot detach",
                        true,
                    );
                }
            }
        }
    }
    // Indexes: only plain btree column indexes; unknown definitions/constraint-owned indexes fail closed.
    for (name, t) in &actual {
        let wanted = desired.get(name);
        for index in &t.indexes {
            let changed = wanted.is_none_or(|w| !w.indexes.contains(index))
                || wanted.is_some_and(|w| !same_structure(w, t));
            if !changed {
                continue;
            }
            if index.constraint_owned {
                if wanted.is_some() && !primary_index(t, index) {
                    b.unsupported(name,format!("constraint-owned index {} changed; unique/exclusion constraint model required",index.name),true);
                }
                continue;
            }
            if !basic_index(index) {
                b.unsupported(
                    name,
                    format!("unsupported index drop/rebuild {}", index.name),
                    true,
                );
                continue;
            }
            b.op(
                15,
                OperationKind::DropIndex,
                name,
                format!("drop index {}", index.name),
                format!(
                    "DROP INDEX {};",
                    QualifiedName::new(&name.schema, &index.name).sql()
                ),
                (true, false),
            );
        }
    }
    for (name, t) in &actual {
        if !desired.contains_key(name) {
            if !t.unsupported_features.is_empty() {
                b.unsupported(name, "table drop has unmodeled dependencies/features", true);
            } else {
                b.op(
                    20,
                    OperationKind::DropTable,
                    name,
                    "drop table; all stored rows lost",
                    format!("DROP TABLE {};", name.sql()),
                    (true, false),
                );
            }
        }
    }
    for (name, wanted) in &desired {
        if wanted
            .indexes
            .iter()
            .any(|i| i.constraint_owned && !primary_index(wanted, i))
            && actual.get(name).is_none_or(|a| *a != *wanted)
        {
            b.unsupported(name,"unique/exclusion constraint-owned index cannot be generated as a primary key or ordinary index",false);
        }
        if !wanted.unsupported_features.is_empty()
            || wanted.columns.iter().any(|c| c.generated.is_some())
        {
            if actual.get(name).is_none_or(|a| *a != *wanted) {
                b.unsupported(name, "table has generated/unmodeled features", false);
            }
            continue;
        }
        let Some(current) = actual.get(name) else {
            b.op(
                40,
                OperationKind::CreateTable,
                name,
                "create table with primary key",
                ddl::create_table_sql(name, wanted)?,
                (false, false),
            );
            continue;
        };
        if !current.unsupported_features.is_empty()
            || current.columns.iter().any(|c| c.generated.is_some())
        {
            if *current != *wanted {
                b.unsupported(name, "actual table has generated/unmodeled features", true);
            }
            continue;
        }
        if !same_pk(&wanted.primary_key, &current.primary_key) {
            if let Some(pk) = &current.primary_key {
                if let Some(n) = &pk.name {
                    b.op(
                        25,
                        OperationKind::DropPrimaryKey,
                        name,
                        "drop primary key",
                        format!(
                            "ALTER TABLE {} DROP CONSTRAINT {};",
                            name.sql(),
                            quote_ident(n)
                        ),
                        (true, false),
                    );
                } else {
                    b.unsupported(name, "actual primary key has no stable name", true);
                }
            }
            if let Some(pk) = &wanted.primary_key {
                b.op(
                    60,
                    OperationKind::AddPrimaryKey,
                    name,
                    "add primary key; existing values require validation",
                    format!(
                        "ALTER TABLE {} ADD {};",
                        name.sql(),
                        ddl::primary_key_sql(pk)
                    ),
                    (false, true),
                );
            }
        }
        let desired_cols = wanted
            .columns
            .iter()
            .map(|c| (&c.name, c))
            .collect::<BTreeMap<_, _>>();
        let current_cols = current
            .columns
            .iter()
            .map(|c| (&c.name, c))
            .collect::<BTreeMap<_, _>>();
        let old_retained = current
            .columns
            .iter()
            .filter(|c| desired_cols.contains_key(&c.name))
            .map(|c| &c.name)
            .collect::<Vec<_>>();
        let new_retained = wanted
            .columns
            .iter()
            .filter(|c| current_cols.contains_key(&c.name))
            .map(|c| &c.name)
            .collect::<Vec<_>>();
        if old_retained != new_retained {
            b.unsupported(name, "column reordering requires table rebuild", true);
        }
        // PostgreSQL adds columns at the end. Insertion before a retained column is not reproduced.
        let mut saw_new = false;
        for c in &wanted.columns {
            if !current_cols.contains_key(&c.name) {
                saw_new = true;
            } else if saw_new {
                b.unsupported(
                    name,
                    "new column insertion before existing column is unsupported",
                    false,
                );
                break;
            }
        }
        for c in &current.columns {
            if !desired_cols.contains_key(&c.name) {
                b.op(
                    30,
                    OperationKind::DropColumn,
                    name,
                    format!("drop column {} and its data", c.name),
                    format!(
                        "ALTER TABLE {} DROP COLUMN {};",
                        name.sql(),
                        quote_ident(&c.name)
                    ),
                    (true, false),
                );
            }
        }
        for wanted_col in &wanted.columns {
            let w = normalized_column(wanted_col)?;
            let Some(c) = current_cols.get(&w.name) else {
                b.op(
                    45,
                    OperationKind::AddColumn,
                    name,
                    format!("add column {}", w.name),
                    format!(
                        "ALTER TABLE {} ADD COLUMN {};",
                        name.sql(),
                        ddl::column_sql(&w)?
                    ),
                    (false, !w.nullable || w.default.is_some()),
                );
                continue;
            };
            let c = normalized_column(c)?;
            if c.data_type != w.data_type {
                b.unsupported(name,format!("column {} type {} -> {} requires explicit USING/conversion and dependency policy",w.name,c.data_type,w.data_type),true);
            }
            if c.nullable != w.nullable {
                b.op(
                    50,
                    OperationKind::SetNullable,
                    name,
                    format!("set nullability for {}", w.name),
                    format!(
                        "ALTER TABLE {} ALTER COLUMN {} {} NOT NULL;",
                        name.sql(),
                        quote_ident(&w.name),
                        if w.nullable { "DROP" } else { "SET" }
                    ),
                    (!w.nullable, !w.nullable),
                );
            }
            if c.default != w.default {
                let action = if let Some(d) = &w.default {
                    format!("SET DEFAULT {d}")
                } else {
                    "DROP DEFAULT".into()
                };
                b.op(
                    50,
                    OperationKind::SetDefault,
                    name,
                    format!("change default for {} (future inserts)", w.name),
                    format!(
                        "ALTER TABLE {} ALTER COLUMN {} {action};",
                        name.sql(),
                        quote_ident(&w.name)
                    ),
                    (false, false),
                );
            }
        }
    }
    for (name, t) in &desired {
        for index in &t.indexes {
            let changed = actual
                .get(name)
                .is_none_or(|a| !a.indexes.contains(index) || !same_structure(t, a));
            if !changed || index.constraint_owned {
                continue;
            }
            if !basic_index(index) {
                b.unsupported(
                    name,
                    format!("unsupported index definition {}", index.name),
                    false,
                );
                continue;
            }
            b.op(
                65,
                OperationKind::CreateIndex,
                name,
                format!("create index {}", index.name),
                index_sql(name, index),
                (false, index.unique),
            );
        }
        if rebuild_fks {
            for fk in &t.foreign_keys {
                if !fk_references_valid(fk, &desired) {
                    b.unsupported(
                        name,
                        format!(
                            "foreign key references unresolved/non-unique key {}",
                            fk.references.sql()
                        ),
                        false,
                    );
                    continue;
                }
                b.op(
                    70,
                    OperationKind::AddForeignKey,
                    name,
                    "add foreign key; existing values require validation",
                    format!(
                        "ALTER TABLE {} ADD {};",
                        name.sql(),
                        ddl::foreign_key_sql(fk)
                    ),
                    (false, true),
                );
            }
        }
    }
    for s in &target_actual.schemas {
        let wanted = desired_schemas.get(&s.name);
        if !s.unsupported_objects.is_empty()
            && wanted.map(|w| &w.unsupported_objects) != Some(&s.unsupported_objects)
        {
            b.unsupported(
                &QualifiedName::new(&s.name, ""),
                "unsupported schema objects cannot be removed/changed",
                true,
            );
        }
        if (shape_changed || indexes_changed) && !s.unsupported_objects.is_empty() {
            b.unsupported(&QualifiedName::new(&s.name,""),"table/index changes may affect unmodeled schema objects; dependency model required",true);
        }
        if shape_changed && !s.views.is_empty() {
            b.unsupported(
                &QualifiedName::new(&s.name, ""),
                "table changes may affect views; dependency-aware view planner required",
                true,
            );
        }
        if wanted.map(|w| &w.views) != Some(&s.views) && !s.views.is_empty() {
            b.unsupported(
                &QualifiedName::new(&s.name, ""),
                "view changes/drops require dependency-aware view planner",
                true,
            );
        }
        if wanted.is_none() {
            if !s.views.is_empty() || !s.unsupported_objects.is_empty() {
                b.unsupported(
                    &QualifiedName::new(&s.name, ""),
                    "schema drop blocked by unmodeled objects/views",
                    true,
                );
            } else {
                b.op(
                    90,
                    OperationKind::DropSchema,
                    &QualifiedName::new(&s.name, ""),
                    "drop empty schema",
                    format!("DROP SCHEMA {};", quote_ident(&s.name)),
                    (true, false),
                );
            }
        }
    }
    for s in &source_desired.schemas {
        if !s.views.is_empty() && actual_schemas.get(&s.name).map(|a| &a.views) != Some(&s.views) {
            b.unsupported(
                &QualifiedName::new(&s.name, ""),
                "view creation/modification requires dependency-aware view planner",
                false,
            );
        }
    }
    // Stable sort preserves per-table column append order and deterministic iteration order.
    b.operations.sort_by_key(|(phase, _)| *phase);
    Ok(MigrationPlan {
        format_version: 2,
        direction: "source_desired -> target_actual".into(),
        source_database: source_desired.database.clone(),
        target_database: target_actual.database.clone(),
        source_fingerprint: source_desired.content_fingerprint()?,
        target_fingerprint: target_actual.content_fingerprint()?,
        source_capture_fingerprint: source_desired.fingerprint()?,
        target_capture_fingerprint: target_actual.fingerprint()?,
        operations: b.operations.into_iter().map(|(_, op)| op).collect(),
        warnings: b.warnings.into_iter().collect(),
        implicit_commit: false,
    })
}
