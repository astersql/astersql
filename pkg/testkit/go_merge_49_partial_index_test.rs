// Copyright 2026 AsterSQL.

use astersql_table_tables::index::{
    BackfillState, ColumnInfo, Index, IndexColumn, IndexError, IndexInfo, SchemaState, TableInfo,
};
use astersql_table_tables::mutation_checker::Datum;

#[test]
fn go_merge_49_partial_index_uses_real_planner_compiler() {
    astersql_planner_core::InstallPlannerExpressionFactory().unwrap();
    let table = TableInfo {
        id: 7,
        columns: vec![ColumnInfo {
            id: 1,
            name: "a".to_owned(),
            needs_restored_data: false,
            field_type: astersql_parser_mysql::r#type::TypeLonglong,
            collation: String::new(),
        }],
    };
    let mut info = IndexInfo {
        id: 1,
        name: "partial".to_owned(),
        columns: vec![IndexColumn {
            name: "a".to_owned(),
            offset: 0,
            length: None,
        }],
        unique: false,
        primary: false,
        state: SchemaState::Public,
        backfill_state: BackfillState::Inapplicable,
        condition: Some("missing > 0".to_owned()),
    };
    assert!(matches!(
        Index::new(false, 7, table.clone(), info.clone()),
        Err(IndexError::Evaluation(_))
    ));
    info.condition = Some("a > 0".to_owned());
    let index = Index::new(false, 7, table, info).unwrap();
    assert!(!index.matches_partial_condition(&[Datum::Int(-1)]).unwrap());
    assert!(index.matches_partial_condition(&[Datum::Int(1)]).unwrap());
    let mut direct_condition = index.index_info.clone();
    direct_condition.condition = Some("a".to_owned());
    let direct = Index::new(false, 7, index.table_info.clone(), direct_condition).unwrap();
    assert!(!direct.matches_partial_condition(&[Datum::Int(-1)]).unwrap());
    assert!(direct.matches_partial_condition(&[Datum::Int(1)]).unwrap());
    assert!(!direct.matches_partial_condition(&[Datum::Null]).unwrap());

    let string_table = TableInfo {
        id: 8,
        columns: vec![ColumnInfo {
            id: 2,
            name: "a".to_owned(),
            needs_restored_data: false,
            field_type: astersql_parser_mysql::r#type::TypeVarString,
            collation: "utf8mb4_general_ci".to_owned(),
        }],
    };
    let mut string_info = index.index_info.clone();
    string_info.condition = Some("a = 'A'".to_owned());
    let string_index = Index::new(true, 8, string_table, string_info).unwrap();
    assert!(
        string_index
            .matches_partial_condition(&[Datum::Bytes(b"a".to_vec())])
            .unwrap()
    );
}
