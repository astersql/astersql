// Copyright 2026 AsterSQL.

use crate::*;
use std::cell::RefCell;
use std::rc::Rc;

fn planner_column(id: i64, unique_id: i64) -> Column {
    Column::new(
        *expression::types::NewFieldType(mysql::r#type::TypeLonglong),
        id,
        unique_id,
        0,
    )
}

#[test]
fn build_key_info_rebuilds_keys_from_all_index_paths_and_handle() {
    let mut primary = model::ColumnInfo::New(1, parser_ast::NewCIStr("id"));
    primary.AddFlag(mysql::r#type::PriKeyFlag | mysql::r#type::NotNullFlag);
    let mut email = model::ColumnInfo::New(2, parser_ast::NewCIStr("email"));
    email.AddFlag(mysql::r#type::NotNullFlag);
    email.Offset = 1;
    let unique_index = model::IndexInfo {
        ID: 7,
        Unique: true,
        Columns: vec![model::IndexColumn {
            Name: parser_ast::NewCIStr("email"),
            Offset: 1,
            Length: expression::types::UnspecifiedLength as isize,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut source = DataSource::default();
    source.TableInfo.PKIsHandle = true;
    source.Columns = vec![primary.clone(), email.clone()];
    source.AllPossibleAccessPaths = vec![
        planner_util::AccessPath {
            IsIntHandlePath: true,
            ..Default::default()
        },
        planner_util::AccessPath {
            Index: Some(unique_index),
            ..Default::default()
        },
    ];

    let mut scan = LogicalIndexScan {
        Source: Some(Rc::new(RefCell::new(source))),
        Columns: vec![primary, email],
        ..Default::default()
    };
    scan.SetSchema(expression::NewSchema(vec![
        planner_column(1, 101),
        planner_column(2, 102),
    ]));

    scan.BuildKeyInfo();

    assert_eq!(scan.Schema().PKOrUK.len(), 2);
    assert!(scan.Schema().PKOrUK.iter().any(|key| key[0].ID == 1));
    assert!(scan.Schema().PKOrUK.iter().any(|key| key[0].ID == 2));
}
