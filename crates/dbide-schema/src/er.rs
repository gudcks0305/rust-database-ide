//! Deterministic bounded ER graph and standalone SVG. FK lines are metadata, never inferred.
use crate::{SchemaError, model::*};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErNode {
    pub id: String,
    pub name: QualifiedName,
    pub columns: Vec<ErColumn>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErColumn {
    pub id: String,
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErEdge {
    pub id: String,
    pub from: String,
    pub to: String,
    pub column_pairs: Vec<(String, String)>,
    pub constraint: Option<String>,
    pub target_in_scope: bool,
    pub column_endpoints_visible: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErGraph {
    pub nodes: Vec<ErNode>,
    pub edges: Vec<ErEdge>,
    pub truncated: bool,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, Copy)]
pub struct ErLimits {
    pub max_tables: usize,
    pub max_columns_per_table: usize,
    pub max_edges: usize,
    pub max_svg_bytes: usize,
}
impl Default for ErLimits {
    fn default() -> Self {
        Self {
            max_tables: 200,
            max_columns_per_table: 40,
            max_edges: 1000,
            max_svg_bytes: 2_000_000,
        }
    }
}
fn node_id(name: &QualifiedName) -> String {
    // XML IDs contain only ASCII hex and begin with a letter.
    format!(
        "t{}",
        name.id()
            .bytes()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}
fn column_id(node: &str, name: &str) -> String {
    format!(
        "{node}c{}",
        name.bytes().map(|b| format!("{b:02x}")).collect::<String>()
    )
}
pub fn graph(snapshot: &SchemaSnapshot, limits: ErLimits) -> Result<ErGraph, SchemaError> {
    snapshot.validate()?;
    if limits.max_tables > 2000
        || limits.max_columns_per_table > 500
        || limits.max_edges > 10_000
        || limits.max_svg_bytes > 20_000_000
    {
        return Err(SchemaError("ER limits exceed safety caps".into()));
    }
    let mut tables = snapshot.tables().collect::<Vec<_>>();
    tables.sort_by(|(a, _), (b, _)| a.cmp(b));
    let mut truncated = tables.len() > limits.max_tables;
    let mut nodes = vec![];
    for (name, t) in tables.iter().take(limits.max_tables) {
        let id = node_id(name);
        truncated |= t.columns.len() > limits.max_columns_per_table;
        nodes.push(ErNode {
            id: id.clone(),
            name: name.clone(),
            columns: t
                .columns
                .iter()
                .take(limits.max_columns_per_table)
                .map(|c| ErColumn {
                    id: column_id(&id, &c.name),
                    name: c.name.clone(),
                    data_type: c.data_type.clone(),
                    nullable: c.nullable,
                    primary_key: t
                        .primary_key
                        .as_ref()
                        .is_some_and(|p| p.columns.contains(&c.name)),
                })
                .collect(),
        });
    }
    let ids = nodes
        .iter()
        .map(|n| n.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let column_ids = nodes
        .iter()
        .flat_map(|n| n.columns.iter().map(|c| c.id.as_str()))
        .collect::<std::collections::BTreeSet<_>>();
    let mut edges = vec![];
    for (name, t) in tables.iter().take(limits.max_tables) {
        let from = node_id(name);
        let mut fks = t.foreign_keys.iter().collect::<Vec<_>>();
        fks.sort_by_key(|f| serde_json::to_string(f).unwrap_or_default());
        for (ordinal, fk) in fks.into_iter().enumerate() {
            if edges.len() == limits.max_edges {
                truncated = true;
                continue;
            }
            let to = node_id(&fk.references);
            edges.push(ErEdge {
                id: format!("{from}f{ordinal}"),
                from: from.clone(),
                to: to.clone(),
                column_pairs: fk
                    .columns
                    .iter()
                    .zip(&fk.referenced_columns)
                    .map(|(a, b)| (column_id(&from, a), column_id(&to, b)))
                    .collect(),
                constraint: fk.name.clone(),
                target_in_scope: ids.contains(to.as_str()),
                column_endpoints_visible: fk.columns.iter().zip(&fk.referenced_columns).all(
                    |(a, b)| {
                        column_ids.contains(column_id(&from, a).as_str())
                            && column_ids.contains(column_id(&to, b).as_str())
                    },
                ),
            });
        }
    }
    let mut warnings = vec![];
    if truncated {
        warnings.push("ER output is truncated by configured table/column/edge limits".into());
    }
    if edges.iter().any(|e| !e.target_in_scope) {
        warnings.push("Some FK targets are outside the displayed graph".into());
    }
    if edges
        .iter()
        .any(|e| e.target_in_scope && !e.column_endpoints_visible)
    {
        warnings.push("Some FK column endpoints are hidden by column limits".into());
    }
    if !snapshot.complete {
        warnings.push("Snapshot catalog is incomplete".into());
    }
    Ok(ErGraph {
        nodes,
        edges,
        truncated,
        warnings,
    })
}
fn xml(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '&' => "&amp;".into(),
            '<' => "&lt;".into(),
            '>' => "&gt;".into(),
            '"' => "&quot;".into(),
            '\'' => "&apos;".into(),
            c if (c < ' ' && c != '\t' && c != '\n' && c != '\r')
                || c == '\u{fffe}'
                || c == '\u{ffff}' =>
            {
                "\u{fffd}".into()
            }
            c => c.to_string(),
        })
        .collect()
}
fn label(s: &str, max: usize) -> String {
    let mut result = s.chars().take(max).collect::<String>();
    if s.chars().count() > max {
        result.push('…');
    }
    xml(&result)
}
/// Simple grid layout; no external scripts/fonts/assets or executable links. Hard byte budget.
pub fn export_svg(snapshot: &SchemaSnapshot, limits: ErLimits) -> Result<String, SchemaError> {
    let graph = graph(snapshot, limits)?;
    let count = graph.nodes.len();
    let columns = (count.max(1) as f64).sqrt().ceil() as usize;
    let max_rows = graph
        .nodes
        .iter()
        .map(|n| n.columns.len())
        .max()
        .unwrap_or(0);
    let box_height = 60 + max_rows * 22;
    let step_x = 440;
    let step_y = box_height + 90;
    let width = columns * step_x + 40;
    let height = count.div_ceil(columns).max(1) * step_y + 70;
    let positions = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            (
                n.id.as_str(),
                (30 + (i % columns) * step_x, 50 + (i / columns) * step_y),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut out = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" role="img"><title>PostgreSQL ER diagram</title><desc>{}</desc><defs><marker id="arrow" markerWidth="10" markerHeight="10" refX="9" refY="3" orient="auto"><path d="M0,0 L0,6 L9,3 z" fill="#668"/></marker></defs><rect width="100%" height="100%" fill="#f7f8fa"/>"##,
        xml(&graph.warnings.join("; "))
    );
    if graph.truncated {
        out.push_str(r##"<text x="30" y="25" fill="#a32" font-family="sans-serif">Truncated schema — see export limits</text>"##);
    }
    for edge in &graph.edges {
        if let (Some(&(x, y)), Some(&(tx, ty))) = (
            positions.get(edge.from.as_str()),
            positions.get(edge.to.as_str()),
        ) {
            let path = if edge.from == edge.to {
                format!(
                    "M{},{} C{},{} {},{} {},{}",
                    x + 380,
                    y + 15,
                    x + 420,
                    y - 35,
                    x + 420,
                    y + 65,
                    x + 380,
                    y + 45
                )
            } else {
                format!("M{},{} L{},{}", x + 380, y + 20, tx, ty + 20)
            };
            write!(out,r##"<path id="{}" d="{path}" fill="none" stroke="#668" stroke-width="1.5" marker-end="url(#arrow)" data-from="{}" data-to="{}"><title>{}</title></path>"##,edge.id,edge.from,edge.to,xml(edge.constraint.as_deref().unwrap_or("foreign key"))).expect("write string");
        }
    }
    for node in &graph.nodes {
        let (x, y) = positions[node.id.as_str()];
        write!(out,r##"<g id="{}"><rect x="{x}" y="{y}" width="380" height="{}" rx="6" fill="white" stroke="#778"/><text x="{}" y="{}" font-family="sans-serif" font-size="15" font-weight="bold"><title>{}</title>{}</text>"##,node.id,60+node.columns.len()*22,x+12,y+25,xml(&node.name.sql()),label(&format!("{}.{}",node.name.schema,node.name.name),39)).expect("write string");
        for (i, c) in node.columns.iter().enumerate() {
            write!(out,r##"<text id="{}" x="{}" y="{}" font-family="monospace" font-size="12"><title>{}</title>{}</text>"##,c.id,x+12,y+53+i*22,xml(&format!("{} {}{}",c.name,c.data_type,if c.nullable { " NULL" } else { " NOT NULL" })),label(&format!("{}{}: {}{}",if c.primary_key { "PK " } else { "" },c.name,c.data_type,if c.nullable { " ?" } else { "" }),49)).expect("write string");
        }
        out.push_str("</g>");
        if out.len() > limits.max_svg_bytes {
            return Err(SchemaError(
                "SVG byte budget exceeded; reduce tables/columns/edges".into(),
            ));
        }
    }
    out.push_str("</svg>");
    if out.len() > limits.max_svg_bytes {
        return Err(SchemaError("SVG byte budget exceeded".into()));
    }
    Ok(out)
}
