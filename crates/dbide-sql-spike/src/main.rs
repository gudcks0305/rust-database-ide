use dbide_sql_spike::{FIXTURES, inspect, measure, parser};
use serde_json::json;

fn main() {
    let mut parser = parser();
    let fixtures: Vec<_> = FIXTURES.iter().map(|f| inspect(f, &mut parser)).collect();
    let passed = fixtures.iter().all(|r| r.contract_passed != Some(false));
    let sizes = if std::env::args().any(|arg| arg == "--large") {
        vec![10 * 1024, 1024 * 1024, 10 * 1024 * 1024]
    } else {
        vec![10 * 1024, 1024 * 1024]
    };
    let measurements: Vec<_> = sizes.into_iter().map(|size| measure(size, 5)).collect();
    let incremental_ok = measurements
        .iter()
        .all(|m| m.incremental_matches_full_reparse);
    let report = json!({
        "schema_version": 1, "kind": "sql_parser_spike", "contract_passed": passed && incremental_ok,
        "versions": {"sqlparser":"0.63.0", "tree_sitter":"0.25.10", "tree_sitter_sequel":"0.3.11"},
        "fixtures": fixtures, "measurements": measurements,
        "limitations": ["Synthetic parser-only workload; not DataGrip or UI benchmark.",
            "Tree structure/range equality does not establish semantic name resolution or full dialect compatibility.",
            "Incremental timing includes text copy/tree edit; five samples only, no tail latency claim.",
            "Unsupported and incomplete SQL remains executable via raw driver path."]
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    if !passed || !incremental_ok {
        std::process::exit(1);
    }
}
