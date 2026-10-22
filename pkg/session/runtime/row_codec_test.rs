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
