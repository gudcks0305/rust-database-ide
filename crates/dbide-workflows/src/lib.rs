//! Dedicated read-only PostgreSQL sessions for bounded catalog capture and plan-only EXPLAIN.
//! No migration execution; credentials and caller SQL are never included in errors.
use dbide_pg_spike::{
    CommitPolicy, ConnectionDiagnostic, Event, ExecutionReport, Limits, Outcome, Session,
};
use dbide_profiles::ConnectionRequest;
use dbide_schema::{
    DatabaseIdentity, ForeignKey, Index, MigrationPlan, PrimaryKey, Provenance, QualifiedName,
    Schema, SchemaError, SchemaSnapshot, Table, View, ddl,
    explain::{ExecutionMode, ExplainDocument},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlparser::{ast::Statement, dialect::PostgreSqlDialect, parser::Parser};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub mod metadata_features;
mod view_capture;
pub use view_capture::{CapturedView, capture_view};

static CAPTURES: AtomicU64 = AtomicU64::new(1);
#[derive(Debug, Clone, Copy)]
pub struct CaptureLimits {
    pub max_records: usize,
    pub max_relations: usize,
    pub max_columns: usize,
    pub max_bytes: usize,
    pub timeout_secs: u64,
}
impl Default for CaptureLimits {
    fn default() -> Self {
        Self {
            max_records: 20_000,
            max_relations: 500,
            max_columns: 10_000,
            max_bytes: 8_000_000,
            timeout_secs: 15,
        }
    }
}
impl CaptureLimits {
    fn validate(self) -> Result<(), WorkflowError> {
        if self.max_records == 0
            || self.max_records > 50_000
            || self.max_relations == 0
            || self.max_relations > 2000
            || self.max_columns == 0
            || self.max_columns > 30_000
            || !(1024..=32_000_000).contains(&self.max_bytes)
            || !(1..=120).contains(&self.timeout_secs)
        {
            return Err(WorkflowError::Configuration(
                "capture limits outside allowed bounds",
            ));
        }
        Ok(())
    }
    fn driver(self) -> Limits {
        Limits {
            retained_bytes: self.max_bytes,
            events: self.max_records + 100,
            sql_bytes: 128_000,
        }
    }
}
#[derive(Debug)]
pub enum WorkflowError {
    Configuration(&'static str),
    Connection(Box<ConnectionDiagnostic>),
    Database { sqlstate: Option<String> },
    Truncated,
    Timeout,
    InvalidCatalog(SchemaError),
}
impl std::fmt::Display for WorkflowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Configuration(m) => f.write_str(m),
            Self::Connection(d) => f.write_str(d.message),
            Self::Database { sqlstate } => write!(
                f,
                "PostgreSQL workflow failed (SQLSTATE {})",
                sqlstate.as_deref().unwrap_or("unknown")
            ),
            Self::Truncated => {
                f.write_str("catalog/plan result exceeded its cap; no complete result returned")
            }
            Self::Timeout => {
                f.write_str("read-only PostgreSQL workflow timed out; dedicated connection retired")
            }
            Self::InvalidCatalog(e) => write!(f, "catalog/plan decoding failed: {e}"),
        }
    }
}
impl std::error::Error for WorkflowError {}
impl From<SchemaError> for WorkflowError {
    fn from(value: SchemaError) -> Self {
        Self::InvalidCatalog(value)
    }
}
impl From<serde_json::Error> for WorkflowError {
    fn from(value: serde_json::Error) -> Self {
        Self::InvalidCatalog(SchemaError(value.to_string()))
    }
}
struct DedicatedSession(Session);
impl Drop for DedicatedSession {
    fn drop(&mut self) {
        self.0.close();
    }
}
async fn execute(
    request: &ConnectionRequest,
    sql: &str,
    limits: CaptureLimits,
) -> Result<ExecutionReport, WorkflowError> {
    let session = DedicatedSession(
        Session::connect_request(request, limits.driver())
            .await
            .map_err(|e| WorkflowError::Connection(Box::new(e)))?,
    );
    execute_on_session(&session.0, sql, limits).await
}
async fn execute_on_session(
    session: &Session,
    sql: &str,
    limits: CaptureLimits,
) -> Result<ExecutionReport, WorkflowError> {
    let report = tokio::time::timeout(
        Duration::from_secs(limits.timeout_secs),
        session.execute(sql, CommitPolicy::Auto),
    )
    .await
    .map_err(|_| WorkflowError::Timeout)?
    .map_err(|_| WorkflowError::Configuration("dedicated session could not submit workflow"))?;
    validate_report(&report)?;
    Ok(report)
}
fn validate_report(report: &ExecutionReport) -> Result<(), WorkflowError> {
    if report.outcome != Outcome::Completed
        || report.terminal_error.is_some()
        || report.connection_retired
        || report
            .events
            .iter()
            .any(|e| matches!(e, Event::Error { .. }))
    {
        return Err(WorkflowError::Database {
            sqlstate: report
                .terminal_error
                .as_ref()
                .and_then(|e| e.sqlstate.clone()),
        });
    }
    if report.truncated {
        return Err(WorkflowError::Truncated);
    }
    Ok(())
}
fn validated_scope(scope: &[String]) -> Result<Vec<String>, WorkflowError> {
    if scope.is_empty() || scope.len() > 32 {
        return Err(WorkflowError::Configuration(
            "choose 1..32 explicit user schemas",
        ));
    }
    let mut set = BTreeSet::new();
    for name in scope {
        if name.is_empty()
            || name.len() > 63
            || name.contains('\0')
            || name == "information_schema"
            || name.starts_with("pg_")
            || !set.insert(name.clone())
        {
            return Err(WorkflowError::Configuration(
                "schema scope contains duplicate, system, empty, NUL or overlong names",
            ));
        }
    }
    Ok(set.into_iter().collect())
}
/// E'' literals remain unambiguous regardless of incoming standard_conforming_strings.
fn sql_literal(value: &str) -> String {
    format!("E'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
}
fn capture_sql(scope: &[String], limits: CaptureLimits) -> String {
    catalog_sql(scope, None, limits)
}
fn catalog_sql(scope: &[String], target: Option<&str>, limits: CaptureLimits) -> String {
    let template = include_str!("catalog.sql").replace(
        "__SCHEMA_OBJECTS__",
        if target.is_none() {
            include_str!("catalog_schema_objects.sql")
        } else {
            ""
        },
    );
    let scope = scope
        .iter()
        .map(|s| sql_literal(s))
        .collect::<Vec<_>>()
        .join(", ");
    let relation_filter = target
        .map(|name| format!("c.relname = {}", sql_literal(name)))
        .unwrap_or_else(|| "TRUE".into());
    let limit = (limits.max_records + 1).to_string();
    let timeout = (limits.timeout_secs * 1000).to_string();
    let replacements = [
        ("__SCOPE__", scope.as_str()),
        ("__RELATION_FILTER__", relation_filter.as_str()),
        ("__LIMIT__", limit.as_str()),
        ("__TIMEOUT_MS__", timeout.as_str()),
        (
            "__TARGETED_CAPTURE__",
            if target.is_some() { "TRUE" } else { "FALSE" },
        ),
    ];
    // Replace tokens in the trusted template once; never rescan inserted identifier literals.
    // Valid PostgreSQL names may themselves contain strings such as __LIMIT__.
    let mut remaining = template.as_str();
    let mut rendered = String::with_capacity(template.len() + scope.len());
    loop {
        let next = replacements
            .iter()
            .filter_map(|(token, value)| remaining.find(token).map(|i| (i, *token, *value)))
            .min_by_key(|(i, _, _)| *i);
        let Some((i, token, value)) = next else {
            rendered.push_str(remaining);
            break;
        };
        rendered.push_str(&remaining[..i]);
        rendered.push_str(value);
        remaining = &remaining[i + token.len()..];
    }
    rendered
}
/// Captures authoritative catalog metadata inside the explicit scope, using a new dedicated session.
/// Unsupported metadata is preserved as markers; caps/errors return no partial snapshot.
pub async fn capture_schema(
    request: &ConnectionRequest,
    scope: &[String],
    limits: CaptureLimits,
) -> Result<SchemaSnapshot, WorkflowError> {
    limits.validate()?;
    let scope = validated_scope(scope)?;
    let report = execute(request, &capture_sql(&scope, limits), limits).await?;
    snapshot_from_report(request, &scope, limits, &report)
}
/// Atomic metadata/identity capture for the table edit workflow. OID is connection/database-local
/// evidence and deliberately remains outside SchemaSnapshot and semantic migration fingerprints.
#[derive(Debug, Clone)]
pub struct CapturedTableForEdit {
    pub snapshot: SchemaSnapshot,
    pub relation_oid: u32,
    /// Required canonical metadata hash shared with the execution layer's post-lock guard.
    pub metadata_fingerprint: String,
}
/// A complete exact-table edit capture inside an explicitly PARTIAL schema snapshot.
/// The function validates all target columns/PK/outgoing FKs/indexes and incoming-action risk
/// markers, plus the canonical OID/metadata guard. `snapshot.complete` is always false:
/// this type does not assert that unrelated tables/schema objects were captured.
#[derive(Debug, Clone)]
pub struct CapturedTargetTableForEdit {
    pub snapshot: SchemaSnapshot,
    pub relation_oid: u32,
    pub metadata_fingerprint: String,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum CaptureExtent {
    Schema,
    TargetTable,
}
/// Additive targeted variant. Same identity/transaction contract as capture_table_for_edit;
/// unrelated schema objects do not consume target limits. Migration diff must reject its
/// incomplete schema snapshot. Consumers need an explicit target-capture constructor/policy.
pub async fn capture_target_table_for_edit(
    request: &ConnectionRequest,
    schema: &str,
    table: &str,
    limits: CaptureLimits,
) -> Result<CapturedTargetTableForEdit, WorkflowError> {
    let capture =
        capture_edit_inner(request, schema, table, limits, CaptureExtent::TargetTable).await?;
    if capture.snapshot.complete
        || capture.snapshot.tables().count() != 1
        || !capture
            .snapshot
            .tables()
            .all(|(name, _)| name.schema == schema && name.name == table)
        || capture
            .snapshot
            .schemas
            .iter()
            .any(|s| !s.views.is_empty() || !s.unsupported_objects.is_empty())
    {
        return Err(invalid(
            "target capture did not preserve exact partial-schema coverage",
        ));
    }
    Ok(CapturedTargetTableForEdit {
        snapshot: capture.snapshot,
        relation_oid: capture.relation_oid,
        metadata_fingerprint: capture.metadata_fingerprint,
    })
}
/// Captures the schema and exact ordinary nonpartitioned table OID from the SAME bounded
/// repeatable-read READ ONLY catalog transaction. Works for empty tables and never samples rows.
/// This is identity evidence, not permission to edit; the caller must still enforce CRUD policy
/// and recheck both OID and metadata fingerprint atomically after locking, before DML on
/// its actual execution connection.
pub async fn capture_table_for_edit(
    request: &ConnectionRequest,
    schema: &str,
    table: &str,
    limits: CaptureLimits,
) -> Result<CapturedTableForEdit, WorkflowError> {
    capture_edit_inner(request, schema, table, limits, CaptureExtent::Schema).await
}
async fn capture_edit_inner(
    request: &ConnectionRequest,
    schema: &str,
    table: &str,
    limits: CaptureLimits,
    extent: CaptureExtent,
) -> Result<CapturedTableForEdit, WorkflowError> {
    limits.validate()?;
    let scope = validated_scope(&[schema.to_owned()])?;
    if table.is_empty() || table.len() > 63 || table.contains('\0') {
        return Err(WorkflowError::Configuration(
            "table name must contain 1..63 UTF-8 bytes and no NUL",
        ));
    }
    let session = DedicatedSession(
        Session::connect_request(request, limits.driver())
            .await
            .map_err(|e| WorkflowError::Connection(Box::new(e)))?,
    );
    let work = async {
        let sql = match extent {
            CaptureExtent::Schema => open_capture_sql(&scope, limits)?,
            CaptureExtent::TargetTable => open_target_capture_sql(&scope, table, limits)?,
        };
        let report = execute_on_session(&session.0, &sql, limits).await?;
        if report.transaction_state != dbide_pg_spike::ServerTransactionState::Active {
            return Err(invalid(
                "catalog capture did not retain its read-only transaction",
            ));
        }
        let selected =
            captured_catalog_from_report_with_extent(request, &scope, limits, &report, extent)?
                .for_edit(&QualifiedName::new(schema, table))?;
        // The canonical query/hash is shared with PG execution guard code, never duplicated here.
        let metadata = execute_on_session(
            &session.0,
            &dbide_pg_spike::table_metadata_query(selected.relation_oid),
            limits,
        )
        .await?;
        if metadata.transaction_state != dbide_pg_spike::ServerTransactionState::Active {
            return Err(invalid(
                "canonical metadata query left its capture transaction",
            ));
        }
        let metadata_fingerprint = metadata_fingerprint_from_report(&metadata)?;
        let rollback = execute_on_session(&session.0, "ROLLBACK", limits).await?;
        if rollback.transaction_state != dbide_pg_spike::ServerTransactionState::Idle {
            return Err(invalid("catalog transaction rollback was not confirmed"));
        }
        Ok(CapturedTableForEdit {
            snapshot: selected.snapshot,
            relation_oid: selected.relation_oid,
            metadata_fingerprint,
        })
    };
    // One wall-clock budget covers catalog capture, canonical metadata and terminal rollback.
    tokio::time::timeout(Duration::from_secs(limits.timeout_secs), work)
        .await
        .map_err(|_| WorkflowError::Timeout)?
}
fn open_capture_sql(scope: &[String], limits: CaptureLimits) -> Result<String, WorkflowError> {
    capture_sql(scope, limits)
        .strip_suffix("ROLLBACK;\n")
        .map(str::to_owned)
        .ok_or(WorkflowError::Configuration(
            "catalog transaction terminator changed unexpectedly",
        ))
}
fn open_target_capture_sql(
    scope: &[String],
    table: &str,
    limits: CaptureLimits,
) -> Result<String, WorkflowError> {
    catalog_sql(scope, Some(table), limits)
        .strip_suffix("ROLLBACK;\n")
        .map(str::to_owned)
        .ok_or(WorkflowError::Configuration(
            "target catalog transaction terminator changed unexpectedly",
        ))
}
fn metadata_fingerprint_from_report(report: &ExecutionReport) -> Result<String, WorkflowError> {
    validate_report(report)?;
    let rows = report
        .events
        .iter()
        .filter_map(|e| {
            if let Event::Row { values } = e {
                Some(values)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if rows.len() != 1 || rows[0].len() != 1 {
        return Err(invalid(
            "canonical table metadata must be exactly one JSON cell",
        ));
    }
    let json = rows[0][0]
        .as_deref()
        .ok_or_else(|| invalid("canonical table metadata is missing or exceeds its bound"))?;
    if json.len() > dbide_pg_spike::MAX_TABLE_METADATA_BYTES {
        return Err(WorkflowError::Truncated);
    }
    if !serde_json::from_str::<serde_json::Value>(json)
        .map_err(|_| invalid("canonical table metadata is invalid JSON"))?
        .is_object()
    {
        return Err(invalid("canonical table metadata is not an object"));
    }
    Ok(dbide_pg_spike::table_metadata_fingerprint(json))
}
struct SelectedTableForEdit {
    snapshot: SchemaSnapshot,
    relation_oid: u32,
}
struct RelationIdentity {
    oid: u32,
    ordinary: bool,
}
struct CapturedCatalog {
    snapshot: SchemaSnapshot,
    relations: BTreeMap<QualifiedName, RelationIdentity>,
}
impl CapturedCatalog {
    fn for_edit(self, name: &QualifiedName) -> Result<SelectedTableForEdit, WorkflowError> {
        let identity = self
            .relations
            .get(name)
            .ok_or(WorkflowError::Configuration(
                "exact table was not found in the captured schema",
            ))?;
        if !identity.ordinary
            || identity.oid == 0
            || !self.snapshot.tables().any(|(n, _)| &n == name)
        {
            return Err(WorkflowError::Configuration(
                "table editing requires an ordinary nonpartitioned, noninherited PostgreSQL table",
            ));
        }
        Ok(SelectedTableForEdit {
            snapshot: self.snapshot,
            relation_oid: identity.oid,
        })
    }
}
/// Offline compare. Source is desired; target is actual. Nothing is executed.
pub fn compare_snapshots(
    source_desired: &SchemaSnapshot,
    target_actual: &SchemaSnapshot,
) -> Result<MigrationPlan, WorkflowError> {
    Ok(dbide_schema::diff(source_desired, target_actual)?)
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Record {
    Identity {
        database: String,
        version: String,
        version_num: u32,
    },
    Schema {
        name: String,
    },
    Relation {
        schema: String,
        name: String,
        relation_kind: String,
        relation_oid: u32,
        ordinary: bool,
        definition: Option<String>,
        features: Vec<String>,
    },
    Column {
        schema: String,
        table: String,
        column: dbide_schema::Column,
    },
    PrimaryKey {
        schema: String,
        table: String,
        key: PrimaryKey,
    },
    ForeignKey {
        schema: String,
        table: String,
        key: ForeignKey,
    },
    Index {
        schema: String,
        table: String,
        index: Index,
    },
    UnsupportedObject {
        schema: String,
        description: String,
    },
}
fn table_for(
    tables: &mut BTreeMap<QualifiedName, Table>,
    schema: String,
    table: String,
) -> Result<&mut Table, WorkflowError> {
    tables
        .get_mut(&QualifiedName::new(schema, table))
        .ok_or_else(|| {
            WorkflowError::InvalidCatalog(SchemaError(
                "metadata refers to an uncaptured relation".into(),
            ))
        })
}
fn invalid(message: &str) -> WorkflowError {
    WorkflowError::InvalidCatalog(SchemaError(message.into()))
}
fn snapshot_from_report(
    request: &ConnectionRequest,
    scope: &[String],
    limits: CaptureLimits,
    report: &ExecutionReport,
) -> Result<SchemaSnapshot, WorkflowError> {
    Ok(captured_catalog_from_report(request, scope, limits, report)?.snapshot)
}
fn captured_catalog_from_report(
    request: &ConnectionRequest,
    scope: &[String],
    limits: CaptureLimits,
    report: &ExecutionReport,
) -> Result<CapturedCatalog, WorkflowError> {
    captured_catalog_from_report_with_extent(request, scope, limits, report, CaptureExtent::Schema)
}
fn captured_catalog_from_report_with_extent(
    request: &ConnectionRequest,
    scope: &[String],
    limits: CaptureLimits,
    report: &ExecutionReport,
    extent: CaptureExtent,
) -> Result<CapturedCatalog, WorkflowError> {
    validate_report(report)?;
    let mut records = vec![];
    for event in &report.events {
        if let Event::Row { values } = event {
            if values.len() != 1 {
                return Err(invalid("catalog row must contain one JSON value"));
            }
            let value = values[0]
                .as_deref()
                .ok_or_else(|| invalid("catalog JSON value is NULL"))?;
            records.push(serde_json::from_str::<Record>(value)?);
            if records.len() > limits.max_records {
                return Err(WorkflowError::Truncated);
            }
        }
    }
    let mut identity = None;
    let mut schemas = BTreeMap::<String, Schema>::new();
    let mut tables = BTreeMap::<QualifiedName, Table>::new();
    let mut relation_identities = BTreeMap::new();
    let mut seen_oids = BTreeSet::new();
    let mut columns = 0;
    let mut relations = 0;
    for record in records {
        match record {
            Record::Identity {
                database,
                version,
                version_num,
            } => {
                if identity.is_some()
                    || database != request.profile.database
                    || version_num < 140000
                {
                    return Err(invalid(
                        "unexpected database identity or unsupported PostgreSQL version (requires 14+)",
                    ));
                }
                let stable = serde_json::to_vec(&(
                    &request.profile.id,
                    &request.profile.host,
                    request.profile.port,
                    &database,
                ))?;
                identity = Some(DatabaseIdentity {
                    engine: "postgresql".into(),
                    identity: format!("pg:{:x}", Sha256::digest(stable)),
                    name: database,
                    server_version: Some(version),
                });
            }
            Record::Schema { name } => {
                if !scope.contains(&name)
                    || schemas
                        .insert(
                            name.clone(),
                            Schema {
                                name,
                                tables: vec![],
                                views: vec![],
                                unsupported_objects: vec![],
                            },
                        )
                        .is_some()
                {
                    return Err(invalid("duplicate/out-of-scope schema record"));
                }
            }
            Record::Relation {
                schema,
                name,
                relation_kind,
                relation_oid,
                ordinary,
                definition,
                mut features,
            } => {
                let qname = QualifiedName::new(&schema, &name);
                if relation_oid == 0
                    || !seen_oids.insert(relation_oid)
                    || relation_identities
                        .insert(
                            qname,
                            RelationIdentity {
                                oid: relation_oid,
                                ordinary: ordinary && relation_kind == "r",
                            },
                        )
                        .is_some()
                {
                    return Err(invalid(
                        "duplicate/zero relation identity in catalog capture",
                    ));
                }
                relations += 1;
                if relations > limits.max_relations {
                    return Err(WorkflowError::Truncated);
                }
                let s = schemas
                    .get_mut(&schema)
                    .ok_or_else(|| invalid("relation schema missing"))?;
                if relation_kind == "v" || relation_kind == "m" {
                    s.views.push(View {
                        name: name.clone(),
                        definition: definition
                            .ok_or_else(|| invalid("view definition unavailable"))?,
                        materialized: relation_kind == "m",
                    });
                    s.unsupported_objects
                        .extend(features.into_iter().map(|f| format!("view {name}: {f}")));
                } else if matches!(relation_kind.as_str(), "r" | "p" | "f") {
                    if relation_kind != "r" {
                        features.push(format!("nonordinary relation kind: {relation_kind}"));
                    }
                    if tables
                        .insert(
                            QualifiedName::new(&schema, &name),
                            Table {
                                name,
                                columns: vec![],
                                primary_key: None,
                                foreign_keys: vec![],
                                indexes: vec![],
                                unsupported_features: features,
                            },
                        )
                        .is_some()
                    {
                        return Err(invalid("duplicate relation record"));
                    }
                } else {
                    return Err(invalid("unknown relation kind"));
                }
            }
            Record::Column {
                schema,
                table,
                mut column,
            } => {
                columns += 1;
                if columns > limits.max_columns {
                    return Err(WorkflowError::Truncated);
                }
                column.data_type = ddl::canonical_type(&column.data_type)?;
                column.default = column
                    .default
                    .as_deref()
                    .map(ddl::canonical_expression)
                    .transpose()?;
                let table = table_for(&mut tables, schema, table)?;
                if let Some(generation) = &column.generated {
                    let marker = if generation.starts_with("identity:") {
                        metadata_features::COLUMN_IDENTITY
                    } else {
                        metadata_features::COLUMN_GENERATED
                    };
                    table.unsupported_features.push(marker.into());
                }
                table.columns.push(column);
            }
            Record::PrimaryKey { schema, table, key } => {
                if table_for(&mut tables, schema, table)?
                    .primary_key
                    .replace(key)
                    .is_some()
                {
                    return Err(invalid("multiple primary key records"));
                }
            }
            Record::ForeignKey { schema, table, key } => table_for(&mut tables, schema, table)?
                .foreign_keys
                .push(key),
            Record::Index {
                schema,
                table,
                index,
            } => {
                if let Some(t) = tables.get_mut(&QualifiedName::new(&schema, &table)) {
                    if index.constraint_owned
                        && (!index.unsupported_features.is_empty()
                            || !index.include.is_empty()
                            || !index.expressions.is_empty()
                            || index.predicate.is_some()
                            || !index.method.eq_ignore_ascii_case("btree"))
                    {
                        t.unsupported_features.push(format!(
                            "constraint-owned index {} has unmodeled options",
                            index.name
                        ));
                    }
                    t.indexes.push(index);
                } else {
                    let s = schemas
                        .get_mut(&schema)
                        .ok_or_else(|| invalid("index schema missing"))?;
                    if s.views.iter().any(|v| v.name == table) {
                        s.unsupported_objects
                            .push(format!("view index {} on {table}", index.name));
                    } else {
                        return Err(invalid("index table missing"));
                    }
                }
            }
            Record::UnsupportedObject {
                schema,
                description,
            } => schemas
                .get_mut(&schema)
                .ok_or_else(|| invalid("unsupported object schema missing"))?
                .unsupported_objects
                .push(description),
        }
    }
    for (name, mut table) in tables {
        table.unsupported_features.sort();
        table.unsupported_features.dedup();
        schemas
            .get_mut(&name.schema)
            .ok_or_else(|| invalid("table schema missing"))?
            .tables
            .push(table);
    }
    let mut snapshot = SchemaSnapshot::empty(
        identity.ok_or_else(|| invalid("catalog identity missing"))?,
        Provenance::Live {
            captured_at: format!(
                "unix-micros:{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| invalid("system clock predates epoch"))?
                    .as_micros()
            ),
            generation: CAPTURES.fetch_add(1, Ordering::Relaxed),
        },
        scope.to_vec(),
    );
    snapshot.complete = extent == CaptureExtent::Schema;
    snapshot.schemas = schemas.into_values().collect();
    for s in &mut snapshot.schemas {
        s.unsupported_objects.sort();
        s.unsupported_objects.dedup();
    }
    snapshot.validate()?;
    Ok(CapturedCatalog {
        snapshot,
        relations: relation_identities,
    })
}

/// Parse exactly one permitted AST statement, then serialize the AST rather than appending raw SQL.
pub fn plan_only_sql(
    sql: &str,
    analyze: bool,
    limits: CaptureLimits,
) -> Result<String, WorkflowError> {
    limits.validate()?;
    if analyze {
        return Err(WorkflowError::Configuration(
            "EXPLAIN ANALYZE execution is unsupported; explicit execution confirmation workflow is required",
        ));
    }
    if sql.len() > 64_000 {
        return Err(WorkflowError::Configuration("EXPLAIN input exceeds 64 KB"));
    }
    let mut parsed = Parser::parse_sql(&PostgreSqlDialect {}, sql)
        .map_err(|_| WorkflowError::Configuration("EXPLAIN input must parse as one statement"))?;
    if parsed.len() != 1 {
        return Err(WorkflowError::Configuration(
            "EXPLAIN accepts exactly one statement",
        ));
    }
    let stmt = parsed.remove(0);
    if !matches!(
        stmt,
        Statement::Query(_)
            | Statement::Insert(_)
            | Statement::Update { .. }
            | Statement::Delete(_)
            | Statement::Merge { .. }
    ) {
        return Err(WorkflowError::Configuration(
            "EXPLAIN supports query/INSERT/UPDATE/DELETE/MERGE plans only",
        ));
    }
    Ok(format!(
        "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY; SET LOCAL statement_timeout = '{}ms'; SET LOCAL lock_timeout = '2000ms'; SET LOCAL standard_conforming_strings = on; SET LOCAL search_path = pg_catalog, public; EXPLAIN (FORMAT JSON, COSTS TRUE, VERBOSE TRUE) {}; ROLLBACK;",
        limits.timeout_secs * 1000,
        stmt
    ))
}
/// Dedicated-session plan-only EXPLAIN. `analyze=true` is rejected before any connection.
pub async fn explain_plan(
    request: &ConnectionRequest,
    sql: &str,
    analyze: bool,
    limits: CaptureLimits,
) -> Result<ExplainDocument, WorkflowError> {
    let sql = plan_only_sql(sql, analyze, limits)?;
    let report = execute(request, &sql, limits).await?;
    let values = report
        .events
        .iter()
        .filter_map(|e| {
            if let Event::Row { values } = e {
                Some(values)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if values.len() != 1 || values[0].len() != 1 {
        return Err(invalid("EXPLAIN returned unexpected result shape"));
    }
    let json = values[0][0]
        .as_deref()
        .ok_or_else(|| invalid("EXPLAIN returned NULL JSON"))?;
    Ok(dbide_schema::explain::parse_postgres_explain(
        json,
        ExecutionMode::PlanOnly,
    )?)
}

#[cfg(test)]
mod tests;
