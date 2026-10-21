//! Shared, search_path-independent table-local metadata witness for capture/apply.
use sha2::{Digest, Sha256};
pub const MAX_TABLE_METADATA_BYTES: usize = 1024 * 1024;
/// One SELECT, one TEXT-or-NULL canonical_json cell. NULL means missing or over
/// budget. Numeric-only OID substitution cannot introduce caller SQL. Raw catalog
/// node trees avoid search_path-sensitive pg_get_* deparsing.
pub fn table_metadata_query(oid: u32) -> String {
    include_str!("table_metadata.sql").replace("__RELATION_OID__", &oid.to_string())
}
/// Hash only the bounded canonical_json returned by table_metadata_query.
pub fn table_metadata_fingerprint(canonical_json: &str) -> String {
    format!("{:x}", Sha256::digest(canonical_json.as_bytes()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_is_one_bounded_select_and_digest_is_stable() {
        use sqlparser::{dialect::PostgreSqlDialect, parser::Parser};
        let query = table_metadata_query(u32::MAX);
        assert!(query.len() < 16 * 1024);
        assert_eq!(
            Parser::parse_sql(&PostgreSqlDialect {}, &query)
                .unwrap()
                .len(),
            1
        );
        assert!(!query.contains("__RELATION_OID__"));
        assert!(!query.contains("pg_get_expr"));
        assert_eq!(
            table_metadata_fingerprint("{}"),
            "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
        );
    }
}
