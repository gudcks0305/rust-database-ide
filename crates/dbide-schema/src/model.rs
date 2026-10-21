use crate::{SchemaError, ddl};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const SNAPSHOT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseIdentity {
    pub engine: String,
    /// Stable identity of this database/connection, never a credential-bearing connection URL.
    pub identity: String,
    pub name: String,
    pub server_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Provenance {
    Live {
        captured_at: String,
        generation: u64,
    },
    OfflineDdl {
        label: String,
    },
    Imported {
        label: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaSnapshot {
    pub format_version: u32,
    pub database: DatabaseIdentity,
    pub provenance: Provenance,
    /// Complete inside `scope`; incomplete/filtered catalogs must set false.
    pub complete: bool,
    /// Authoritative schema scope, including an absent schema intended to be created/dropped.
    pub scope: Vec<String>,
    pub schemas: Vec<Schema>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schema {
    pub name: String,
    pub tables: Vec<Table>,
    pub views: Vec<View>,
    /// Objects outside this model (sequences, routines, domains, grants, extensions, etc.).
    /// Their presence prevents destructive schema drops.
    #[serde(default)]
    pub unsupported_objects: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualifiedName {
    pub schema: String,
    pub name: String,
}
impl QualifiedName {
    pub fn new(schema: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            schema: schema.into(),
            name: name.into(),
        }
    }
    pub fn sql(&self) -> String {
        format!("{}.{}", quote_ident(&self.schema), quote_ident(&self.name))
    }
    /// Length-delimited components prevent collisions such as `a.b/c` versus `a/b.c`.
    pub fn id(&self) -> String {
        format!(
            "{}:{}{}:{}",
            self.schema.len(),
            self.schema,
            self.name.len(),
            self.name
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Table {
    pub name: String,
    pub columns: Vec<Column>,
    pub primary_key: Option<PrimaryKey>,
    pub foreign_keys: Vec<ForeignKey>,
    pub indexes: Vec<Index>,
    /// Preserve metadata that the migration generator cannot safely reproduce.
    #[serde(default)]
    pub unsupported_features: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Column {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub default: Option<String>,
    /// Identity/generated columns cannot be recreated as ordinary columns.
    #[serde(default)]
    pub generated: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrimaryKey {
    pub name: Option<String>,
    pub columns: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForeignKey {
    pub name: Option<String>,
    pub columns: Vec<String>,
    pub references: QualifiedName,
    pub referenced_columns: Vec<String>,
    pub on_delete: ReferentialAction,
    pub on_update: ReferentialAction,
    pub deferrable: bool,
    pub initially_deferred: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReferentialAction {
    #[default]
    NoAction,
    Restrict,
    Cascade,
    SetNull,
    SetDefault,
}
impl ReferentialAction {
    pub fn sql(self) -> &'static str {
        match self {
            Self::NoAction => "NO ACTION",
            Self::Restrict => "RESTRICT",
            Self::Cascade => "CASCADE",
            Self::SetNull => "SET NULL",
            Self::SetDefault => "SET DEFAULT",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Index {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    pub method: String,
    pub predicate: Option<String>,
    pub expressions: Vec<String>,
    pub include: Vec<String>,
    /// Constraint-owned indexes must not be dropped separately.
    pub constraint_owned: bool,
    #[serde(default)]
    pub unsupported_features: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    pub name: String,
    pub definition: String,
    pub materialized: bool,
}

pub fn quote_ident(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
pub fn quote_columns(values: &[String]) -> String {
    values
        .iter()
        .map(|s| quote_ident(s))
        .collect::<Vec<_>>()
        .join(", ")
}
fn valid_ident(value: &str) -> Result<(), SchemaError> {
    if value.is_empty() || value.contains('\0') || value.len() > 63 {
        return Err(SchemaError("identifier must contain 1..63 UTF-8 bytes and no NUL (PostgreSQL default identifier limit)".into()));
    }
    Ok(())
}
fn unique<'a>(values: impl IntoIterator<Item = &'a str>, context: &str) -> Result<(), SchemaError> {
    let mut found = BTreeSet::new();
    for value in values {
        valid_ident(value)?;
        if !found.insert(value) {
            return Err(SchemaError(format!("duplicate {context}: {value}")));
        }
    }
    Ok(())
}
fn column_refs(columns: &[String], table: &Table) -> Result<(), SchemaError> {
    if columns.is_empty() {
        return Err(SchemaError(format!("empty constraint on {}", table.name)));
    }
    unique(columns.iter().map(String::as_str), "constraint column")?;
    for name in columns {
        if !table.columns.iter().any(|c| &c.name == name) {
            return Err(SchemaError(format!(
                "unknown column {name} on {}",
                table.name
            )));
        }
    }
    Ok(())
}
impl SchemaSnapshot {
    pub fn empty(database: DatabaseIdentity, provenance: Provenance, scope: Vec<String>) -> Self {
        Self {
            format_version: SNAPSHOT_VERSION,
            database,
            provenance,
            complete: true,
            scope,
            schemas: vec![],
        }
    }
    pub fn validate(&self) -> Result<(), SchemaError> {
        if self.format_version != SNAPSHOT_VERSION {
            return Err(SchemaError("unsupported schema snapshot version".into()));
        }
        if self.database.engine != "postgresql" {
            return Err(SchemaError("only postgresql snapshots supported".into()));
        }
        if self.database.identity.is_empty() || self.database.identity.contains('\0') {
            return Err(SchemaError(
                "database capture identity is empty/invalid".into(),
            ));
        }
        valid_ident(&self.database.name)?;
        unique(self.scope.iter().map(String::as_str), "scope")?;
        unique(self.schemas.iter().map(|s| s.name.as_str()), "schema")?;
        for schema in &self.schemas {
            if !self.scope.contains(&schema.name) {
                return Err(SchemaError(format!(
                    "schema {} is outside snapshot scope",
                    schema.name
                )));
            }
            unique(
                schema
                    .tables
                    .iter()
                    .map(|t| t.name.as_str())
                    .chain(schema.views.iter().map(|v| v.name.as_str())),
                "relation",
            )?;
            let mut index_names = BTreeSet::new();
            for table in &schema.tables {
                unique(table.columns.iter().map(|c| c.name.as_str()), "column")?;
                unique(
                    table
                        .primary_key
                        .iter()
                        .filter_map(|p| p.name.as_deref())
                        .chain(table.foreign_keys.iter().filter_map(|f| f.name.as_deref())),
                    "constraint",
                )?;
                for column in &table.columns {
                    ddl::column_sql(column)?;
                }
                if let Some(pk) = &table.primary_key {
                    column_refs(&pk.columns, table)?;
                    if pk
                        .columns
                        .iter()
                        .any(|n| table.columns.iter().any(|c| &c.name == n && c.nullable))
                    {
                        return Err(SchemaError(
                            "primary key columns must be non-nullable".into(),
                        ));
                    }
                }
                for fk in &table.foreign_keys {
                    column_refs(&fk.columns, table)?;
                    valid_ident(&fk.references.schema)?;
                    valid_ident(&fk.references.name)?;
                    unique(
                        fk.referenced_columns.iter().map(String::as_str),
                        "referenced column",
                    )?;
                    if fk.columns.len() != fk.referenced_columns.len() {
                        return Err(SchemaError("foreign key column counts differ".into()));
                    }
                    if fk.initially_deferred && !fk.deferrable {
                        return Err(SchemaError("initially deferred requires deferrable".into()));
                    }
                }
                for index in &table.indexes {
                    valid_ident(&index.name)?;
                    if !index_names.insert(&index.name) {
                        return Err(SchemaError(format!(
                            "duplicate index {} in schema {}",
                            index.name, schema.name
                        )));
                    }
                    if !index.columns.is_empty() {
                        column_refs(&index.columns, table)?;
                    }
                    if schema.tables.iter().any(|t| t.name == index.name)
                        || schema.views.iter().any(|v| v.name == index.name)
                    {
                        return Err(SchemaError("index collides with relation name".into()));
                    }
                }
            }
        }
        Ok(())
    }
    pub fn to_json(&self) -> Result<String, SchemaError> {
        self.validate()?;
        Ok(serde_json::to_string_pretty(self)?)
    }
    pub fn from_json(value: &str) -> Result<Self, SchemaError> {
        if value.len() > 32_000_000 {
            return Err(SchemaError("snapshot JSON exceeds 32 MB input cap".into()));
        }
        let result: Self = serde_json::from_str(value)?;
        result.validate()?;
        Ok(result)
    }
    /// Stable content AND capture identity fingerprint. No implicit sorting of column order.
    pub fn fingerprint(&self) -> Result<String, SchemaError> {
        self.hash_canonical(true)
    }
    /// Semantic schema identity, excluding capture time/generation/offline labels.
    /// Stable database identity, scope, completeness and unsupported metadata remain included.
    pub fn content_fingerprint(&self) -> Result<String, SchemaError> {
        self.hash_canonical(false)
    }
    fn hash_canonical(&self, include_provenance: bool) -> Result<String, SchemaError> {
        self.validate()?;
        let mut canonical = self.clone();
        canonical.scope.sort();
        canonical.schemas.sort_by(|a, b| a.name.cmp(&b.name));
        for s in &mut canonical.schemas {
            s.tables.sort_by(|a, b| a.name.cmp(&b.name));
            s.views.sort_by(|a, b| a.name.cmp(&b.name));
            s.unsupported_objects.sort();
            for t in &mut s.tables {
                t.foreign_keys
                    .sort_by_key(|f| serde_json::to_string(f).unwrap_or_default());
                t.indexes.sort_by(|a, b| a.name.cmp(&b.name));
                t.unsupported_features.sort();
            }
        }
        let mut value = serde_json::to_value(&canonical)?;
        if !include_provenance {
            value
                .as_object_mut()
                .expect("snapshot object")
                .remove("provenance");
        }
        Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&value)?)))
    }
    pub fn tables(&self) -> impl Iterator<Item = (QualifiedName, &Table)> {
        self.schemas.iter().flat_map(|s| {
            s.tables
                .iter()
                .map(move |t| (QualifiedName::new(&s.name, &t.name), t))
        })
    }
}
