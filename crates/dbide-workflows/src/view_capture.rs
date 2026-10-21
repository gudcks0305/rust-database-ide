//! Exact-object view definition reads. No whole-schema completeness claim or driver changes.
use crate::{
    CaptureLimits, WorkflowError, execute, invalid, sql_literal, validate_report, validated_scope,
};
use dbide_pg_spike::{Event, ExecutionReport, ServerTransactionState};
use dbide_profiles::{ConnectionRequest, DatabaseEngine};
use dbide_schema::View;
use serde::Deserialize;

/// Definition metadata for one exact view/materialized view. This is partial object scope,
/// deliberately not a SchemaSnapshot and never evidence of whole-schema completeness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedView {
    pub schema: String,
    pub view: View,
}

fn metadata_request(
    request: &ConnectionRequest,
    limits: CaptureLimits,
) -> Result<(ConnectionRequest, CaptureLimits), WorkflowError> {
    limits.validate()?;
    if request.profile.engine != DatabaseEngine::PostgreSql {
        return Err(WorkflowError::Configuration(
            "targeted view definitions require PostgreSQL",
        ));
    }
    request
        .profile
        .validate()
        .map_err(|_| WorkflowError::Configuration("view metadata connection profile is invalid"))?;
    let mut probe = request.clone();
    probe.profile.connect_timeout_secs = probe.profile.connect_timeout_secs.min(4);
    let limits = CaptureLimits {
        timeout_secs: limits.timeout_secs.min(4),
        ..limits
    };
    Ok((probe, limits))
}

fn query_sql(schema: &str, name: &str, limits: CaptureLimits) -> Result<String, WorkflowError> {
    limits.validate()?;
    validated_scope(&[schema.to_owned()])?;
    if name.is_empty() || name.len() > 63 || name.contains('\0') {
        return Err(WorkflowError::Configuration(
            "view name must contain 1..63 UTF-8 bytes and no NUL",
        ));
    }
    // Session currently exposes simple-query SQL, not a parameterized SELECT API. Names are
    // data values encoded by the existing E-literal encoder; never interpolate identifiers.
    // format! substitutes each value once, including names containing template-like text.
    Ok(format!(
        "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL statement_timeout = '{timeout}ms';
SET LOCAL lock_timeout = '2000ms';
SET LOCAL standard_conforming_strings = on;
SET LOCAL search_path = pg_catalog;
WITH exact_view AS MATERIALIZED (
 SELECT n.nspname,c.relname,c.relkind,c.oid
 FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
 WHERE n.nspname={schema} AND c.relname={name} AND c.relkind IN ('v','m') LIMIT 2
), definition AS MATERIALIZED (
 SELECT nspname,relname,relkind,pg_catalog.pg_get_viewdef(oid,true) AS source FROM exact_view
), encoded AS (
 SELECT pg_catalog.jsonb_build_object('schema',nspname,'name',relname,'relation_kind',relkind::text,'definition',source)::text AS payload FROM definition
)
SELECT CASE WHEN pg_catalog.octet_length(payload)<={max_bytes} THEN payload ELSE NULL END AS view_record FROM encoded LIMIT 2;
ROLLBACK;",
        timeout = limits.timeout_secs * 1000,
        schema = sql_literal(schema),
        name = sql_literal(name),
        max_bytes = limits.max_bytes,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewRecord {
    schema: String,
    name: String,
    relation_kind: String,
    definition: String,
}
fn decode(
    report: &ExecutionReport,
    schema: &str,
    name: &str,
    limits: CaptureLimits,
) -> Result<CapturedView, WorkflowError> {
    validate_report(report)?;
    if report.transaction_state != ServerTransactionState::Idle {
        return Err(invalid(
            "view metadata transaction rollback was not confirmed",
        ));
    }
    let rows = report
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Row { values } => Some(values),
            _ => None,
        })
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return Err(WorkflowError::Configuration(
            "exact object is missing or is not a view/materialized view; refresh the navigator",
        ));
    }
    if rows.len() != 1 || rows[0].len() != 1 {
        return Err(invalid("view metadata must contain one JSON cell"));
    }
    let value = rows[0][0].as_deref().ok_or(WorkflowError::Truncated)?;
    if value.len() > limits.max_bytes || report.retained_bytes > limits.max_bytes {
        return Err(WorkflowError::Truncated);
    }
    let record: ViewRecord = serde_json::from_str(value)?;
    if record.schema != schema || record.name != name {
        return Err(invalid(
            "view metadata identity differs from requested object",
        ));
    }
    let materialized = match record.relation_kind.as_str() {
        "v" => false,
        "m" => true,
        _ => return Err(invalid("captured object is not an actual PostgreSQL view")),
    };
    Ok(CapturedView {
        schema: record.schema,
        view: View {
            name: record.name,
            definition: record.definition,
            materialized,
        },
    })
}

/// Reads only one exact view/materialized definition; no unrelated relation/column scan.
/// The caller profile is preserved. A private copy caps connection timeout at four seconds;
/// a separate private limits copy caps query/server timeout at four seconds. Existing shorter
/// limits remain shorter. Dedicated READ ONLY session closes on success/error/cancellation.
pub async fn capture_view(
    request: &ConnectionRequest,
    schema: &str,
    name: &str,
    limits: CaptureLimits,
) -> Result<CapturedView, WorkflowError> {
    let (probe, limits) = metadata_request(request, limits)?;
    let sql = query_sql(schema, name, limits)?;
    let report = execute(&probe, &sql, limits).await?;
    decode(&report, schema, name, limits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbide_pg_spike::{CommitPolicy, ExecutionKey, Outcome};
    use dbide_profiles::ConnectionProfile;
    use serde_json::json;
    use sqlparser::{ast::Statement, dialect::PostgreSqlDialect, parser::Parser};

    fn report(kind: &str) -> ExecutionReport {
        ExecutionReport {
            key: ExecutionKey { session: 1, generation: 1, execution: 1 },
            commit_policy: CommitPolicy::Auto,
            transaction_state: ServerTransactionState::Idle,
            outcome: Outcome::Completed,
            terminal_error: None,
            events: vec![Event::Row { values: vec![Some(json!({"schema":"s","name":"v","relation_kind":kind,"definition":"SELECT '한글'::text AS value;"}).to_string())] }],
            notices: vec![],retained_bytes:300,truncated:false,connection_retired:false,elapsed_micros:1,
        }
    }
    #[test]
    fn exact_query_escapes_data_and_never_scans_unrelated_column_catalogs() {
        let schema = "s__LIMIT__'\\;--";
        let name = "한글'\\;DROP TABLE x;--";
        let sql = query_sql(
            schema,
            name,
            CaptureLimits {
                max_relations: 1,
                max_records: 1,
                max_columns: 1,
                ..CaptureLimits::default()
            },
        )
        .unwrap();
        let statements = Parser::parse_sql(&PostgreSqlDialect {}, &sql).unwrap();
        assert_eq!(statements.len(), 7);
        assert_eq!(
            statements
                .iter()
                .filter(|s| matches!(s, Statement::Query(_)))
                .count(),
            1
        );
        assert!(
            !statements
                .iter()
                .any(|s| matches!(s, Statement::Drop { .. }))
        );
        assert!(sql.contains(&format!("n.nspname={}", sql_literal(schema))));
        assert!(sql.contains(&format!("c.relname={}", sql_literal(name))));
        assert!(sql.contains("c.relkind IN ('v','m')"));
        for unrelated in [
            "pg_attribute",
            "pg_constraint",
            "pg_index",
            "unsupported_object",
        ] {
            assert!(!sql.contains(unrelated));
        }
        for (schema, name) in [("s", ""), ("s", "bad\0name"), ("pg_catalog", "v")] {
            assert!(query_sql(schema, name, CaptureLimits::default()).is_err());
        }
        assert!(query_sql("s", &"x".repeat(64), CaptureLimits::default()).is_err());
        assert!(query_sql("bad\0schema", "v", CaptureLimits::default()).is_err());
        assert!(query_sql(&"x".repeat(64), "v", CaptureLimits::default()).is_err());
        assert!(query_sql("s", &"가".repeat(22), CaptureLimits::default()).is_err());
        assert!(
            query_sql(
                "s",
                "v",
                CaptureLimits {
                    max_bytes: 0,
                    ..CaptureLimits::default()
                }
            )
            .is_err()
        );
        assert!(
            query_sql(
                "s",
                "v",
                CaptureLimits {
                    timeout_secs: 0,
                    ..CaptureLimits::default()
                }
            )
            .is_err()
        );
    }
    #[test]
    fn metadata_deadlines_cap_private_copies_and_preserve_caller_profile() {
        let request = ConnectionRequest {
            profile: ConnectionProfile {
                connect_timeout_secs: 300,
                ..ConnectionProfile::fixture()
            },
            password: "private-test-password".into(),
        };
        let before = request.profile.clone();
        let limits = CaptureLimits {
            timeout_secs: 120,
            ..CaptureLimits::default()
        };
        let (probe, bounded) = metadata_request(&request, limits).unwrap();
        let mut expected = before.clone();
        expected.connect_timeout_secs = 4;
        assert_eq!(probe.profile, expected);
        assert_eq!(probe.password, request.password);
        assert_eq!(request.profile, before);
        assert_eq!(bounded.timeout_secs, 4);
        assert_eq!(limits.timeout_secs, 120);
        let mut shorter = request.clone();
        shorter.profile.connect_timeout_secs = 1;
        let (probe, bounded) = metadata_request(
            &shorter,
            CaptureLimits {
                timeout_secs: 1,
                ..limits
            },
        )
        .unwrap();
        assert_eq!(probe.profile.connect_timeout_secs, 1);
        assert_eq!(bounded.timeout_secs, 1);
        let mut other = request;
        other.profile.engine = DatabaseEngine::Sqlite;
        assert!(metadata_request(&other, limits).is_err());
    }
    #[test]
    fn typed_view_kind_and_partial_object_contract_reject_bad_identity_or_incomplete_reports() {
        let limits = CaptureLimits::default();
        let plain = decode(&report("v"), "s", "v", limits).unwrap();
        assert!(!plain.view.materialized);
        assert_eq!(plain.schema, "s");
        assert!(plain.view.definition.contains("한글"));
        assert!(
            decode(&report("m"), "s", "v", limits)
                .unwrap()
                .view
                .materialized
        );
        assert!(decode(&report("r"), "s", "v", limits).is_err());
        assert!(decode(&report("v"), "different", "v", limits).is_err());
        let mut r = report("v");
        r.truncated = true;
        assert!(matches!(
            decode(&r, "s", "v", limits),
            Err(WorkflowError::Truncated)
        ));
        r = report("v");
        r.events = vec![];
        assert!(decode(&r, "s", "v", limits).is_err());
        r = report("v");
        r.events = vec![Event::Row { values: vec![None] }];
        assert!(matches!(
            decode(&r, "s", "v", limits),
            Err(WorkflowError::Truncated)
        ));
        r = report("v");
        r.events.push(r.events[0].clone());
        assert!(decode(&r, "s", "v", limits).is_err());
        r = report("v");
        r.transaction_state = ServerTransactionState::Active;
        assert!(decode(&r, "s", "v", limits).is_err());
        r = report("v");
        r.events = vec![Event::Row {
            values: vec![Some(
                json!({"schema":"s","name":"v","relation_kind":"v","definition":"x".repeat(1024)})
                    .to_string(),
            )],
        }];
        assert!(matches!(
            decode(
                &r,
                "s",
                "v",
                CaptureLimits {
                    max_bytes: 1024,
                    ..limits
                }
            ),
            Err(WorkflowError::Truncated)
        ));
        r = report("v");
        r.retained_bytes = limits.max_bytes + 1;
        assert!(matches!(
            decode(&r, "s", "v", limits),
            Err(WorkflowError::Truncated)
        ));
    }
}
