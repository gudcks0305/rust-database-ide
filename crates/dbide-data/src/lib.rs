//! Bounded, lossless transfer models, explicit-key data comparison and preview-only edits.
//! No function in this crate connects to a database or executes SQL.
mod diff;
mod edit;
mod pg_values;
mod table_query;
mod transfer;

pub use diff::*;
pub use edit::*;
use serde::{Deserialize, Serialize};
use std::fmt;
pub use table_query::*;
pub use transfer::*;

/// Decimal is a validated exact decimal lexeme, never a floating point number.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Cell {
    Null,
    Text(String),
    Bool(bool),
    Integer(i64),
    Decimal(String),
    Binary(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    /// Positional result label, not a unique database identifier. Duplicate/empty
    /// SELECT labels are preserved; destination mapping/edit names are stricter.
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabularData {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Cell>>,
}

/// Required finite row and logical payload byte budgets. Input decoders also cap raw bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub max_rows: usize,
    pub max_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_rows: 10_000,
            max_bytes: 16 * 1024 * 1024,
        }
    }
}
impl Limits {
    pub fn validate(self) -> Result<()> {
        if self.max_rows == 0 || self.max_bytes == 0 || self.max_bytes == usize::MAX {
            return Err(Error::Invalid(
                "limits must be positive and byte limit finite".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum Error {
    Invalid(String),
    Limit(String),
    Io(std::io::Error),
    Csv(csv::Error),
    Json(serde_json::Error),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(e) => write!(f, "invalid data: {e}"),
            Self::Limit(e) => write!(f, "budget exceeded: {e}"),
            Self::Io(e) => write!(f, "I/O: {e}"),
            Self::Csv(e) => write!(f, "CSV: {e}"),
            Self::Json(e) => write!(f, "JSON: {e}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<csv::Error> for Error {
    fn from(e: csv::Error) -> Self {
        Self::Csv(e)
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}
pub type Result<T> = std::result::Result<T, Error>;

impl Cell {
    pub fn validate(&self) -> Result<()> {
        if let Self::Decimal(value) = self
            && !valid_decimal(value)
        {
            return Err(Error::Invalid(format!("invalid exact decimal {value:?}")));
        }
        Ok(())
    }
    pub fn kind(&self) -> CellKind {
        match self {
            Self::Null => CellKind::Null,
            Self::Text(_) => CellKind::Text,
            Self::Bool(_) => CellKind::Bool,
            Self::Integer(_) => CellKind::Integer,
            Self::Decimal(_) => CellKind::Decimal,
            Self::Binary(_) => CellKind::Binary,
        }
    }
    fn payload_bytes(&self) -> usize {
        match self {
            Self::Null => 1,
            Self::Text(v) | Self::Decimal(v) => v.len().max(1),
            Self::Binary(v) => v.len().max(1),
            Self::Bool(_) => 1,
            Self::Integer(_) => 8,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CellKind {
    Null,
    Text,
    Bool,
    Integer,
    Decimal,
    Binary,
}

fn valid_decimal(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = usize::from(bytes.first() == Some(&b'-'));
    let start = i;
    while bytes.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    if i == start {
        return false;
    }
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        let fraction = i;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == fraction {
            return false;
        }
    }
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(bytes.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let exponent = i;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == exponent {
            return false;
        }
    }
    i == bytes.len()
}

fn add_bytes(total: &mut usize, additional: usize, limits: Limits) -> Result<()> {
    *total = total
        .checked_add(additional)
        .ok_or_else(|| Error::Limit("byte count overflow".into()))?;
    if *total > limits.max_bytes {
        return Err(Error::Limit("payload bytes".into()));
    }
    Ok(())
}
fn validate_columns(columns: &[Column], limits: Limits) -> Result<usize> {
    limits.validate()?;
    if columns.is_empty() {
        return Err(Error::Invalid("at least one column required".into()));
    }
    let mut bytes = 0;
    for column in columns {
        add_bytes(&mut bytes, column.name.len().max(1), limits)?;
    }
    Ok(bytes)
}
fn validate_row(row: &[Cell], width: usize, bytes: &mut usize, limits: Limits) -> Result<()> {
    if row.len() != width {
        return Err(Error::Invalid("row/column arity mismatch".into()));
    }
    for cell in row {
        cell.validate()?;
        add_bytes(bytes, cell.payload_bytes(), limits)?;
    }
    Ok(())
}
impl TabularData {
    pub fn validate(&self, limits: Limits) -> Result<()> {
        let mut bytes = validate_columns(&self.columns, limits)?;
        if self.rows.len() > limits.max_rows {
            return Err(Error::Limit("row count".into()));
        }
        for row in &self.rows {
            validate_row(row, self.columns.len(), &mut bytes, limits)?;
        }
        Ok(())
    }
}
