// Copyright 2026 AsterSQL.

use astersql_meta_model as model;
use astersql_types as types;
use astersql_util_regionsplit::{
    BuildModelHandleColsForSplit, ConvertValueToColumnType, GetHandleColumnInfos,
    GetSplitTableKeysForModel, StatementContext,
};

#[test]
fn go_merge_32_handle_columns_follow_go_pk_rules() {
    let mut table = model::TableInfo::default();
    assert_eq!(GetHandleColumnInfos(&table)[0].ID, model::ExtraHandleID);

    table.PKIsHandle = true;
    assert!(GetHandleColumnInfos(&table).is_empty());
    let mut pk = model::ColumnInfo::default();
    pk.ID = 42;
    pk.SetFlag(astersql_parser_mysql::r#type::PriKeyFlag);
    table.Columns.push(pk);
    assert_eq!(GetHandleColumnInfos(&table)[0].ID, 42);

    table.PKIsHandle = false;
    table.IsCommonHandle = true;
    assert!(GetHandleColumnInfos(&table).is_empty());
    table.Indices.push(model::IndexInfo {
        Primary: true,
        Columns: vec![model::IndexColumn {
            Offset: 0,
            ..Default::default()
        }],
        ..Default::default()
    });
    assert_eq!(GetHandleColumnInfos(&table)[0].ID, 42);
}

#[test]
fn go_merge_32_common_handle_truncates_prefix_without_mutating_input() {
    let mut table = model::TableInfo::default();
    table.IsCommonHandle = true;
    let mut column = model::ColumnInfo::default();
    column.SetCharset("utf8mb4".to_owned());
    table.Columns.push(column);
    table.Indices.push(model::IndexInfo {
        Primary: true,
        Columns: vec![model::IndexColumn {
            Offset: 0,
            Length: 3,
            ..Default::default()
        }],
        ..Default::default()
    });
    let row = vec![types::datum::NewStringDatum("abcdef".to_owned())];
    let handle = BuildModelHandleColsForSplit(&table)
        .BuildHandleByDatums(&row)
        .expect("valid common handle");
    let expected = astersql_util_codec::EncodeKey(
        astersql_util_codec::time::UTC,
        Vec::new(),
        vec![types::datum::NewStringDatum("abc".to_owned())],
    )
    .unwrap();
    assert_eq!(handle, expected);
    assert_eq!(row[0].GetString(), "abcdef");

    let keys = GetSplitTableKeysForModel(
        &StatementContext::default(),
        &table,
        8,
        &row,
        &[types::datum::NewStringDatum("xyzuvw".to_owned())],
        2,
        Vec::new(),
    )
    .expect("split common handle bounds");
    assert_eq!(keys.len(), 1);
    assert!(keys[0].starts_with(b"t"));
}

#[test]
fn go_merge_32_conversion_normalizes_bad_number_with_column_name() {
    let mut column = model::ColumnInfo::default();
    column.FieldType = *types::datum::NewFieldType(types::metadata::mysql::TypeLonglong);
    let value = types::datum::NewStringDatum("bad-number".to_owned());
    let error = match ConvertValueToColumnType(
        &value,
        &column,
        (*types::DefaultStmtNoWarningContext).clone(),
    ) {
        Ok(_) => panic!("invalid integer must fail"),
        Err(error) => error,
    };
    assert!(error.Equal(&types::errors::ErrTruncated), "{error}");
    assert!(
        error
            .to_string()
            .contains("Incorrect value: 'bad-number' for column")
    );
}
