//! Stable table-level catalog markers. They describe missing migration metadata, not a blanket
//! CRUD authorization decision. Consumers must explicitly allow known safe cases and reject unknowns.
//! Column.generated separately preserves identity:a/d and generated:s/v:<expression> modes.
pub const COLUMN_IDENTITY: &str = "column identity";
pub const COLUMN_GENERATED: &str = "column generated";
pub const COLUMN_PRIVILEGES: &str = "column privileges";
pub const COLUMN_COMMENT: &str = "column comment";
pub const COLUMN_CUSTOM_COLLATION: &str = "column custom collation";
pub const COLUMN_NONDETERMINISTIC_COLLATION: &str = "column nondeterministic collation";
pub const CONSTRAINT_CHECK: &str = "constraint check";
pub const CONSTRAINT_UNIQUE: &str = "constraint unique";
pub const CONSTRAINT_EXCLUSION: &str = "constraint exclusion";
pub const CONSTRAINT_NOT_NULL: &str = "constraint not null";
pub const CONSTRAINT_UNKNOWN_KIND: &str = "constraint unknown kind";
pub const CONSTRAINT_UNVALIDATED: &str = "constraint unvalidated";
pub const CONSTRAINT_NOT_ENFORCED: &str = "constraint not enforced";
pub const CONSTRAINT_DEFERRABLE: &str = "constraint deferrable";
pub const PRIMARY_KEY_DEFERRABLE: &str = "primary key deferrable";
pub const PRIMARY_KEY_INCLUDE_COLUMNS: &str = "primary key include columns";
pub const FOREIGN_KEY_NON_SIMPLE_MATCH: &str = "foreign key non-simple match";
pub const CONSTRAINT_TEMPORAL_PERIOD: &str = "constraint temporal/period";
pub const FOREIGN_KEY_DELETE_COLUMN_SUBSET: &str = "foreign key delete-column subset";

/// Target-only capture checks incoming CASCADE/SET NULL/SET DEFAULT actions across ALL namespaces.
/// A CRUD consumer must reject these effects unless it implements a broader mutation preview.
pub const INCOMING_MUTATING_FOREIGN_KEY: &str = "incoming mutating foreign key";

/// An ordinary table owns rewrite rules (ALSO/INSTEAD actions). Row editing must remain read-only;
/// inspect the rule definitions before choosing an explicitly broader execution workflow.
/// Views' normal _RETURN rules do not produce this table marker.
pub const USER_REWRITE_RULES: &str = "user rewrite rules";
