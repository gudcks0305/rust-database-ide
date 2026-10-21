//! Closed, bounded PostgreSQL read predicates. No database access or arbitrary SQL input.
use crate::{Cell, Error, PgType, Result, pg_values::validate_scalar};

pub const TABLE_SOURCE_ALIAS: &str = "__dbide_source";
pub const TABLE_PAGE_SIZES: [usize; 4] = [50, 100, 250, 500];
pub const MAX_TABLE_FILTERS: usize = 8;
pub const MAX_FILTER_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterOperator {
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    IsNull,
    IsNotNull,
}
impl FilterOperator {
    pub const ALL: [Self; 8] = [
        Self::Equal,
        Self::NotEqual,
        Self::Less,
        Self::LessEqual,
        Self::Greater,
        Self::GreaterEqual,
        Self::IsNull,
        Self::IsNotNull,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Equal => "=",
            Self::NotEqual => "≠",
            Self::Less => "<",
            Self::LessEqual => "≤",
            Self::Greater => ">",
            Self::GreaterEqual => "≥",
            Self::IsNull => "IS NULL",
            Self::IsNotNull => "IS NOT NULL",
        }
    }
    pub fn needs_value(self) -> bool {
        !matches!(self, Self::IsNull | Self::IsNotNull)
    }
    fn sql(self) -> &'static str {
        match self {
            Self::NotEqual => "<>",
            Self::LessEqual => "<=",
            Self::GreaterEqual => ">=",
            _ => self.label(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableFilter {
    pub column: usize,
    pub operator: FilterOperator,
    pub value: Cell,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableSort {
    pub column: usize,
    pub direction: SortDirection,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableQuery {
    pub filters: Vec<TableFilter>,
    pub sort: Option<TableSort>,
    pub page_size: usize,
}
impl Default for TableQuery {
    fn default() -> Self {
        Self {
            filters: vec![],
            sort: None,
            page_size: 100,
        }
    }
}
#[derive(Clone, Debug)]
pub struct QueryColumn {
    pub name: String,
    pub pg_type: Option<PgType>,
}
fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}
fn identifier(value: &str) -> Result<String> {
    if value.is_empty() || value.contains('\0') {
        return Err(invalid("invalid captured query identifier"));
    }
    Ok(format!("\"{}\"", value.replace('"', "\"\"")))
}
fn cast(ty: PgType) -> &'static str {
    match ty {
        PgType::Text => "text",
        PgType::Boolean => "bool",
        PgType::SmallInt => "int2",
        PgType::Integer => "int4",
        PgType::BigInt => "int8",
        PgType::Numeric => "numeric",
        PgType::Bytea => "bytea",
        PgType::Uuid => "uuid",
        PgType::Date => "date",
        PgType::Timestamp => "timestamp",
        PgType::TimestampTz => "timestamptz",
        PgType::Json => "json",
        PgType::Jsonb => "jsonb",
        PgType::DoublePrecision => "float8",
    }
}
fn literal(value: &Cell, ty: PgType) -> Result<String> {
    if *value == Cell::Null {
        return Err(invalid("use IS NULL or IS NOT NULL for SQL NULL"));
    }
    let payload_size = match value {
        Cell::Text(value) | Cell::Decimal(value) => value.len(),
        Cell::Binary(value) => value.len().saturating_mul(2).saturating_add(2),
        _ => 0,
    };
    if payload_size > MAX_FILTER_BYTES {
        return Err(Error::Limit("filter value exceeds 4096 bytes".into()));
    }
    let text = match value {
        Cell::Text(v) if ty != PgType::Bytea => v.clone(),
        Cell::Binary(v) if ty == PgType::Bytea => format!(
            "\\x{}",
            v.iter().map(|v| format!("{v:02x}")).collect::<String>()
        ),
        Cell::Bool(v) if ty == PgType::Boolean => v.to_string(),
        Cell::Integer(v)
            if matches!(
                ty,
                PgType::SmallInt | PgType::Integer | PgType::BigInt | PgType::Numeric
            ) =>
        {
            v.to_string()
        }
        Cell::Decimal(v) if ty == PgType::Numeric => v.clone(),
        _ => return Err(invalid("filter value does not match captured column type")),
    };
    if text.len() > MAX_FILTER_BYTES {
        return Err(Error::Limit("filter value exceeds 4096 bytes".into()));
    }
    if text.contains('\0') {
        return Err(invalid("filter value contains NUL"));
    }
    validate_scalar(value, ty)?;
    // Same E'' encoding contract as metadata capture: independent of standard_conforming_strings.
    Ok(format!(
        "E'{}'::pg_catalog.{}",
        text.replace('\\', "\\\\").replace('\'', "''"),
        cast(ty)
    ))
}
/// Validates captured column indices and emits only a WHERE/ORDER/LIMIT/OFFSET suffix.
/// Primary-key indices append ASC tie-breakers without duplicating the selected sort key.
/// The caller must alias the source relation with `TABLE_SOURCE_ALIAS`; qualifying every
/// sort key ensures PostgreSQL orders raw values rather than encoded display projections.
pub fn table_query_suffix(
    columns: &[QueryColumn],
    primary_key: &[usize],
    query: &TableQuery,
    page: usize,
) -> Result<String> {
    if columns.is_empty() {
        return Err(invalid("captured table has no columns"));
    }
    if !TABLE_PAGE_SIZES.contains(&query.page_size) {
        return Err(invalid("unsupported table page size"));
    }
    if query.filters.len() > MAX_TABLE_FILTERS {
        return Err(Error::Limit("at most eight table filters".into()));
    }
    let offset = page
        .checked_mul(query.page_size)
        .filter(|offset| *offset <= i64::MAX as usize)
        .ok_or_else(|| invalid("table page offset overflow"))?;
    let qualified =
        |name: &str| identifier(name).map(|name| format!("\"{TABLE_SOURCE_ALIAS}\".{name}"));
    let column = |index: usize| {
        columns
            .get(index)
            .ok_or_else(|| invalid("query references an uncaptured column"))
    };
    let mut predicates = vec![];
    for filter in &query.filters {
        let c = column(filter.column)?;
        let name = qualified(&c.name)?;
        if !filter.operator.needs_value() {
            if filter.value != Cell::Null {
                return Err(invalid("NULL predicate must not carry a value"));
            }
            predicates.push(format!("{name} {}", filter.operator.sql()));
        } else {
            let ty = c.pg_type.ok_or_else(|| {
                invalid("value filters unsupported for this captured type; use NULL predicates")
            })?;
            if ty == PgType::Json {
                return Err(invalid(
                    "PostgreSQL json has no comparison operator; use NULL predicates",
                ));
            }
            predicates.push(format!(
                "{name} {} {}",
                filter.operator.sql(),
                literal(&filter.value, ty)?
            ));
        }
    }
    let mut order = vec![];
    if let Some(sort) = query.sort {
        let c = column(sort.column)?;
        if c.pg_type.is_none() || c.pg_type == Some(PgType::Json) {
            return Err(invalid("ordering unsupported for this captured type"));
        }
        order.push(format!(
            "{} {}",
            qualified(&c.name)?,
            match sort.direction {
                SortDirection::Ascending => "ASC",
                SortDirection::Descending => "DESC",
            }
        ));
    }
    for &index in primary_key {
        if query.sort.is_some_and(|sort| sort.column == index) {
            continue;
        }
        let key = format!("{} ASC", qualified(&column(index)?.name)?);
        if !order.contains(&key) {
            order.push(key);
        }
    }
    Ok(format!(
        "{}{} LIMIT {} OFFSET {offset};",
        if predicates.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", predicates.join(" AND "))
        },
        if order.is_empty() {
            String::new()
        } else {
            format!(" ORDER BY {}", order.join(", "))
        },
        query.page_size
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn columns() -> Vec<QueryColumn> {
        vec![
            QueryColumn {
                name: "id".into(),
                pg_type: Some(PgType::Integer),
            },
            QueryColumn {
                name: "display\"name".into(),
                pg_type: Some(PgType::Text),
            },
            QueryColumn {
                name: "created".into(),
                pg_type: Some(PgType::Date),
            },
        ]
    }
    #[test]
    fn combined_filters_sort_pk_tiebreak_and_paging() {
        let query = TableQuery {
            filters: vec![
                TableFilter {
                    column: 0,
                    operator: FilterOperator::GreaterEqual,
                    value: Cell::Text("7".into()),
                },
                TableFilter {
                    column: 1,
                    operator: FilterOperator::IsNotNull,
                    value: Cell::Null,
                },
            ],
            sort: Some(TableSort {
                column: 1,
                direction: SortDirection::Descending,
            }),
            page_size: 250,
        };
        assert_eq!(
            table_query_suffix(&columns(), &[0], &query, 3).unwrap(),
            " WHERE \"__dbide_source\".\"id\" >= E'7'::pg_catalog.int4 AND \"__dbide_source\".\"display\"\"name\" IS NOT NULL ORDER BY \"__dbide_source\".\"display\"\"name\" DESC, \"__dbide_source\".\"id\" ASC LIMIT 250 OFFSET 750;"
        );
        let query = TableQuery {
            sort: Some(TableSort {
                column: 0,
                direction: SortDirection::Descending,
            }),
            ..TableQuery::default()
        };
        assert_eq!(
            table_query_suffix(&columns(), &[0], &query, 0).unwrap(),
            " ORDER BY \"__dbide_source\".\"id\" DESC LIMIT 100 OFFSET 0;"
        );
    }
    #[test]
    fn values_are_typed_and_null_predicates_explicit() {
        for (column, value) in [
            (0, "2147483648"),
            (0, "1 OR true"),
            (2, "2025-02-29"),
            (2, "2024-01-01'; DROP TABLE x; --"),
        ] {
            let q = TableQuery {
                filters: vec![TableFilter {
                    column,
                    operator: FilterOperator::Equal,
                    value: Cell::Text(value.into()),
                }],
                ..TableQuery::default()
            };
            assert!(table_query_suffix(&columns(), &[0], &q, 0).is_err());
        }
        let mut q = TableQuery {
            filters: vec![TableFilter {
                column: 1,
                operator: FilterOperator::Equal,
                value: Cell::Null,
            }],
            ..TableQuery::default()
        };
        assert!(table_query_suffix(&columns(), &[0], &q, 0).is_err());
        q.filters[0].operator = FilterOperator::IsNull;
        assert!(
            table_query_suffix(&columns(), &[0], &q, 0)
                .unwrap()
                .contains("IS NULL")
        );
        q.filters[0].value = Cell::Text("ignored data".into());
        assert!(table_query_suffix(&columns(), &[0], &q, 0).is_err());
    }
    #[test]
    fn text_injection_is_one_escaped_typed_literal() {
        let q = TableQuery {
            filters: vec![TableFilter {
                column: 1,
                operator: FilterOperator::Equal,
                value: Cell::Text("한글\\'; SELECT pg_sleep(9); --".into()),
            }],
            ..TableQuery::default()
        };
        let sql = table_query_suffix(&columns(), &[0], &q, 0).unwrap();
        assert_eq!(
            sql,
            " WHERE \"__dbide_source\".\"display\"\"name\" = E'한글\\\\''; SELECT pg_sleep(9); --'::pg_catalog.text ORDER BY \"__dbide_source\".\"id\" ASC LIMIT 100 OFFSET 0;"
        );
    }
    #[test]
    fn budgets_column_scope_and_unsupported_types_are_closed() {
        let mut q = TableQuery::default();
        for size in [0, 1, 501, usize::MAX] {
            q.page_size = size;
            assert!(table_query_suffix(&columns(), &[0], &q, 0).is_err());
        }
        for size in TABLE_PAGE_SIZES {
            q.page_size = size;
            assert!(
                table_query_suffix(&columns(), &[0], &q, 1)
                    .unwrap()
                    .ends_with(&format!("LIMIT {size} OFFSET {size};"))
            );
        }
        q.page_size = 100;
        assert!(table_query_suffix(&columns(), &[0], &q, usize::MAX).is_err());
        q.sort = Some(TableSort {
            column: 3,
            direction: SortDirection::Ascending,
        });
        assert!(table_query_suffix(&columns(), &[0], &q, 0).is_err());
        q.sort = None;
        assert!(table_query_suffix(&columns(), &[3], &q, 0).is_err());
        q.filters = vec![TableFilter {
            column: 1,
            operator: FilterOperator::Equal,
            value: Cell::Text("x".repeat(MAX_FILTER_BYTES + 1)),
        }];
        assert!(table_query_suffix(&columns(), &[0], &q, 0).is_err());
        q.filters[0].value = Cell::Text("\0".into());
        assert!(table_query_suffix(&columns(), &[0], &q, 0).is_err());
        q.filters[0].value = Cell::Text("x".into());
        q.filters = vec![q.filters[0].clone(); MAX_TABLE_FILTERS + 1];
        assert!(table_query_suffix(&columns(), &[0], &q, 0).is_err());
        for pg_type in [None, Some(PgType::Json)] {
            let c = vec![QueryColumn {
                name: "opaque".into(),
                pg_type,
            }];
            let mut q = TableQuery {
                filters: vec![TableFilter {
                    column: 0,
                    operator: FilterOperator::Equal,
                    value: Cell::Text("{}".into()),
                }],
                ..TableQuery::default()
            };
            assert!(table_query_suffix(&c, &[], &q, 0).is_err());
            q.filters[0] = TableFilter {
                column: 0,
                operator: FilterOperator::IsNull,
                value: Cell::Null,
            };
            assert!(table_query_suffix(&c, &[], &q, 0).is_ok());
            q.sort = Some(TableSort {
                column: 0,
                direction: SortDirection::Ascending,
            });
            assert!(table_query_suffix(&c, &[], &q, 0).is_err());
        }
    }
}
