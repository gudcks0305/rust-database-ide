//! Read PostgreSQL EXPLAIN (FORMAT JSON) output. No query generation/execution occurs here.
use crate::SchemaError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    PlanOnly,
    AnalyzeExecuted,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExplainDocument {
    pub mode: ExecutionMode,
    pub contains_actual_metrics: bool,
    /// Caller-provided mode is preserved; absence of metrics never proves ANALYZE was not run.
    pub warnings: Vec<String>,
    pub statements: Vec<ExplainStatement>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExplainStatement {
    pub plan: PlanNode,
    pub planning_time_ms: Option<f64>,
    pub execution_time_ms: Option<f64>,
    /// All fields, including triggers/settings/JIT/new PostgreSQL fields, are retained.
    pub raw: Map<String, Value>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanNode {
    pub node_type: String,
    pub relation_name: Option<String>,
    pub schema: Option<String>,
    pub alias: Option<String>,
    pub startup_cost: Option<f64>,
    pub total_cost: Option<f64>,
    pub plan_rows: Option<f64>,
    pub plan_width: Option<f64>,
    pub actual_startup_time_ms: Option<f64>,
    pub actual_total_time_ms: Option<f64>,
    pub actual_rows: Option<f64>,
    pub actual_loops: Option<f64>,
    pub children: Vec<PlanNode>,
    /// Raw node retains unknown properties. `Plans` remains here as well as typed children.
    pub raw: Map<String, Value>,
}
fn number(map: &Map<String, Value>, key: &str) -> Result<Option<f64>, SchemaError> {
    match map.get(key) {
        None => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|n| n.is_finite())
            .map(Some)
            .ok_or_else(|| SchemaError(format!("EXPLAIN {key} must be a finite number"))),
    }
}
fn string(map: &Map<String, Value>, key: &str) -> Result<Option<String>, SchemaError> {
    match map.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        _ => Err(SchemaError(format!("EXPLAIN {key} must be a string"))),
    }
}
fn node(value: &Value, depth: usize, count: &mut usize) -> Result<PlanNode, SchemaError> {
    *count += 1;
    if depth > 64 || *count > 10_000 {
        return Err(SchemaError(
            "EXPLAIN plan nesting/node limit exceeded".into(),
        ));
    }
    let raw = value
        .as_object()
        .ok_or_else(|| SchemaError("EXPLAIN Plan node must be an object".into()))?
        .clone();
    let children = match raw.get("Plans") {
        None => vec![],
        Some(Value::Array(v)) => v
            .iter()
            .map(|v| node(v, depth + 1, count))
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err(SchemaError("EXPLAIN Plans must be an array".into())),
    };
    Ok(PlanNode {
        node_type: string(&raw, "Node Type")?
            .ok_or_else(|| SchemaError("EXPLAIN node missing Node Type".into()))?,
        relation_name: string(&raw, "Relation Name")?,
        schema: string(&raw, "Schema")?,
        alias: string(&raw, "Alias")?,
        startup_cost: number(&raw, "Startup Cost")?,
        total_cost: number(&raw, "Total Cost")?,
        plan_rows: number(&raw, "Plan Rows")?,
        plan_width: number(&raw, "Plan Width")?,
        actual_startup_time_ms: number(&raw, "Actual Startup Time")?,
        actual_total_time_ms: number(&raw, "Actual Total Time")?,
        actual_rows: number(&raw, "Actual Rows")?,
        actual_loops: number(&raw, "Actual Loops")?,
        children,
        raw,
    })
}
fn actual(node: &PlanNode) -> bool {
    node.raw.keys().any(|k| k.starts_with("Actual ")) || node.children.iter().any(actual)
}
pub fn parse_postgres_explain(
    json: &str,
    mode: ExecutionMode,
) -> Result<ExplainDocument, SchemaError> {
    if json.len() > 16_000_000 {
        return Err(SchemaError("EXPLAIN JSON exceeds 16 MB input cap".into()));
    }
    let value: Value = serde_json::from_str(json)?;
    let rows = value
        .as_array()
        .ok_or_else(|| SchemaError("PostgreSQL EXPLAIN JSON must be an array".into()))?;
    if rows.is_empty() || rows.len() > 100 {
        return Err(SchemaError(
            "EXPLAIN requires 1..100 statement plans".into(),
        ));
    }
    let mut count = 0;
    let mut statements = vec![];
    for row in rows {
        let raw = row
            .as_object()
            .ok_or_else(|| SchemaError("EXPLAIN result must be object".into()))?
            .clone();
        let plan = node(
            raw.get("Plan")
                .ok_or_else(|| SchemaError("EXPLAIN result missing Plan".into()))?,
            0,
            &mut count,
        )?;
        statements.push(ExplainStatement {
            plan,
            planning_time_ms: number(&raw, "Planning Time")?,
            execution_time_ms: number(&raw, "Execution Time")?,
            raw,
        });
    }
    let contains_actual_metrics = statements
        .iter()
        .any(|s| actual(&s.plan) || s.execution_time_ms.is_some());
    let mut warnings = vec![];
    if mode == ExecutionMode::AnalyzeExecuted {
        warnings.push(
            "EXPLAIN ANALYZE executed the statement; writes and side effects may have occurred."
                .into(),
        );
    }
    if mode == ExecutionMode::Unknown {
        warnings.push(
            "Execution mode is unknown; JSON alone cannot establish whether ANALYZE ran.".into(),
        );
    }
    if mode == ExecutionMode::PlanOnly && contains_actual_metrics {
        warnings.push("Actual execution metrics conflict with caller's plan-only mode.".into());
    }
    Ok(ExplainDocument {
        mode,
        contains_actual_metrics,
        warnings,
        statements,
    })
}
