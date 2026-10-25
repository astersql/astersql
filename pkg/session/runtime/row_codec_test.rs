// Copyright 2026 AsterSQL.

use super::row_codec::insert_default_runtime_value;

#[test]
fn timestamp_default_accepts_ast_restored_fractional_precision() {
    for expression in [
        "CURRENT_TIMESTAMP(6)",
        "CURRENT_TIMESTAMP('6')",
        "CURRENT_TIMESTAMP('6').000000",
    ] {
        let mut column = astersql_meta_model::ColumnInfo::default();
        column.SetType(astersql_parser_mysql::r#type::TypeTimestamp);
        column.SetDecimal(6);
        column
            .SetDefaultValue(Some(astersql_meta_model::DefaultValue::String(
                expression.as_bytes().to_vec(),
            )))
            .unwrap();
        let value = insert_default_runtime_value(&column).unwrap();
        assert_eq!(value.len(), 26);
        assert!(chrono::NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S%.6f").is_ok());
    }
}

#[test]
fn uuid_expression_default_is_evaluated_per_row() {
    let mut column = astersql_meta_model::ColumnInfo::default();
    column
        .SetDefaultValue(Some(astersql_meta_model::DefaultValue::String(
            b"UUID()".to_vec(),
        )))
        .unwrap();
    column.DefaultIsExpr = true;

    let first = insert_default_runtime_value(&column).unwrap();
    let second = insert_default_runtime_value(&column).unwrap();

    assert_eq!(uuid::Uuid::parse_str(&first).unwrap().get_version_num(), 4);
    assert_eq!(uuid::Uuid::parse_str(&second).unwrap().get_version_num(), 4);
    assert_ne!(first, second);
}

#[test]
fn decimal_index_cursor_preserves_declared_precision_and_scale() {
    let mut column = astersql_meta_model::ColumnInfo::default();
    column.SetType(astersql_parser_mysql::r#type::TypeNewDecimal);
    column.SetDecimal(2);
    for (precision, expected) in [
        (10, vec![6, 10, 2, 128, 0, 0, 12, 30]),
        (5, vec![6, 5, 2, 128, 12, 30]),
    ] {
        column.SetFlen(precision);
        for input in ["12.30", "12.3"] {
            let datum = super::row_codec::runtime_value_to_datum(
                Some(&input.to_owned()),
                &column,
                astersql_types::Flags::default(),
            )
            .unwrap();
            let key = astersql_tablecodec::codec::NewEncoder(false)
                .EncodeKey(astersql_tablecodec::time::UTC, Vec::new(), vec![datum])
                .unwrap();
            // Match the declared DECIMAL field metadata used by Go's row
            // decoder and MODIFY COLUMN backfill, independently of spelling.
            assert_eq!(key, expected, "DECIMAL({precision},2): {input}");
        }
    }
}

#[test]
fn reorg_global_index_copy_mutations_stop_at_delete_only() {
    use astersql_meta_model::{
        ColumnInfo, IndexColumn, IndexInfo, PartitionInfo, SchemaState as S, TableInfo,
    };
    let mut column = ColumnInfo::default();
    column.ID = 1;
    column.Name = astersql_parser_ast::NewCIStr("b");
    column.SetType(astersql_parser_mysql::r#type::TypeLonglong);
    let row = std::collections::HashMap::from([
        ("b".to_owned(), Some("10".to_owned())),
        ("_tidb_rowid".to_owned(), Some("1".to_owned())),
    ]);
    for unique in [false, true] {
        for version in [0, astersql_meta_model::GlobalIndexVersionV1] {
            for state in [S::WriteOnly, S::DeleteOnly] {
                let mut pi = PartitionInfo::default();
                pi.DDLChangedIndex.insert(7, true);
                let table = TableInfo {
                    ID: 100,
                    Columns: vec![column.clone()],
                    Partition: Some(pi),
                    Indices: vec![IndexInfo {
                        ID: 7,
                        Name: astersql_parser_ast::NewCIStr("same_name"),
                        Global: true,
                        Unique: unique,
                        GlobalIndexVersion: version,
                        State: state,
                        Columns: vec![IndexColumn {
                            Name: column.Name.clone(),
                            Length: -1,
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                };
                let inserted = super::row_codec::relational_index_mutations(
                    &table,
                    None,
                    Some(&row),
                    astersql_types::Flags::default(),
                )
                .unwrap();
                assert_eq!(inserted.len(), usize::from(state == S::WriteOnly));
                let deleted = super::row_codec::relational_index_mutations(
                    &table,
                    Some(&row),
                    None,
                    astersql_types::Flags::default(),
                )
                .unwrap();
                assert_eq!(deleted.len(), 1);
                assert!(deleted[0].1.is_none());
            }
        }
    }
}

#[test]
fn pg_scalar_15_numeric_row_formats_and_legacy_metadata() {
    use astersql_meta_model::{ColumnInfo, SchemaState, TableInfo};
    use std::collections::HashMap;
    for pg_numeric in [false, true] {
        for new_format in [false, true] {
            let mut column = ColumnInfo::default();
            column.ID = 1;
            column.Name = astersql_parser_ast::NewCIStr("n");
            column.State = SchemaState::Public;
            column.SetType(astersql_parser_mysql::r#type::TypeNewDecimal);
            let mut table = TableInfo {
                ID: 9815,
                ExactName: true,
                PostgreSQLNumericText: pg_numeric,
                Columns: vec![column],
                ..Default::default()
            };
            let encoded_metadata = serde_json::to_vec(&table).unwrap();
            table = serde_json::from_slice(&encoded_metadata).unwrap();
            assert_eq!(table.PostgreSQLNumericText, pg_numeric);
            if !pg_numeric {
                assert!(
                    !String::from_utf8(encoded_metadata)
                        .unwrap()
                        .contains("postgresql_numeric_text")
                );
            }
            for (precision, scale, input, expected) in [
                (5, 2, Some("12.345".to_owned()), Some("12.35".to_owned())),
                (2, -3, Some("12500".to_owned()), Some("13000".to_owned())),
                (
                    3,
                    5,
                    Some("0.001234".to_owned()),
                    Some("0.00123".to_owned()),
                ),
                (-1, -1, Some("9".repeat(1000)), Some("9".repeat(1000))),
                (-1, -1, Some("NaN".to_owned()), Some("NaN".to_owned())),
                (
                    -1,
                    -1,
                    Some("Infinity".to_owned()),
                    Some("Infinity".to_owned()),
                ),
                (5, 2, None, None),
            ] {
                if !pg_numeric && (precision != 5 || scale != 2) {
                    continue;
                }
                table.Columns[0].SetFlen(precision);
                table.Columns[0].SetDecimal(scale);
                let row = HashMap::from([
                    ("n".to_owned(), input),
                    ("_tidb_rowid".to_owned(), Some("1".to_owned())),
                ]);
                let (_, bytes) = super::row_codec::encode_relational_row_with_format(
                    &table,
                    &row,
                    astersql_types::Flags::default(),
                    new_format,
                )
                .unwrap();
                let fields = table
                    .Columns
                    .iter()
                    .map(|column| (column.ID, Box::new(column.FieldType.clone())))
                    .collect();
                let handle = astersql_tablecodec::kv::IntHandle(1);
                let (_, decoded) = super::relational_scan::decode_relational_row_value(
                    &table, &fields, &handle, &bytes,
                )
                .unwrap_or_else(|error| panic!("PG={pg_numeric} new_format={new_format} precision={precision} scale={scale}: {error}"));
                assert_eq!(
                    decoded.get("n").cloned().flatten(),
                    expected,
                    "PG={pg_numeric} new_format={new_format} precision={precision} scale={scale}"
                );
            }
        }
    }
}
