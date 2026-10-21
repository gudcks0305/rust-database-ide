use dbide_data::*;
fn text(value: &str) -> Cell {
    Cell::Text(value.into())
}
fn limits() -> Limits {
    Limits {
        max_rows: 100,
        max_bytes: 2_000_000,
    }
}
fn spec(pg_type: PgType) -> TableSpec {
    TableSpec {
        schema: "public".into(),
        name: "typed".into(),
        columns: vec![
            EditColumn {
                name: "id".into(),
                pg_type: PgType::BigInt,
                nullable: false,
            },
            EditColumn {
                name: "value".into(),
                pg_type,
                nullable: true,
            },
        ],
        identity: Some(RowIdentity {
            kind: IdentityKind::PrimaryKey,
            columns: vec![0],
        }),
    }
}
fn insert(pg_type: PgType, value: Cell) -> Result<TransactionPlan> {
    plan_edits(
        &spec(pg_type),
        &[Edit::Insert {
            values: vec![Cell::Integer(1), value],
        }],
        limits(),
    )
}
#[test]
fn closed_scalar_casts_preserve_text_wire_payloads() {
    for (pg_type, value, cast) in [
        (PgType::SmallInt, "-32768", "int2"),
        (PgType::Integer, "2147483647", "int4"),
        (PgType::BigInt, "9223372036854775807", "int8"),
        (PgType::Boolean, "t", "bool"),
        (PgType::Uuid, "A0EEBC99-9C0B-4EF8-BB6D-6BB9BD380A11", "uuid"),
        (PgType::Date, "2024-02-29", "date"),
        (PgType::Timestamp, "2026-10-21 12:34:56.123456", "timestamp"),
        (
            PgType::TimestampTz,
            "2026-10-21T12:34:56.123456+09:00",
            "timestamptz",
        ),
        (
            PgType::Json,
            " {\"정확\":123456789012345678901234567890.00001} ",
            "json",
        ),
        (PgType::Jsonb, "{\"exact\":1e400}", "jsonb"),
        (PgType::DoublePrecision, "-1.2345678901234567e100", "float8"),
        (PgType::Numeric, "+000123.450000e-2", "numeric"),
    ] {
        let plan = insert(pg_type, text(value)).unwrap();
        let statement = &plan.statements[0];
        assert_eq!(statement.parameters[1].pg_type, pg_type);
        assert_eq!(statement.parameters[1].value.as_deref(), Some(value));
        assert!(
            statement
                .sql
                .contains(&format!("$2::pg_catalog.text::pg_catalog.{cast}"))
        );
        if value.len() > 2 {
            assert!(!statement.sql.contains(value));
        }
        let null = insert(pg_type, Cell::Null).unwrap();
        assert_eq!(null.statements[0].parameters[1].value, None);
    }
}
#[test]
fn scalar_validation_rejects_invalid_dates_json_uuid_integer_ranges_and_double_overflow() {
    for (pg_type, value) in [
        (PgType::SmallInt, "32768"),
        (PgType::Integer, "2147483648"),
        (PgType::BigInt, "9223372036854775808"),
        (PgType::Uuid, "a0eebc99-not-a-uuid"),
        (PgType::Date, "2023-02-29"),
        (PgType::Date, "0000-01-01"),
        (PgType::Date, "2024-04-31"),
        (PgType::Timestamp, "2024-01-01 24:00:00"),
        (PgType::Timestamp, "2024-01-01 01:02:03+00"),
        (PgType::TimestampTz, "2024-01-01 01:02:03"),
        (PgType::TimestampTz, "2024-01-01 01:02:03+16:00"),
        (PgType::Json, "{\"x\":}"),
        (PgType::Jsonb, "{\"x\":1} trailing"),
        (PgType::DoublePrecision, "1e9999"),
        (PgType::Numeric, "+-1"),
        (PgType::Numeric, "NaN"),
    ] {
        assert!(insert(pg_type, text(value)).is_err(), "{pg_type:?} {value}");
    }
    for value in ["NaN", "Infinity", "-Infinity", "-0.0"] {
        assert!(insert(PgType::DoublePrecision, text(value)).is_ok());
    }
    for value in ["2024-01-01 01:02:03+00", "2024-01-01T01:02:03Z"] {
        assert!(insert(PgType::TimestampTz, text(value)).is_ok());
    }
}
#[test]
fn defaulted_inserts_generate_only_explicit_default_tokens_and_contiguous_parameters() {
    let mut table = spec(PgType::Jsonb);
    table.columns.push(EditColumn {
        name: "created_at".into(),
        pg_type: PgType::TimestampTz,
        nullable: false,
    });
    table.columns.push(EditColumn {
        name: "note".into(),
        pg_type: PgType::Text,
        nullable: true,
    });
    let injection = "DEFAULT); DROP TABLE users; --";
    let edits = [Edit::InsertDefaulted {
        values: vec![
            InsertValue::Default,
            InsertValue::Value(text("{\"한글\":true}")),
            InsertValue::Default,
            InsertValue::Value(text(injection)),
        ],
    }];
    let plan = plan_edits(&table, &edits, limits()).unwrap();
    let statement = &plan.statements[0];
    assert_eq!(
        statement.sql,
        "INSERT INTO \"public\".\"typed\" (\"id\", \"value\", \"created_at\", \"note\") VALUES (DEFAULT, $1::pg_catalog.text::pg_catalog.jsonb, DEFAULT, $2::pg_catalog.text)"
    );
    assert_eq!(statement.parameters.len(), 2);
    assert_eq!(statement.parameters[1].value.as_deref(), Some(injection));
    assert!(!statement.sql.contains(injection));
    assert!(!plan.destructive);
    let all_default = [Edit::InsertDefaulted {
        values: vec![InsertValue::Default; table.columns.len()],
    }];
    assert!(
        plan_edits(&table, &all_default, limits())
            .unwrap()
            .statements[0]
            .parameters
            .is_empty()
    );
    let missing = [Edit::InsertDefaulted {
        values: vec![InsertValue::Default],
    }];
    assert!(plan_edits(&table, &missing, limits()).is_err());
    let null_key = [Edit::InsertDefaulted {
        values: vec![
            InsertValue::Value(Cell::Null),
            InsertValue::Default,
            InsertValue::Default,
            InsertValue::Default,
        ],
    }];
    assert!(plan_edits(&table, &null_key, limits()).is_err());
    assert_eq!(
        serde_json::from_str::<Edit>(&serde_json::to_string(&edits[0]).unwrap()).unwrap(),
        edits[0]
    );
}
#[test]
fn json_original_predicates_compare_stored_text_without_nonexistent_json_equality() {
    for (pg_type, expected) in [
        (
            PgType::Json,
            "\"value\"::pg_catalog.text IS NOT DISTINCT FROM $3::pg_catalog.text",
        ),
        (
            PgType::Jsonb,
            "\"value\" IS NOT DISTINCT FROM $3::pg_catalog.text::pg_catalog.jsonb",
        ),
    ] {
        let original = vec![Cell::Integer(1), text(" {\"x\":1} ")];
        let replacement = vec![Cell::Integer(1), text("{\"x\":2}")];
        let plan = plan_edits(
            &spec(pg_type),
            &[Edit::Update {
                original,
                replacement,
            }],
            limits(),
        )
        .unwrap();
        assert!(plan.statements[0].sql.contains(expected));
        assert_eq!(
            plan.statements[0].parameters[2].value.as_deref(),
            Some(" {\"x\":1} ")
        );
    }
}
fn composite_table() -> TableSpec {
    TableSpec {
        schema: "s".into(),
        name: "t".into(),
        columns: vec![
            EditColumn {
                name: "uuid".into(),
                pg_type: PgType::Uuid,
                nullable: false,
            },
            EditColumn {
                name: "numeric".into(),
                pg_type: PgType::Numeric,
                nullable: false,
            },
            EditColumn {
                name: "text".into(),
                pg_type: PgType::Text,
                nullable: false,
            },
        ],
        identity: Some(RowIdentity {
            kind: IdentityKind::UniqueConstraint,
            columns: vec![0, 1],
        }),
    }
}
#[test]
fn composite_uuid_numeric_identity_normalizes_aliases_without_changing_parameters() {
    let table = composite_table();
    for (first, second) in [
        ("+00100.000e-2", "1e0"),
        ("-0.000", "+0e10"),
        ("-1000e-3", "-1.0000"),
    ] {
        let originals = [
            vec![
                text("A0EEBC99-9C0B-4EF8-BB6D-6BB9BD380A11"),
                text(first),
                text("a"),
            ],
            vec![
                text("a0eebc999c0b4ef8bb6d6bb9bd380a11"),
                text(second),
                text("b"),
            ],
        ];
        let edits: Vec<Edit> = originals
            .into_iter()
            .map(|original| Edit::Delete { original })
            .collect();
        let error = plan_edits(&table, &edits, limits()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("multiple edits target the same original identity")
        );
    }
    let original = vec![
        text("A0EEBC99-9C0B-4EF8-BB6D-6BB9BD380A11"),
        text("+00100.000e-2"),
        text("a"),
    ];
    let plan = plan_edits(&table, &[Edit::Delete { original }], limits()).unwrap();
    assert_eq!(
        plan.statements[0].parameters[0].value.as_deref(),
        Some("A0EEBC99-9C0B-4EF8-BB6D-6BB9BD380A11")
    );
    assert_eq!(
        plan.statements[0].parameters[1].value.as_deref(),
        Some("+00100.000e-2")
    );
}
#[test]
fn integer_and_boolean_identity_text_aliases_cannot_bypass_duplicate_detection() {
    for (pg_type, first, second) in [
        (PgType::SmallInt, Cell::Integer(1), text("+01")),
        (PgType::Integer, Cell::Integer(-1), text("-01")),
        (PgType::BigInt, Cell::Integer(0), text("-0")),
        (PgType::Boolean, Cell::Bool(true), text("T")),
    ] {
        let mut table = spec(PgType::Text);
        table.columns[0].pg_type = pg_type;
        let edits = [
            Edit::Delete {
                original: vec![first, text("a")],
            },
            Edit::Delete {
                original: vec![second, text("b")],
            },
        ];
        assert!(
            plan_edits(&table, &edits, limits())
                .unwrap_err()
                .to_string()
                .contains("multiple edits target")
        );
    }
}
#[test]
fn numeric_validation_enforces_pg_precision_scale_exponent_and_input_budgets_without_expansion() {
    for valid in ["1e131071", "1e-16383", "-0.00", "000000.00000"] {
        assert!(insert(PgType::Numeric, text(valid)).is_ok(), "{valid}");
    }
    for invalid in [
        "1e131072",
        "1e-16384",
        "1e999999999999999999999999999",
        "0e147456",
        "1e-9223372036854775808",
    ] {
        assert!(insert(PgType::Numeric, text(invalid)).is_err(), "{invalid}");
    }
    assert!(insert(PgType::Numeric, text(&format!("1{}", "0".repeat(131_071)))).is_ok());
    assert!(insert(PgType::Numeric, text(&format!("1{}", "0".repeat(131_072)))).is_err());
    assert!(insert(PgType::Numeric, text(&format!("0.{}1", "0".repeat(16_382)))).is_ok());
    assert!(insert(PgType::Numeric, text(&format!("0.{}1", "0".repeat(16_383)))).is_err());
    assert!(insert(PgType::Numeric, text(&"0".repeat(160_001))).is_err());
}
#[test]
fn unnormalized_float_timestamp_json_keys_are_explicitly_readonly() {
    for pg_type in [
        PgType::DoublePrecision,
        PgType::Timestamp,
        PgType::TimestampTz,
        PgType::Json,
        PgType::Jsonb,
    ] {
        let mut table = spec(PgType::Text);
        table.columns[0].pg_type = pg_type;
        let error = plan_edits(
            &table,
            &[Edit::InsertDefaulted {
                values: vec![InsertValue::Default, InsertValue::Default],
            }],
            limits(),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("requires PostgreSQL-equivalent key normalization")
        );
    }
}
