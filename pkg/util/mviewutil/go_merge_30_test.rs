// Copyright 2026 AsterSQL.

use astersql_meta_model::{
    ColumnInfo, IndexColumn, IndexInfo, StateDeleteOnly, StatePublic, TableInfo,
};
use astersql_parser_ast as ast;
use astersql_parser_mysql as mysql;

#[test]
fn go_merge_30_visible_index_prefix_covers_grouping_columns_in_any_order() {
    assert!(!crate::HasIndexWithPrefixCoveringColumns(
        None,
        &["a"],
        "",
        true
    ));
    let mut table = TableInfo::default();
    table.Indices.push(IndexInfo {
        Name: ast::NewCIStr("idx_ab"),
        State: StatePublic,
        Columns: vec![
            IndexColumn {
                Name: ast::NewCIStr("a"),
                Length: -1,
                ..Default::default()
            },
            IndexColumn {
                Name: ast::NewCIStr("b"),
                Length: -1,
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    assert!(crate::HasIndexWithPrefixCoveringColumns(
        Some(&table),
        &["B", "A"],
        "",
        true
    ));
    assert_eq!(
        crate::FindVisibleIndexWithPrefixCoveringColumns(Some(&table), &["b", "a"]),
        ("idx_ab".into(), true)
    );
    assert_eq!(
        crate::FindVisibleIndexesWithPrefixCoveringColumns(Some(&table), &[]),
        Vec::<String>::new()
    );
    assert!(!crate::HasIndexWithPrefixCoveringColumns(
        Some(&table),
        &["a", "c"],
        "",
        true
    ));
    assert!(!crate::HasIndexWithPrefixCoveringColumns(
        Some(&table),
        &["a", "b"],
        "idx_ab",
        true
    ));
    table.Indices[0].Invisible = true;
    assert!(!crate::HasIndexWithPrefixCoveringColumns(
        Some(&table),
        &["a", "b"],
        "",
        true
    ));
    assert!(crate::HasIndexWithPrefixCoveringColumns(
        Some(&table),
        &["a", "b"],
        "",
        false
    ));
    table.Indices[0].Invisible = false;
    table.Indices[0].State = StateDeleteOnly;
    assert!(!crate::HasIndexWithPrefixCoveringColumns(
        Some(&table),
        &["a", "b"],
        "",
        true
    ));
    table.Indices[0].State = StatePublic;
    table.Indices[0].Columns[0].Length = 3;
    assert!(!crate::HasIndexWithPrefixCoveringColumns(
        Some(&table),
        &["a", "b"],
        "",
        false
    ));

    table.PKIsHandle = true;
    let mut primary = ColumnInfo::default();
    primary.Name = ast::NewCIStr("pk");
    primary.SetFlag(mysql::r#type::PriKeyFlag);
    table.Columns.push(primary);
    assert_eq!(
        crate::FindVisibleIndexWithPrefixCoveringColumns(Some(&table), &["PK"]),
        ("PRIMARY".into(), true)
    );
    assert!(!crate::HasIndexWithPrefixCoveringColumns(
        Some(&table),
        &["pk"],
        "primary",
        true
    ));
}

#[test]
fn go_merge_30_select_rejects_unsupported_clauses() {
    let mut cte = ast::SelectStmt::default();
    cte.With = Some(ast::WithClause::default().into_shared());
    assert!(crate::CheckMaterializedViewSelect(&cte).is_err());

    let locked = ast::SelectStmt::with_lock(ast::SelectLockType::ForUpdate);
    assert!(crate::CheckMaterializedViewSelect(&locked).is_err());

    let mut into = ast::SelectStmt::default();
    into.SelectIntoOpt = Some(ast::SelectIntoOption::default());
    assert!(crate::CheckMaterializedViewSelect(&into).is_err());

    let mut time_travel = ast::SelectStmt::default();
    time_travel.From = Some(ast::TableRefsClause {
        TableRefs: ast::Join {
            Left: Some(Box::new(ast::ResultSetNode::TableSource(
                ast::TableSource {
                    AsOf: Some(ast::AsOfClause::default()),
                    ..Default::default()
                },
            ))),
            ..Default::default()
        },
    });
    assert!(crate::CheckMaterializedViewSelect(&time_travel).is_err());

    let mut sample = ast::SelectStmt::default();
    sample.From = Some(ast::TableRefsClause {
        TableRefs: ast::Join {
            Left: Some(Box::new(ast::ResultSetNode::TableSource(
                ast::TableSource {
                    TableSample: Some(ast::TableSample::default()),
                    ..Default::default()
                },
            ))),
            ..Default::default()
        },
    });
    assert!(crate::CheckMaterializedViewSelect(&sample).is_err());

    let mut join = sample;
    join.From.as_mut().unwrap().TableRefs.Right = Some(Box::new(ast::ResultSetNode::TableSource(
        ast::TableSource::default(),
    )));
    assert!(crate::CheckMaterializedViewSelect(&join).is_ok());
}
