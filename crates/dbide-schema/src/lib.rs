//! Offline PostgreSQL schema tools. This crate deliberately has no database connection or executor.
pub mod ddl;
pub mod er;
pub mod explain;
pub mod migration;
pub mod model;

pub use migration::{MigrationPlan, Operation, OperationKind, diff};
pub use model::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaError(pub String);
impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for SchemaError {}
impl From<serde_json::Error> for SchemaError {
    fn from(value: serde_json::Error) -> Self {
        Self(value.to_string())
    }
}
