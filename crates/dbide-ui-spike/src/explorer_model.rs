//! Adapter-normalized explorer semantics, independent of the view.
pub fn is_table(kind: &str) -> bool {
    matches!(kind, "table" | "r")
}
pub fn matches(object: &dbide_bridge::SchemaObject, query: &str) -> bool {
    query.is_empty()
        || object.schema.to_lowercase().contains(query)
        || object.name.to_lowercase().contains(query)
        || object.columns.iter().any(|c| {
            c.name.to_lowercase().contains(query) || c.data_type.to_lowercase().contains(query)
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalized_adapter_table_kind_opens_data_viewer() {
        assert!(is_table("table"));
        assert!(is_table("r"));
        assert!(!is_table("view"));
    }
    #[test]
    fn search_includes_schema_and_column_types() {
        let object = dbide_bridge::SchemaObject {
            schema: "public".into(),
            name: "Orders".into(),
            kind: "table".into(),
            columns: vec![dbide_bridge::SchemaColumn {
                name: "CustomerId".into(),
                data_type: "bigint".into(),
                nullable: false,
                primary_key_ordinal: Some(1),
                foreign_key: false,
                default: None,
                extra: None,
            }],
        };
        for query in ["public", "orders", "customer", "bigint"] {
            assert!(matches(&object, query));
        }
        assert!(!matches(&object, "missing"));
    }
}
