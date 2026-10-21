//! Verified PostgreSQL table editing. Arbitrary query results never enter this model.
use dbide_data::{
    Cell, Edit, EditColumn, IdentityKind, InsertValue, Limits, PgType, RowIdentity, TableSpec,
    TransactionPlan,
};
use dbide_profiles::ConnectionProfile;
use dbide_schema::{SchemaSnapshot, Table};
use std::collections::BTreeMap;

pub const PAGE_ROWS: usize = 100;
#[derive(Clone)]
pub struct VerifiedTable {
    pub schema: String,
    pub table: Table,
    pub spec: Option<TableSpec>,
    pub read_only: Option<String>,
}
fn pg_type(value: &str) -> Option<PgType> {
    let value = value.to_ascii_lowercase().replace("pg_catalog.", "");
    let without_typmod = if let Some(start) = value.find('(') {
        let end = value[start..].find(')')? + start;
        format!("{}{}", &value[..start], &value[end + 1..])
    } else {
        value
    };
    let base = without_typmod.trim();
    Some(match base {
        "text" | "character varying" | "varchar" => PgType::Text,
        "boolean" | "bool" => PgType::Boolean,
        "smallint" | "int2" => PgType::SmallInt,
        "integer" | "int" | "int4" => PgType::Integer,
        "bigint" | "int8" => PgType::BigInt,
        "numeric" | "decimal" => PgType::Numeric,
        "bytea" => PgType::Bytea,
        "uuid" => PgType::Uuid,
        "date" => PgType::Date,
        "timestamp" | "timestamp without time zone" => PgType::Timestamp,
        "timestamptz" | "timestamp with time zone" => PgType::TimestampTz,
        "json" => PgType::Json,
        "jsonb" => PgType::Jsonb,
        "double precision" | "float8" => PgType::DoublePrecision,
        _ => return None,
    })
}
impl VerifiedTable {
    pub fn capture(
        snapshot: &SchemaSnapshot,
        schema: &str,
        name: &str,
        profile: &ConnectionProfile,
    ) -> Result<Self, String> {
        if profile.engine != dbide_profiles::DatabaseEngine::PostgreSql
            || snapshot.database.engine != "postgresql"
        {
            return Err("This edit surface requires a verified PostgreSQL table".into());
        }
        if !snapshot.complete
            || !matches!(snapshot.provenance, dbide_schema::Provenance::Live { .. })
        {
            return Err("Editable rows require a complete live catalog capture".into());
        }
        if snapshot.database.name != profile.database {
            return Err("Catalog database identity does not match the captured profile".into());
        }
        let table = snapshot
            .schemas
            .iter()
            .find(|s| s.name == schema)
            .and_then(|s| s.tables.iter().find(|t| t.name == name))
            .cloned()
            .ok_or("Object is not an ordinary captured table")?;
        let mut result = Self {
            schema: schema.into(),
            table,
            spec: None,
            read_only: None,
        };
        let readonly = (|| -> Result<TableSpec, String> {
            if profile.read_only {
                return Err("Profile requests a read-only session; editing is disabled".into());
            }
            // Only explicitly understood metadata is allowed. Defaults/identity/checks
            // are server-evaluated inside the eventual checked transaction.
            use dbide_workflows::metadata_features as m;
            let safe = [
                m::COLUMN_IDENTITY,
                m::COLUMN_GENERATED,
                m::COLUMN_PRIVILEGES,
                m::COLUMN_COMMENT,
                m::CONSTRAINT_CHECK,
                m::CONSTRAINT_UNIQUE,
                m::CONSTRAINT_NOT_NULL,
                "noncurrent relation owner",
                "explicit relation privileges",
                "custom tablespace",
                "custom replica identity",
                "relation comment",
            ];
            for feature in &result.table.unsupported_features {
                if !safe.contains(&feature.as_str()) {
                    return Err(format!("Read-only metadata: {feature}"));
                }
            }
            for owner in snapshot.schemas.iter().flat_map(|s| &s.tables) {
                if owner.foreign_keys.iter().any(|fk| {
                    fk.references.schema == schema
                        && fk.references.name == name
                        && [fk.on_delete, fk.on_update].into_iter().any(|action| {
                            matches!(
                                action,
                                dbide_schema::ReferentialAction::Cascade
                                    | dbide_schema::ReferentialAction::SetNull
                                    | dbide_schema::ReferentialAction::SetDefault
                            )
                        })
                }) {
                    return Err(
                        "Incoming cascading foreign-key effects require a broader edit preview"
                            .into(),
                    );
                }
            }
            let columns = result
                .table
                .columns
                .iter()
                .map(|column| {
                    Ok(EditColumn {
                        name: column.name.clone(),
                        nullable: column.nullable,
                        pg_type: pg_type(&column.data_type).ok_or_else(|| {
                            format!(
                                "Unsupported editable PostgreSQL type: {} ({})",
                                column.name, column.data_type
                            )
                        })?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let identity = if let Some(pk) = &result.table.primary_key {
                Some((IdentityKind::PrimaryKey, pk.columns.clone()))
            } else {
                result
                    .table
                    .indexes
                    .iter()
                    .find(|i| {
                        i.constraint_owned
                            && i.unique
                            && i.method.eq_ignore_ascii_case("btree")
                            && i.predicate.is_none()
                            && i.expressions.is_empty()
                            && i.include.is_empty()
                            && i.unsupported_features.is_empty()
                            && !i.columns.is_empty()
                            && i.columns.iter().all(|name| {
                                result
                                    .table
                                    .columns
                                    .iter()
                                    .any(|c| c.name == *name && !c.nullable)
                            })
                    })
                    .map(|i| (IdentityKind::UniqueConstraint, i.columns.clone()))
            }
            .ok_or("No verified non-null primary/unique constraint; rows are read-only")?;
            let indices = identity
                .1
                .iter()
                .map(|name| {
                    columns
                        .iter()
                        .position(|c| c.name == *name)
                        .ok_or("Identity references an uncaptured column".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            let spec = TableSpec {
                schema: schema.into(),
                name: name.into(),
                columns,
                identity: Some(RowIdentity {
                    kind: identity.0,
                    columns: indices,
                }),
            };
            dbide_data::plan_edits(&spec, &[], edit_limits()).map_err(|e| e.to_string())?;
            Ok(spec)
        })();
        match readonly {
            Ok(spec) => result.spec = Some(spec),
            Err(reason) => result.read_only = Some(reason),
        }
        Ok(result)
    }
    pub fn select_sql(&self, page: usize) -> String {
        let cols = self
            .table
            .columns
            .iter()
            .map(|column| {
                let id = dbide_schema::quote_ident(&column.name);
                let expression = match pg_type(&column.data_type) {
                    Some(PgType::Bytea) => format!("pg_catalog.encode({id}, 'hex')"),
                    Some(PgType::DoublePrecision) => {
                        format!("pg_catalog.encode(pg_catalog.float8send({id}), 'hex')")
                    }
                    Some(PgType::Date) => temporal_projection(&id, &id, "YYYY-MM-DD"),
                    Some(PgType::Timestamp) => {
                        temporal_projection(&id, &id, "YYYY-MM-DD\"T\"HH24:MI:SS.US")
                    }
                    Some(PgType::TimestampTz) => temporal_projection(
                        &id,
                        &format!("{id} AT TIME ZONE 'UTC'"),
                        "YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"",
                    ),
                    _ => id.clone(),
                };
                format!("{expression} AS {id}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let order = self
            .spec
            .as_ref()
            .and_then(|s| {
                s.identity.as_ref().map(|i| {
                    i.columns
                        .iter()
                        .map(|&ix| dbide_schema::quote_ident(&s.columns[ix].name))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
            })
            .or_else(|| {
                self.table
                    .primary_key
                    .as_ref()
                    .map(|p| dbide_schema::quote_columns(&p.columns))
            });
        format!(
            "SELECT {cols} FROM {}.{}{} LIMIT {PAGE_ROWS} OFFSET {};",
            dbide_schema::quote_ident(&self.schema),
            dbide_schema::quote_ident(&self.table.name),
            order.map(|o| format!(" ORDER BY {o}")).unwrap_or_default(),
            page.saturating_mul(PAGE_ROWS)
        )
    }
    pub fn decode_rows(
        &self,
        columns: &[String],
        rows: &[Vec<Option<String>>],
        truncated: bool,
    ) -> Result<Vec<Vec<Cell>>, String> {
        if truncated {
            return Err(
                "Driver result was truncated; editing requires a complete requested page".into(),
            );
        }
        if columns
            != self
                .table
                .columns
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>()
        {
            return Err("Projection differs from verified catalog column order".into());
        }
        let parsed = rows
            .iter()
            .map(|row| {
                if row.len() != self.table.columns.len() {
                    return Err("Incomplete row identity/projection".into());
                }
                row.iter()
                    .zip(&self.table.columns)
                    .map(|(value, column)| match value {
                        None => Ok(Cell::Null),
                        Some(value) => match pg_type(&column.data_type) {
                            Some(PgType::Bytea) => Ok(Cell::Binary(hex(value)?)),
                            Some(PgType::DoublePrecision) => {
                                let bytes: [u8; 8] = hex(value)?
                                    .try_into()
                                    .map_err(|_| "Invalid float8 wire projection")?;
                                let number = f64::from_be_bytes(bytes);
                                Ok(Cell::Text(if number == f64::INFINITY {
                                    "Infinity".into()
                                } else if number == f64::NEG_INFINITY {
                                    "-Infinity".into()
                                } else {
                                    number.to_string()
                                }))
                            }
                            _ => Ok(Cell::Text(value.clone())),
                        },
                    })
                    .collect()
            })
            .collect::<Result<Vec<Vec<Cell>>, String>>()?;
        if let Some(spec) = &self.spec {
            let unchanged = parsed
                .iter()
                .map(|row| Edit::Update {
                    original: row.clone(),
                    replacement: row.clone(),
                })
                .collect::<Vec<_>>();
            dbide_data::plan_edits(
                spec,
                &unchanged,
                Limits {
                    max_rows: PAGE_ROWS,
                    max_bytes: 8 * 1024 * 1024,
                },
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(parsed)
    }
    pub fn cell_from_input(&self, index: usize, text: &str, null: bool) -> Result<Cell, String> {
        if null {
            return Ok(Cell::Null);
        }
        if text.contains('\0') {
            return Err(
                "PostgreSQL text values cannot contain NUL; use a bytea hex input for binary data"
                    .into(),
            );
        }
        match self
            .table
            .columns
            .get(index)
            .and_then(|c| pg_type(&c.data_type))
        {
            Some(PgType::Bytea) => Ok(Cell::Binary(hex(text.strip_prefix("\\x").unwrap_or(text))?)),
            _ => Ok(Cell::Text(text.into())),
        }
    }
}
fn temporal_projection(original: &str, value: &str, format: &str) -> String {
    format!(
        "CASE WHEN {original} IS NULL THEN NULL WHEN NOT pg_catalog.isfinite({original}) OR pg_catalog.to_char({value}, 'BC') = 'BC' THEN {original}::pg_catalog.text ELSE pg_catalog.to_char({value}, '{format}') END"
    )
}
#[derive(Clone)]
pub struct RowContext {
    pub profile: ConnectionProfile,
    pub connection_revision: u64,
    pub row_execution: Option<dbide_pg_spike::ExecutionKey>,
}
impl RowContext {
    pub fn matches(&self, current: &Self, require_execution: bool) -> bool {
        self.profile == current.profile
            && self.profile.engine == dbide_profiles::DatabaseEngine::PostgreSql
            && self.connection_revision == current.connection_revision
            && (!require_execution
                || (!self.profile.read_only
                    && self.row_execution.is_some()
                    && self.row_execution == current.row_execution))
    }
}
pub fn cell_text(cell: &Cell) -> String {
    match cell {
        Cell::Null => String::new(),
        Cell::Text(s) | Cell::Decimal(s) => s.clone(),
        Cell::Binary(bytes) => format!(
            "\\x{}",
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
        ),
        Cell::Bool(v) => v.to_string(),
        Cell::Integer(v) => v.to_string(),
    }
}
fn hex(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) || value.len() > 8 * 1024 * 1024 {
        return Err("Invalid/oversized binary hex value".into());
    }
    value
        .as_bytes()
        .chunks(2)
        .map(|pair| {
            let digit = |x: u8| match x {
                b'0'..=b'9' => Some(x - b'0'),
                b'a'..=b'f' => Some(x - b'a' + 10),
                b'A'..=b'F' => Some(x - b'A' + 10),
                _ => None,
            };
            Ok(digit(pair[0]).ok_or("Invalid hex")? * 16 + digit(pair[1]).ok_or("Invalid hex")?)
        })
        .collect()
}
pub fn edit_limits() -> Limits {
    Limits {
        max_rows: 100,
        max_bytes: 4 * 1024 * 1024,
    }
}
#[derive(Default)]
pub struct Staging {
    pub rows: BTreeMap<usize, Edit>,
    pub inserts: Vec<Edit>,
    pub revision: u64,
    pub preview: Option<(u64, TransactionPlan)>,
}
impl Staging {
    pub fn edits(&self) -> Vec<Edit> {
        self.rows
            .values()
            .chain(self.inserts.iter())
            .cloned()
            .collect()
    }
    pub fn changed(&mut self) {
        self.revision = self.revision.saturating_add(1);
        self.preview = None;
    }
    pub fn stage_row(&mut self, index: usize, edit: Edit, spec: &TableSpec) -> Result<(), String> {
        if matches!(&edit,Edit::Update{original,replacement}if original==replacement) {
            self.rows.remove(&index);
            self.changed();
            return Ok(());
        }
        let previous = self.rows.insert(index, edit);
        let plan = dbide_data::plan_edits(spec, &self.edits(), edit_limits());
        if let Err(error) = plan {
            if let Some(previous) = previous {
                self.rows.insert(index, previous);
            } else {
                self.rows.remove(&index);
            }
            return Err(error.to_string());
        }
        self.changed();
        Ok(())
    }
    pub fn stage_insert(
        &mut self,
        values: Vec<InsertValue>,
        spec: &TableSpec,
    ) -> Result<(), String> {
        self.inserts.push(Edit::InsertDefaulted { values });
        if let Err(error) = dbide_data::plan_edits(spec, &self.edits(), edit_limits()) {
            self.inserts.pop();
            return Err(error.to_string());
        }
        self.changed();
        Ok(())
    }
    pub fn plan(&mut self, spec: &TableSpec) -> Result<(), String> {
        let plan = dbide_data::plan_edits(spec, &self.edits(), edit_limits())
            .map_err(|e| e.to_string())?;
        if plan.statements.is_empty() {
            return Err("No changed row values to submit".into());
        }
        self.preview = Some((self.revision, plan));
        Ok(())
    }
    pub fn ready_plan(&self) -> Result<&TransactionPlan, String> {
        self.preview
            .as_ref()
            .filter(|(revision, _)| *revision == self.revision)
            .map(|(_, p)| p)
            .ok_or("Generate a current transaction preview first".into())
    }
    pub fn count(&self) -> usize {
        self.rows.len() + self.inserts.len()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use dbide_schema::*;
    fn fixture() -> (SchemaSnapshot, ConnectionProfile) {
        let profile = ConnectionProfile::fixture();
        let table = Table {
            name: "items".into(),
            columns: vec![
                Column {
                    name: "id".into(),
                    data_type: "BIGINT".into(),
                    nullable: false,
                    default: None,
                    generated: Some("identity:d".into()),
                },
                Column {
                    name: "name".into(),
                    data_type: "TEXT".into(),
                    nullable: false,
                    default: None,
                    generated: None,
                },
                Column {
                    name: "notes".into(),
                    data_type: "TEXT".into(),
                    nullable: true,
                    default: None,
                    generated: None,
                },
                Column {
                    name: "payload".into(),
                    data_type: "JSON".into(),
                    nullable: true,
                    default: None,
                    generated: None,
                },
            ],
            primary_key: Some(PrimaryKey {
                name: Some("items_pkey".into()),
                columns: vec!["id".into()],
            }),
            foreign_keys: vec![],
            indexes: vec![],
            unsupported_features: vec![dbide_workflows::metadata_features::COLUMN_IDENTITY.into()],
        };
        let mut snapshot = SchemaSnapshot::empty(
            DatabaseIdentity {
                engine: "postgresql".into(),
                identity: "owned-fixture".into(),
                name: profile.database.clone(),
                server_version: None,
            },
            Provenance::Live {
                captured_at: "test".into(),
                generation: 1,
            },
            vec!["public".into()],
        );
        snapshot.schemas.push(Schema {
            name: "public".into(),
            tables: vec![table],
            views: vec![],
            unsupported_objects: vec![],
        });
        (snapshot, profile)
    }
    #[test]
    fn incoming_update_cascade_is_read_only_even_with_no_action_delete() {
        let (mut snapshot, profile) = fixture();
        let mut child = snapshot.schemas[0].tables[0].clone();
        child.name = "children".into();
        child.foreign_keys.push(ForeignKey {
            name: None,
            columns: vec!["id".into()],
            references: QualifiedName::new("public", "items"),
            referenced_columns: vec!["id".into()],
            on_delete: ReferentialAction::NoAction,
            on_update: ReferentialAction::Cascade,
            deferrable: false,
            initially_deferred: false,
        });
        snapshot.schemas[0].tables.push(child);
        let model = VerifiedTable::capture(&snapshot, "public", "items", &profile).unwrap();
        assert!(model.spec.is_none());
        assert!(model.read_only.unwrap().contains("cascading"));
    }
    #[test]
    fn complete_identity_and_collation_are_required_but_generated_defaults_supported() {
        let (mut snapshot, profile) = fixture();
        assert!(
            VerifiedTable::capture(&snapshot, "public", "items", &profile)
                .unwrap()
                .spec
                .is_some()
        );
        snapshot.complete = false;
        assert!(VerifiedTable::capture(&snapshot, "public", "items", &profile).is_err());
        snapshot.complete = true;
        snapshot.schemas[0].tables[0]
            .unsupported_features
            .push(dbide_workflows::metadata_features::COLUMN_NONDETERMINISTIC_COLLATION.into());
        assert!(
            VerifiedTable::capture(&snapshot, "public", "items", &profile)
                .unwrap()
                .spec
                .is_none()
        );
        snapshot.schemas[0].tables[0].unsupported_features.clear();
        snapshot.schemas[0].tables[0].primary_key = None;
        assert!(
            VerifiedTable::capture(&snapshot, "public", "items", &profile)
                .unwrap()
                .spec
                .is_none()
        );
    }
    #[test]
    fn unchanged_multiline_text_json_remain_exact_and_explicit_updates_are_bound() {
        let (snapshot, profile) = fixture();
        let model = VerifiedTable::capture(&snapshot, "public", "items", &profile).unwrap();
        let spec = model.spec.as_ref().unwrap();
        let notes = "line1\r\nline2\n한글";
        let json = "{\n  \"x\": \"한글\"\r\n}";
        let original = vec![
            Cell::Text("1".into()),
            Cell::Text("old".into()),
            Cell::Text(notes.into()),
            Cell::Text(json.into()),
        ];
        let mut replacement = original.clone();
        replacement[1] = model.cell_from_input(1, "new", false).unwrap();
        let mut staged = Staging::default();
        staged
            .stage_row(
                0,
                Edit::Update {
                    original: original.clone(),
                    replacement,
                },
                spec,
            )
            .unwrap();
        staged.plan(spec).unwrap();
        let plan = staged.ready_plan().unwrap();
        let assignments = plan.statements[0]
            .sql
            .split(" SET ")
            .nth(1)
            .unwrap()
            .split(" WHERE ")
            .next()
            .unwrap();
        assert!(!assignments.contains(r#""notes""#));
        assert!(!assignments.contains(r#""payload""#));
        assert!(
            plan.statements[0]
                .parameters
                .iter()
                .any(|p| p.value.as_deref() == Some(notes))
        );
        assert!(
            plan.statements[0]
                .parameters
                .iter()
                .any(|p| p.value.as_deref() == Some(json))
        );
        let new_notes = "changed\r\nstill multiline";
        let mut replacement = original.clone();
        replacement[2] = model.cell_from_input(2, new_notes, false).unwrap();
        staged
            .stage_row(
                0,
                Edit::Update {
                    original,
                    replacement,
                },
                spec,
            )
            .unwrap();
        assert!(staged.ready_plan().is_err());
        staged.plan(spec).unwrap();
        assert!(
            staged.ready_plan().unwrap().statements[0]
                .parameters
                .iter()
                .any(|p| p.value.as_deref() == Some(new_notes))
        );
    }
    #[test]
    fn default_insert_has_no_interpolated_values_and_failed_stage_preserves_buffer() {
        let (snapshot, profile) = fixture();
        let model = VerifiedTable::capture(&snapshot, "public", "items", &profile).unwrap();
        let spec = model.spec.as_ref().unwrap();
        let mut staging = Staging::default();
        let name = "name'); DROP TABLE items; --";
        staging
            .stage_insert(
                vec![
                    InsertValue::Default,
                    InsertValue::Value(Cell::Text(name.into())),
                    InsertValue::Value(Cell::Null),
                    InsertValue::Value(Cell::Null),
                ],
                spec,
            )
            .unwrap();
        staging.plan(spec).unwrap();
        let plan = staging.ready_plan().unwrap();
        assert!(plan.statements[0].sql.contains("DEFAULT"));
        assert!(!plan.statements[0].sql.contains(name));
        assert_eq!(
            plan.statements[0].parameters[0].value.as_deref(),
            Some(name)
        );
        assert!(
            staging
                .stage_insert(vec![InsertValue::Value(Cell::Null); 4], spec)
                .is_err()
        );
        assert_eq!(staging.count(), 1);
    }
    #[test]
    fn temporal_projection_keeps_infinity_and_era_and_precision_maps_correctly() {
        assert_eq!(
            pg_type("timestamp(6) with time zone"),
            Some(PgType::TimestampTz)
        );
        let query = temporal_projection(r#""when""#, r#""when""#, "YYYY-MM-DD");
        assert!(query.contains("IS NULL THEN NULL"));
        assert!(query.contains("NOT pg_catalog.isfinite"));
        assert!(query.contains(r#"'BC') = 'BC' THEN "when"::pg_catalog.text"#));
    }
    #[test]
    fn truncated_projection_and_stale_physical_execution_are_rejected() {
        let (snapshot, profile) = fixture();
        let model = VerifiedTable::capture(&snapshot, "public", "items", &profile).unwrap();
        let names = model
            .table
            .columns
            .iter()
            .map(|c| c.name.clone())
            .collect::<Vec<_>>();
        assert!(model.decode_rows(&names, &[], true).is_err());
        assert!(model.decode_rows(&["wrong".into()], &[], false).is_err());
        let original = RowContext {
            profile: profile.clone(),
            connection_revision: 2,
            row_execution: Some(dbide_pg_spike::ExecutionKey {
                session: 1,
                generation: 1,
                execution: 7,
            }),
        };
        let mut current = original.clone();
        assert!(original.matches(&current, true));
        current.connection_revision = 3;
        assert!(!original.matches(&current, true));
        current = original.clone();
        current.row_execution.as_mut().unwrap().execution = 8;
        assert!(!original.matches(&current, true));
        current = original.clone();
        current.profile.host = "other".into();
        assert!(!original.matches(&current, true));
    }
}
