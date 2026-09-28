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
