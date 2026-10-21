use crate::pg_values::{IdentityValue, identity_value, validate_scalar};
use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Deliberately narrow PostgreSQL binding contract. Parameters are optional text-format
/// payloads bound as PostgreSQL TEXT with explicit casts to the target type. The executor
/// must bind, never interpolate or quote their values.
/// Closed casts cover ordinary scalar columns; arrays/custom types require future adapters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PgType {
    Text,
    Boolean,
    SmallInt,
    Integer,
    BigInt,
    Numeric,
    Bytea,
    Uuid,
    Date,
    Timestamp,
    TimestampTz,
    Json,
    Jsonb,
    DoublePrecision,
}
impl PgType {
    fn cast(self) -> &'static str {
        match self {
            Self::Text => "pg_catalog.text",
            Self::Boolean => "pg_catalog.bool",
            Self::SmallInt => "pg_catalog.int2",
            Self::Integer => "pg_catalog.int4",
            Self::BigInt => "pg_catalog.int8",
            Self::Numeric => "pg_catalog.numeric",
            Self::Bytea => "pg_catalog.bytea",
            Self::Uuid => "pg_catalog.uuid",
            Self::Date => "pg_catalog.date",
            Self::Timestamp => "pg_catalog.timestamp",
            Self::TimestampTz => "pg_catalog.timestamptz",
            Self::Json => "pg_catalog.json",
            Self::Jsonb => "pg_catalog.jsonb",
            Self::DoublePrecision => "pg_catalog.float8",
        }
    }
    fn accepts(self, cell: &Cell) -> bool {
        matches!(
            (self, cell),
            (_, Cell::Null)
                | (Self::Text, Cell::Text(_))
                | (Self::Boolean, Cell::Bool(_) | Cell::Text(_))
                | (
                    Self::SmallInt | Self::Integer | Self::BigInt,
                    Cell::Integer(_) | Cell::Text(_)
                )
                | (
                    Self::Numeric,
                    Cell::Decimal(_) | Cell::Integer(_) | Cell::Text(_)
                )
                | (Self::Bytea, Cell::Binary(_))
                | (
                    Self::Uuid
                        | Self::Date
                        | Self::Timestamp
                        | Self::TimestampTz
                        | Self::Json
                        | Self::Jsonb
                        | Self::DoublePrecision,
                    Cell::Text(_)
                )
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditColumn {
    pub name: String,
    pub pg_type: PgType,
    pub nullable: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityKind {
    PrimaryKey,
    UniqueConstraint,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowIdentity {
    /// Caller must obtain this enforced constraint from catalog metadata, never guess it.
    /// Verify built-in type semantics and deterministic text collations; custom/domain or
    /// nondeterministic collation equivalence is not normalized by this model.
    pub kind: IdentityKind,
    pub columns: Vec<usize>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableSpec {
    pub schema: String,
    pub name: String,
    pub columns: Vec<EditColumn>,
    /// None means read-only, including inserts in this initial edit surface.
    pub identity: Option<RowIdentity>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Edit {
    Insert {
        values: Vec<Cell>,
    },
    InsertDefaulted {
        values: Vec<InsertValue>,
    },
    Update {
        original: Vec<Cell>,
        replacement: Vec<Cell>,
    },
    Delete {
        original: Vec<Cell>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum InsertValue {
    Value(Cell),
    /// Server evaluates the catalog default/generated identity; never a raw SQL expression.
    Default,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PgParameter {
    /// Destination column type. The actual prepared-statement parameter type is TEXT;
    /// SQL casts TEXT to this destination, allowing an ordinary Option<String> binding.
    pub pg_type: PgType,
    /// None is SQL NULL; Some("") remains empty text. Decimal lexemes remain exact.
    pub value: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationKind {
    Insert,
    Update,
    Delete,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedStatement {
    pub kind: MutationKind,
    pub sql: String,
    pub parameters: Vec<PgParameter>,
    pub expected_affected_rows: u64,
    pub destructive: bool,
}
impl PlannedStatement {
    /// Executor must roll back the entire batch on zero/multiple affected rows. Zero is an
    /// optimistic conflict; multiple means the claimed row identity was invalid or changed.
    pub fn check_affected_rows(&self, actual: u64) -> Result<()> {
        if actual != self.expected_affected_rows {
            return Err(Error::Invalid(format!(
                "edit conflict: expected {} affected row, got {actual}; rollback batch",
                self.expected_affected_rows
            )));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionPlan {
    pub statements: Vec<PlannedStatement>,
    /// Execute only after a separate, explicit UI submit gesture and parameter-capable driver.
    pub requires_explicit_submit: bool,
    /// BEGIN -> bind/execute/check each -> COMMIT. Any error/conflict: ROLLBACK.
    /// Commit failure or disconnect: outcome unknown; never automatically retry the batch.
    pub requires_transaction: bool,
    pub destructive: bool,
}

/// Builds preview-only SQL. Uses fully quoted identifiers, closed-set type casts and bound
/// parameters. Original values for EVERY column participate in optimistic predicates using
/// IS NOT DISTINCT FROM. This is value-based conflict detection (cannot detect ABA changes).
/// No SQL executes here; current simple-protocol query APIs cannot execute these plans safely.
pub fn plan_edits(table: &TableSpec, edits: &[Edit], limits: Limits) -> Result<TransactionPlan> {
    limits.validate()?;
    if edits.len() > limits.max_rows {
        return Err(Error::Limit("edit count".into()));
    }
    let identity = table
        .identity
        .as_ref()
        .ok_or_else(|| Error::Invalid("read-only: no verified primary/unique identity".into()))?;
    validate_key_columns(&identity.columns, table.columns.len())?;
    if identity.columns.iter().any(|&index| {
        matches!(
            table.columns[index].pg_type,
            PgType::DoublePrecision
                | PgType::Timestamp
                | PgType::TimestampTz
                | PgType::Json
                | PgType::Jsonb
        )
    }) {
        return Err(Error::Invalid(
            "read-only: float/timestamp/JSON identity requires PostgreSQL-equivalent key normalization".into(),
        ));
    }
    let qualified = format!(
        "{}.{}",
        quote_identifier(&table.schema)?,
        quote_identifier(&table.name)?
    );
    let mut names = HashSet::new();
    let mut byte_count = 0;
    add_bytes(&mut byte_count, qualified.len(), limits)?;
    for column in &table.columns {
        quote_identifier(&column.name)?;
        if !names.insert(&column.name) {
            return Err(Error::Invalid("duplicate edit column names".into()));
        }
        add_bytes(&mut byte_count, column.name.len().max(1), limits)?;
    }
    let mut statements = Vec::new();
    let mut targets = HashSet::new();
    for edit in edits {
        let (original, values) = match edit {
            Edit::Insert { values } => (None, Some(values)),
            Edit::InsertDefaulted { .. } => (None, None),
            Edit::Update {
                original,
                replacement,
            } => (Some(original), Some(replacement)),
            Edit::Delete { original } => (Some(original), None),
        };
        for row in original.into_iter().chain(values) {
            validate_edit_row(table, row, identity, &mut byte_count, limits)?;
        }
        if let Edit::InsertDefaulted { values } = edit {
            if values.len() != table.columns.len() {
                return Err(Error::Invalid("row/column arity mismatch".into()));
            }
            for (index, (value, column)) in values.iter().zip(&table.columns).enumerate() {
                match value {
                    InsertValue::Default => add_bytes(&mut byte_count, 1, limits)?,
                    InsertValue::Value(value) => {
                        value.validate()?;
                        add_bytes(&mut byte_count, value.payload_bytes(), limits)?;
                        validate_edit_cell(value, column)?;
                        if *value == Cell::Null && identity.columns.contains(&index) {
                            return Err(Error::Invalid(
                                "NULL primary/unique key cannot identify an editable row".into(),
                            ));
                        }
                    }
                }
            }
        }
        if let Some(row) = original {
            let key: Result<Vec<IdentityValue>> = identity
                .columns
                .iter()
                .map(|&i| identity_value(&row[i], table.columns[i].pg_type))
                .collect();
            if !targets.insert(key?) {
                return Err(Error::Invalid(
                    "multiple edits target the same original identity".into(),
                ));
            }
        }
        let mut parameters = Vec::new();
        let (kind, sql, destructive) = match edit {
            Edit::Insert { values } => {
                let names: Result<Vec<String>> = table
                    .columns
                    .iter()
                    .map(|c| quote_identifier(&c.name))
                    .collect();
                let placeholders: Result<Vec<String>> = values
                    .iter()
                    .zip(&table.columns)
                    .map(|(value, column)| bind(value, column.pg_type, &mut parameters))
                    .collect();
                (
                    MutationKind::Insert,
                    format!(
                        "INSERT INTO {qualified} ({}) VALUES ({})",
                        names?.join(", "),
                        placeholders?.join(", ")
                    ),
                    false,
                )
            }
            Edit::InsertDefaulted { values } => {
                let names: Result<Vec<String>> = table
                    .columns
                    .iter()
                    .map(|column| quote_identifier(&column.name))
                    .collect();
                let placeholders: Result<Vec<String>> = values
                    .iter()
                    .zip(&table.columns)
                    .map(|(value, column)| match value {
                        InsertValue::Default => Ok("DEFAULT".into()),
                        InsertValue::Value(value) => bind(value, column.pg_type, &mut parameters),
                    })
                    .collect();
                (
                    MutationKind::Insert,
                    format!(
                        "INSERT INTO {qualified} ({}) VALUES ({})",
                        names?.join(", "),
                        placeholders?.join(", ")
                    ),
                    false,
                )
            }
            Edit::Update {
                original,
                replacement,
            } => {
                let mut assignments = Vec::new();
                for (i, column) in table.columns.iter().enumerate() {
                    if original[i] != replacement[i] {
                        assignments.push(format!(
                            "{} = {}",
                            quote_identifier(&column.name)?,
                            bind(&replacement[i], column.pg_type, &mut parameters)?
                        ));
                    }
                }
                if assignments.is_empty() {
                    continue;
                }
                let predicate = original_predicate(table, identity, original, &mut parameters)?;
                (
                    MutationKind::Update,
                    format!(
                        "UPDATE {qualified} SET {} WHERE {predicate}",
                        assignments.join(", ")
                    ),
                    true,
                )
            }
            Edit::Delete { original } => {
                let predicate = original_predicate(table, identity, original, &mut parameters)?;
                (
                    MutationKind::Delete,
                    format!("DELETE FROM {qualified} WHERE {predicate}"),
                    true,
                )
            }
        };
        add_bytes(&mut byte_count, sql.len(), limits)?;
        for parameter in &parameters {
            add_bytes(
                &mut byte_count,
                parameter
                    .value
                    .as_ref()
                    .map_or(1, |value| value.len().max(1)),
                limits,
            )?;
        }
        statements.push(PlannedStatement {
            kind,
            sql,
            parameters,
            expected_affected_rows: 1,
            destructive,
        });
    }
    Ok(TransactionPlan {
        destructive: statements.iter().any(|s| s.destructive),
        statements,
        requires_explicit_submit: true,
        requires_transaction: true,
    })
}
fn validate_edit_row(
    table: &TableSpec,
    row: &[Cell],
    identity: &RowIdentity,
    bytes: &mut usize,
    limits: Limits,
) -> Result<()> {
    validate_row(row, table.columns.len(), bytes, limits)?;
    for (value, column) in row.iter().zip(&table.columns) {
        validate_edit_cell(value, column)?;
    }
    if identity.columns.iter().any(|&i| row[i] == Cell::Null) {
        return Err(Error::Invalid(
            "NULL primary/unique key cannot identify an editable row".into(),
        ));
    }
    Ok(())
}
fn validate_edit_cell(value: &Cell, column: &EditColumn) -> Result<()> {
    if (!column.nullable && *value == Cell::Null) || !column.pg_type.accepts(value) {
        return Err(Error::Invalid(format!(
            "NULL/type mismatch for column {:?}; explicit conversion required",
            column.name
        )));
    }
    if let Cell::Text(text) = value
        && text.contains('\0')
    {
        return Err(Error::Invalid("PostgreSQL text cannot contain NUL".into()));
    }
    validate_scalar(value, column.pg_type)?;
    Ok(())
}
fn original_predicate(
    table: &TableSpec,
    identity: &RowIdentity,
    original: &[Cell],
    parameters: &mut Vec<PgParameter>,
) -> Result<String> {
    let mut order = identity.columns.clone();
    order.extend((0..table.columns.len()).filter(|i| !identity.columns.contains(i)));
    let mut predicates = Vec::new();
    for i in order {
        let column = &table.columns[i];
        if column.pg_type == PgType::Json {
            // PostgreSQL json has no equality operator. Its text representation retains
            // whitespace/key order, so compare exact stored text without a json cast.
            predicates.push(format!(
                "{}::pg_catalog.text IS NOT DISTINCT FROM {}",
                quote_identifier(&column.name)?,
                bind(&original[i], PgType::Text, parameters)?
            ));
            continue;
        }
        predicates.push(format!(
            "{} IS NOT DISTINCT FROM {}",
            quote_identifier(&column.name)?,
            bind(&original[i], column.pg_type, parameters)?
        ));
    }
    Ok(predicates.join(" AND "))
}
fn quote_identifier(name: &str) -> Result<String> {
    // Standard PostgreSQL NAMEDATALEN is 64, including the terminating byte.
    // Quoting does not stop server truncation; refusing oversized UTF-8 names
    // prevents a preview from silently targeting a different catalog object.
    if name.is_empty() || name.contains('\0') || name.len() > 63 {
        return Err(Error::Invalid(
            "SQL identifier must be nonempty, NUL-free and at most 63 UTF-8 bytes".into(),
        ));
    }
    Ok(format!("\"{}\"", name.replace('"', "\"\"")))
}
fn bind(value: &Cell, pg_type: PgType, parameters: &mut Vec<PgParameter>) -> Result<String> {
    let value = match value {
        Cell::Null => None,
        Cell::Text(text) | Cell::Decimal(text) => Some(text.clone()),
        Cell::Bool(value) => Some(value.to_string()),
        Cell::Integer(value) => Some(value.to_string()),
        Cell::Binary(value) => {
            use std::fmt::Write;
            let mut hex = String::from("\\x");
            for byte in value {
                write!(&mut hex, "{byte:02x}").expect("write to String");
            }
            Some(hex)
        }
    };
    parameters.push(PgParameter { pg_type, value });
    if parameters.len() > 65_535 {
        return Err(Error::Limit("PostgreSQL parameter count".into()));
    }
    let placeholder = format!("${}::pg_catalog.text", parameters.len());
    Ok(if pg_type == PgType::Text {
        placeholder
    } else {
        format!("{placeholder}::{}", pg_type.cast())
    })
}
