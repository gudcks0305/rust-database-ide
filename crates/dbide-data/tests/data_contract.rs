use dbide_data::*;

fn columns(names: &[&str]) -> Vec<Column> {
    names
        .iter()
        .map(|name| Column {
            name: (*name).into(),
        })
        .collect()
}
fn text(value: &str) -> Cell {
    Cell::Text(value.into())
}
fn limits() -> Limits {
    Limits {
        max_rows: 100,
        max_bytes: 100_000,
    }
}
fn table_data(rows: Vec<Vec<Cell>>) -> TabularData {
    TabularData {
        columns: columns(&["id", "value"]),
        rows,
    }
}

#[test]
fn csv_roundtrips_null_empty_quotes_newlines_and_unicode() {
    let data = TabularData {
        columns: columns(&["id", "이름"]),
        rows: vec![
            vec![text("1"), text("한글 😊, \"quote\"\nsecond line")],
            vec![text("2"), Cell::Null],
            vec![text("3"), text("")],
            vec![text("4"), text("NULL")],
        ],
    };
    let mut bytes = Vec::new();
    write_csv(
        &mut bytes,
        &data.columns,
        &data.rows,
        &CsvOptions::default(),
        limits(),
    )
    .unwrap();
    let decoded = read_csv(
        bytes.as_slice(),
        &CsvOptions::default(),
        None,
        limits(),
        ReadMode::Import,
    )
    .unwrap();
    assert_eq!(decoded.data, data);
    assert!(!decoded.truncated);
}

#[test]
fn incremental_csv_matches_existing_contract_and_poisoned_encoder_cannot_finish() {
    let data = table_data(vec![
        vec![text("1"), Cell::Null],
        vec![text("2"), text("")],
        vec![text("3"), text("한글,\"quoted\"\nline")],
    ]);
    let mut incremental = Vec::new();
    let mut writer = CsvStreamWriter::new(
        &mut incremental,
        &data.columns,
        &CsvOptions::default(),
        limits(),
    )
    .unwrap();
    for row in &data.rows {
        writer.write_row(row).unwrap();
    }
    writer.finish().unwrap();
    let mut existing = Vec::new();
    write_csv(
        &mut existing,
        &data.columns,
        &data.rows,
        &CsvOptions::default(),
        limits(),
    )
    .unwrap();
    assert_eq!(incremental, existing);
    let mut writer = CsvStreamWriter::new(
        Vec::new(),
        &columns(&["v"]),
        &CsvOptions::default(),
        limits(),
    )
    .unwrap();
    assert!(writer.write_row(&[text("\\N")]).is_err());
    assert!(writer.write_row(&[text("later")]).is_err());
    assert!(writer.finish().is_err());
}

#[test]
fn incremental_csv_surfaces_partial_writer_and_final_flush_failures() {
    struct FailingWrite;
    impl std::io::Write for FailingWrite {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("owned writer failure"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = CsvStreamWriter::new(
        FailingWrite,
        &columns(&["v"]),
        &CsvOptions::default(),
        limits(),
    )
    .unwrap();
    assert!(writer.write_row(&[text(&"x".repeat(20_000))]).is_err());
    assert!(writer.finish().is_err());
    let mut writer = CsvStreamWriter::new(
        FailingWrite,
        &columns(&["v"]),
        &CsvOptions::default(),
        limits(),
    )
    .unwrap();
    writer.write_row(&[text("buffered")]).unwrap();
    assert!(writer.finish().is_err());
}

#[test]
fn incremental_csv_null_marker_expansion_hits_output_cap_and_poisoned_finish() {
    let columns = columns(&["v"; 64]);
    let options = CsvOptions {
        null_marker: Some("n".repeat(4096)),
        ..CsvOptions::default()
    };
    let mut writer = CsvStreamWriter::new(
        Vec::new(),
        &columns,
        &options,
        Limits {
            max_rows: 1,
            max_bytes: 512,
        },
    )
    .unwrap();
    // Logical header/NULL payload fits; expanded markers exceed the encoded
    // cap. Only the current field is expanded before that cap stops encoding.
    let error = writer.write_row(&vec![Cell::Null; 64]).unwrap_err();
    assert!(
        error.to_string().contains("encoded output byte budget"),
        "{error}"
    );
    assert!(writer.finish().is_err());
}

#[test]
fn csv_null_marker_collision_is_rejected_and_configurable() {
    let data = table_data(vec![vec![text("1"), text("\\N")]]);
    assert!(
        write_csv(
            Vec::new(),
            &data.columns,
            &data.rows,
            &CsvOptions::default(),
            limits()
        )
        .is_err()
    );
    let options = CsvOptions {
        null_marker: Some("<NULL>".into()),
        ..Default::default()
    };
    let mut bytes = Vec::new();
    write_csv(&mut bytes, &data.columns, &data.rows, &options, limits()).unwrap();
    assert_eq!(
        read_csv(bytes.as_slice(), &options, None, limits(), ReadMode::Import)
            .unwrap()
            .data,
        data
    );
    let invalid = CsvOptions {
        null_marker: Some(String::new()),
        ..Default::default()
    };
    assert!(read_csv(&b"x\n\n"[..], &invalid, None, limits(), ReadMode::Import).is_err());
}

#[test]
fn csv_formula_escaping_requires_explicit_option() {
    let data = table_data(vec![vec![text("  =SUM(1,2)"), text("@cmd")]]);
    let mut preserved = Vec::new();
    write_csv(
        &mut preserved,
        &data.columns,
        &data.rows,
        &CsvOptions::default(),
        limits(),
    )
    .unwrap();
    assert_eq!(
        read_csv(
            preserved.as_slice(),
            &CsvOptions::default(),
            None,
            limits(),
            ReadMode::Import
        )
        .unwrap()
        .data,
        data
    );
    let escaped = CsvOptions {
        formula_mode: FormulaMode::Escape,
        ..Default::default()
    };
    let mut bytes = Vec::new();
    write_csv(&mut bytes, &data.columns, &data.rows, &escaped, limits()).unwrap();
    let result = read_csv(bytes.as_slice(), &escaped, None, limits(), ReadMode::Import).unwrap();
    assert_eq!(
        result.data.rows[0],
        vec![text("'  =SUM(1,2)"), text("'@cmd")]
    );
}

#[test]
fn csv_rejects_utf8_arity_invalid_options_and_budgets() {
    let options = CsvOptions::default();
    assert!(
        read_csv(
            &b"x\n\xff\n"[..],
            &options,
            None,
            limits(),
            ReadMode::Import
        )
        .is_err()
    );
    assert!(
        read_csv(
            &b"a,b\n1,2,3\n"[..],
            &options,
            None,
            limits(),
            ReadMode::Import
        )
        .is_err()
    );
    let small = Limits {
        max_rows: 1,
        max_bytes: 100,
    };
    assert!(read_csv(&b"a\n1\n2\n"[..], &options, None, small, ReadMode::Import).is_err());
    let preview = read_csv(&b"a\n1\n2\n"[..], &options, None, small, ReadMode::Preview).unwrap();
    assert_eq!(preview.data.rows.len(), 1);
    assert!(preview.truncated);
    let tiny = Limits {
        max_rows: 10,
        max_bytes: 5,
    };
    assert!(read_csv(&b"a\n1234567\n"[..], &options, None, tiny, ReadMode::Import).is_err());
    let invalid = CsvOptions {
        delimiter: b'\n',
        ..options
    };
    assert!(read_csv(&b"a\n"[..], &invalid, None, limits(), ReadMode::Import).is_err());
    assert!(
        Limits {
            max_rows: 0,
            max_bytes: 1
        }
        .validate()
        .is_err()
    );
    assert!(
        Limits {
            max_rows: 1,
            max_bytes: usize::MAX
        }
        .validate()
        .is_err()
    );
}

#[test]
fn tsv_headerless_and_typed_csv_contract_are_explicit() {
    let options = CsvOptions {
        delimiter: b'\t',
        header: false,
        ..Default::default()
    };
    let parsed = read_csv(
        &b"1\ttext\n"[..],
        &options,
        Some(columns(&["id", "value"])),
        limits(),
        ReadMode::Import,
    )
    .unwrap();
    assert_eq!(parsed.data.rows, vec![vec![text("1"), text("text")]]);
    assert!(
        read_csv(
            &b"1\ttext\n"[..],
            &options,
            None,
            limits(),
            ReadMode::Import
        )
        .is_err()
    );
    let data = table_data(vec![vec![
        Cell::Integer(1),
        Cell::Decimal("1.0000000000000000001".into()),
    ]]);
    assert!(write_csv(Vec::new(), &data.columns, &data.rows, &options, limits()).is_err());
    let textual = CsvOptions {
        scalar_mode: CsvScalarMode::TextualValues,
        ..options
    };
    let mut bytes = Vec::new();
    write_csv(&mut bytes, &data.columns, &data.rows, &textual, limits()).unwrap();
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        "1\t1.0000000000000000001\n"
    );
}

#[test]
fn tagged_json_preserves_exact_types_without_inference() {
    let data = TabularData {
        columns: columns(&["a", "b", "c", "d", "e", "f", "g"]),
        rows: vec![vec![
            Cell::Null,
            text(""),
            text("한글 😊"),
            Cell::Integer(i64::MAX),
            Cell::Decimal("-999999999999999999999999.000000000000000000001e+12".into()),
            Cell::Bool(false),
            Cell::Binary(vec![0, 255, 123]),
        ]],
    };
    let mut bytes = Vec::new();
    write_json(&mut bytes, &data, limits()).unwrap();
    assert_eq!(read_json(bytes.as_slice(), limits()).unwrap(), data);
    assert!(read_json(&b"{\"a\":1.234567890123456789}"[..], limits()).is_err());
    assert!(
        read_json(
            bytes.as_slice(),
            Limits {
                max_rows: 1,
                max_bytes: 10
            }
        )
        .is_err()
    );
    let invalid = table_data(vec![vec![text("1"), Cell::Decimal("NaN".into())]]);
    assert!(write_json(Vec::new(), &invalid, limits()).is_err());
}

#[test]
fn encoded_output_limits_are_enforced() {
    let data = TabularData {
        columns: columns(&["x"]),
        rows: vec![vec![text("\"\"\"")]],
    };
    let small = Limits {
        max_rows: 1,
        max_bytes: 6,
    };
    assert!(
        write_csv(
            Vec::new(),
            &data.columns,
            &data.rows,
            &CsvOptions::default(),
            small
        )
        .is_err()
    );
    assert!(write_json(Vec::new(), &data, small).is_err());
}

#[test]
fn explicit_import_mapping_previews_types_and_nullability() {
    let data = table_data(vec![
        vec![text("1"), Cell::Null],
        vec![text("2"), text("42")],
    ]);
    let mapping = vec![
        ColumnMapping {
            source_index: 1,
            target_name: "amount".into(),
            expected_kind: CellKind::Integer,
            nullable: true,
        },
        ColumnMapping {
            source_index: 0,
            target_name: "key".into(),
            expected_kind: CellKind::Text,
            nullable: false,
        },
    ];
    let preview = preview_import(&data, &mapping, limits()).unwrap();
    assert!(!preview.ready());
    assert_eq!(
        preview.issues,
        vec![MappingIssue {
            row: 1,
            column: 0,
            actual: CellKind::Text
        }]
    );
    assert_eq!(preview.rows[1], vec![text("42"), text("2")]);
    assert!(preview_import(&data, &mapping[..1], limits()).is_err());
    let mut duplicate = mapping;
    duplicate[1].source_index = 1;
    assert!(preview_import(&data, &duplicate, limits()).is_err());
}

#[test]
fn data_diff_explicit_keys_classifies_insert_delete_change_and_unchanged() {
    let before = table_data(vec![
        vec![text("1"), Cell::Null],
        vec![text("2"), text("gone")],
        vec![text("4"), text("same")],
    ]);
    let after = table_data(vec![
        vec![text("1"), text("")],
        vec![text("3"), text("new")],
        vec![text("4"), text("same")],
    ]);
    let diff = diff_by_key(&before, &after, &[0], limits()).unwrap();
    assert_eq!(diff.unchanged, 1);
    assert!(
        matches!(&diff.differences[0], RowDifference::Change { changed_columns, .. } if changed_columns == &[1])
    );
    assert!(matches!(&diff.differences[1], RowDifference::Delete { .. }));
    assert!(matches!(&diff.differences[2], RowDifference::Insert { .. }));
}

#[test]
fn data_diff_rejects_missing_duplicate_null_and_ambiguous_keys() {
    let data = table_data(vec![vec![text("1"), text("x")]]);
    let duplicate = table_data(vec![vec![text("1"), text("x")], vec![text("1"), text("y")]]);
    assert!(diff_by_key(&duplicate, &data, &[0], limits()).is_err());
    assert!(diff_by_key(&data, &duplicate, &[0], limits()).is_err());
    assert!(diff_by_key(&data, &data, &[], limits()).is_err());
    assert!(diff_by_key(&data, &data, &[0, 0], limits()).is_err());
    assert!(diff_by_key(&data, &data, &[2], limits()).is_err());
    let null = table_data(vec![vec![Cell::Null, text("x")]]);
    assert!(diff_by_key(&null, &data, &[0], limits()).is_err());
}

fn table() -> TableSpec {
    TableSpec {
        schema: "public".into(),
        name: "people".into(),
        columns: vec![
            EditColumn {
                name: "tenant".into(),
                pg_type: PgType::Text,
                nullable: false,
            },
            EditColumn {
                name: "id".into(),
                pg_type: PgType::BigInt,
                nullable: false,
            },
            EditColumn {
                name: "name".into(),
                pg_type: PgType::Text,
                nullable: true,
            },
        ],
        identity: Some(RowIdentity {
            kind: IdentityKind::PrimaryKey,
            columns: vec![0, 1],
        }),
    }
}
fn original() -> Vec<Cell> {
    vec![text("one"), Cell::Integer(42), Cell::Null]
}

#[test]
fn edit_plan_binds_values_quotes_identifiers_and_all_original_conditions() {
    let mut table = table();
    table.schema = "pub\"lic".into();
    table.name = "people; DROP TABLE users;--".into();
    table.columns[2].name = "na\"me".into();
    let malicious = "'); DROP TABLE users;--";
    let mut replacement = original();
    replacement[2] = text(malicious);
    let plan = plan_edits(
        &table,
        &[Edit::Update {
            original: original(),
            replacement,
        }],
        limits(),
    )
    .unwrap();
    let statement = &plan.statements[0];
    assert_eq!(
        statement.sql,
        "UPDATE \"pub\"\"lic\".\"people; DROP TABLE users;--\" SET \"na\"\"me\" = $1::pg_catalog.text WHERE \"tenant\" IS NOT DISTINCT FROM $2::pg_catalog.text AND \"id\" IS NOT DISTINCT FROM $3::pg_catalog.text::pg_catalog.int8 AND \"na\"\"me\" IS NOT DISTINCT FROM $4::pg_catalog.text"
    );
    assert!(!statement.sql.contains(malicious));
    assert_eq!(statement.parameters[0].value.as_deref(), Some(malicious));
    assert_eq!(statement.parameters[3].value, None);
    assert!(plan.requires_explicit_submit && plan.requires_transaction && plan.destructive);
    assert!(statement.check_affected_rows(1).is_ok());
    assert!(statement.check_affected_rows(0).is_err());
    assert!(statement.check_affected_rows(2).is_err());
}

#[test]
fn edit_plans_insert_delete_changed_identity_and_noop() {
    let mut replacement = original();
    replacement[1] = Cell::Integer(43);
    let plan = plan_edits(
        &table(),
        &[Edit::Update {
            original: original(),
            replacement,
        }],
        limits(),
    )
    .unwrap();
    assert_eq!(
        plan.statements[0].parameters[0].value.as_deref(),
        Some("43")
    );
    assert_eq!(
        plan.statements[0].parameters[2].value.as_deref(),
        Some("42")
    );
    let insert = plan_edits(&table(), &[Edit::Insert { values: original() }], limits()).unwrap();
    assert!(!insert.destructive);
    assert!(insert.statements[0].sql.starts_with("INSERT INTO"));
    let delete = plan_edits(
        &table(),
        &[Edit::Delete {
            original: original(),
        }],
        limits(),
    )
    .unwrap();
    assert!(delete.destructive);
    assert!(delete.statements[0].sql.starts_with("DELETE FROM"));
    let noop = plan_edits(
        &table(),
        &[Edit::Update {
            original: original(),
            replacement: original(),
        }],
        limits(),
    )
    .unwrap();
    assert!(noop.statements.is_empty());
}

#[test]
fn edits_reject_unverified_identity_null_keys_arity_types_and_duplicates() {
    let mut spec = table();
    spec.identity = None;
    assert!(plan_edits(&spec, &[Edit::Insert { values: original() }], limits()).is_err());
    let mut null = original();
    null[0] = Cell::Null;
    assert!(plan_edits(&table(), &[Edit::Delete { original: null }], limits()).is_err());
    let mut bad = original();
    bad[1] = text("42; DROP TABLE people");
    assert!(plan_edits(&table(), &[Edit::Insert { values: bad }], limits()).is_err());
    assert!(
        plan_edits(
            &table(),
            &[Edit::Insert {
                values: vec![text("one")]
            }],
            limits()
        )
        .is_err()
    );
    assert!(
        plan_edits(
            &table(),
            &[
                Edit::Delete {
                    original: original()
                },
                Edit::Delete {
                    original: original()
                }
            ],
            limits()
        )
        .is_err()
    );
    let mut nul = original();
    nul[2] = text("a\0b");
    assert!(plan_edits(&table(), &[Edit::Insert { values: nul }], limits()).is_err());
}

#[test]
fn edit_exact_decimal_and_binary_use_closed_typed_casts() {
    let spec = TableSpec {
        schema: "s".into(),
        name: "t".into(),
        columns: vec![
            EditColumn {
                name: "id".into(),
                pg_type: PgType::BigInt,
                nullable: false,
            },
            EditColumn {
                name: "n".into(),
                pg_type: PgType::Numeric,
                nullable: false,
            },
            EditColumn {
                name: "bytes".into(),
                pg_type: PgType::Bytea,
                nullable: false,
            },
        ],
        identity: Some(RowIdentity {
            kind: IdentityKind::UniqueConstraint,
            columns: vec![0],
        }),
    };
    let exact = "999999999999999999999.0000000000000001";
    let values = vec![
        Cell::Integer(1),
        Cell::Decimal(exact.into()),
        Cell::Binary(vec![0, 255]),
    ];
    let plan = plan_edits(&spec, &[Edit::Insert { values }], limits()).unwrap();
    assert_eq!(
        plan.statements[0].parameters[1].value.as_deref(),
        Some(exact)
    );
    assert_eq!(
        plan.statements[0].parameters[2].value.as_deref(),
        Some("\\x00ff")
    );
    assert!(
        plan.statements[0]
            .sql
            .contains("$2::pg_catalog.text::pg_catalog.numeric")
    );
}

#[test]
fn spreadsheet_escape_rejects_formula_null_marker_and_post_escape_collision() {
    let data = table_data(vec![vec![text("1"), Cell::Null]]);
    let dangerous = CsvOptions {
        null_marker: Some("=NULL".into()),
        formula_mode: FormulaMode::Escape,
        ..Default::default()
    };
    assert!(write_csv(Vec::new(), &data.columns, &data.rows, &dangerous, limits()).is_err());
    let data = table_data(vec![vec![text("1"), text("=x")]]);
    let collision = CsvOptions {
        null_marker: Some("'=x".into()),
        formula_mode: FormulaMode::Escape,
        ..Default::default()
    };
    assert!(write_csv(Vec::new(), &data.columns, &data.rows, &collision, limits()).is_err());
}

#[test]
fn mapping_never_overrides_nonnullable_contract_and_decimal_validation_is_exact() {
    let data = TabularData {
        columns: columns(&["x"]),
        rows: vec![vec![Cell::Null]],
    };
    let mapping = [ColumnMapping {
        source_index: 0,
        target_name: "x".into(),
        expected_kind: CellKind::Null,
        nullable: false,
    }];
    assert!(!preview_import(&data, &mapping, limits()).unwrap().ready());
    for invalid in [
        "",
        "-",
        "1.",
        ".1",
        "1e",
        "1e+",
        " 1",
        "1; DROP TABLE t",
        "Infinity",
    ] {
        assert!(
            Cell::Decimal(invalid.into()).validate().is_err(),
            "{invalid:?}"
        );
    }
    for valid in ["0", "-0.00", "0123.0001", "1e-9999", "123E+123"] {
        assert!(Cell::Decimal(valid.into()).validate().is_ok(), "{valid:?}");
    }
}

#[test]
fn edit_identifier_limits_count_utf8_bytes_for_schema_table_and_columns() {
    let assign_name = |spec: &mut TableSpec, position: usize, name: String| match position {
        0 => spec.schema = name,
        1 => spec.name = name,
        2 => spec.columns[0].name = name,
        _ => unreachable!(),
    };
    for position in 0..3 {
        for name in ["a".repeat(63), "가".repeat(21), "\"".repeat(63)] {
            assert_eq!(name.len(), 63);
            let mut spec = table();
            assign_name(&mut spec, position, name);
            assert!(
                plan_edits(
                    &spec,
                    &[Edit::Delete {
                        original: original()
                    }],
                    limits()
                )
                .is_ok()
            );
        }
        for name in [
            "a".repeat(64),
            format!("{}a", "가".repeat(21)),
            "가".repeat(22),
        ] {
            assert!(name.len() > 63);
            let mut spec = table();
            assign_name(&mut spec, position, name);
            let error = plan_edits(
                &spec,
                &[Edit::Delete {
                    original: original(),
                }],
                limits(),
            )
            .unwrap_err();
            assert!(error.to_string().contains("at most 63 UTF-8 bytes"));
        }
    }
}

#[test]
fn numeric_identity_normalizes_lexeme_and_text_aliases_to_reject_same_target() {
    let mut spec = table();
    spec.columns[1].pg_type = PgType::Numeric;
    for (first, second) in [
        (Cell::Decimal("1".into()), Cell::Decimal("1.00".into())),
        (Cell::Decimal("1e0".into()), Cell::Decimal("0.1e1".into())),
        (text("1"), text("1.00")),
    ] {
        let original_a = vec![text("one"), first, text("0")];
        let original_b = vec![text("one"), second, text("0")];
        let replacement_a = vec![text("one"), Cell::Decimal("1.0".into()), text("0.0")];
        let replacement_b = vec![text("one"), Cell::Decimal("1".into()), text("2")];
        let edits = [
            Edit::Update {
                original: original_a,
                replacement: replacement_a,
            },
            Edit::Update {
                original: original_b,
                replacement: replacement_b,
            },
        ];
        let error = plan_edits(&spec, &edits, limits()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("multiple edits target the same original identity")
        );
    }
    let plan = plan_edits(
        &spec,
        &[Edit::Insert {
            values: vec![text("one"), Cell::Decimal("1".into()), Cell::Null],
        }],
        limits(),
    )
    .unwrap();
    assert_eq!(plan.statements[0].parameters[1].value.as_deref(), Some("1"));
}

#[test]
fn duplicate_and_empty_result_labels_roundtrip_positionally_without_renaming() {
    for labels in [
        vec!["?column?", "?column?"],
        vec!["id", "id"],
        vec!["", ""],
        vec![""],
    ] {
        let data = TabularData {
            columns: columns(&labels),
            rows: vec![
                (0..labels.len())
                    .map(|index| text(&format!("value {index}")))
                    .collect(),
            ],
        };
        let mut csv = Vec::new();
        write_csv(
            &mut csv,
            &data.columns,
            &data.rows,
            &CsvOptions::default(),
            limits(),
        )
        .unwrap();
        assert_eq!(
            read_csv(
                csv.as_slice(),
                &CsvOptions::default(),
                None,
                limits(),
                ReadMode::Import
            )
            .unwrap()
            .data,
            data
        );
        let mut json = Vec::new();
        write_json(&mut json, &data, limits()).unwrap();
        assert_eq!(read_json(json.as_slice(), limits()).unwrap(), data);
        assert_eq!(
            diff_by_key(&data, &data, &[0], limits()).unwrap().unchanged,
            1
        );
        if labels.len() > 1 {
            let mut after = data.clone();
            after.rows[0][1] = text("changed");
            let diff = diff_by_key(&data, &after, &[0], limits()).unwrap();
            assert!(
                matches!(&diff.differences[0], RowDifference::Change { changed_columns, .. } if changed_columns == &[1])
            );
        }
    }
}

#[test]
fn duplicate_source_labels_require_explicit_unique_nonempty_mapping_targets() {
    let data = TabularData {
        columns: columns(&["id", "id"]),
        rows: vec![vec![text("one"), text("two")]],
    };
    let mapping = vec![
        ColumnMapping {
            source_index: 0,
            target_name: "left_id".into(),
            expected_kind: CellKind::Text,
            nullable: false,
        },
        ColumnMapping {
            source_index: 1,
            target_name: "right_id".into(),
            expected_kind: CellKind::Text,
            nullable: false,
        },
    ];
    assert!(preview_import(&data, &mapping, limits()).unwrap().ready());
    assert_eq!(data.columns, columns(&["id", "id"]));
    for names in [["id", "id"], ["left_id", ""]] {
        let mut ambiguous = mapping.clone();
        for (entry, name) in ambiguous.iter_mut().zip(names) {
            entry.target_name = name.into();
        }
        assert!(preview_import(&data, &ambiguous, limits()).is_err());
    }
    let mut duplicate_edit = table();
    duplicate_edit.columns[2].name = duplicate_edit.columns[0].name.clone();
    assert!(
        plan_edits(
            &duplicate_edit,
            &[Edit::Delete {
                original: original()
            }],
            limits()
        )
        .is_err()
    );
    let mut empty_edit = table();
    empty_edit.columns[2].name.clear();
    assert!(
        plan_edits(
            &empty_edit,
            &[Edit::Delete {
                original: original()
            }],
            limits()
        )
        .is_err()
    );
}
