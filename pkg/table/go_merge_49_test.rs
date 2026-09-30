// Copyright 2026 AsterSQL.

use crate::mview_log::{
    MLogDMLType, MLogSourceStmt, classify_add_record, project_log_row, should_log_update,
    validate_meta,
};
use model_dependency::group_4 as model;

fn column(name: &str, offset: isize) -> model::ColumnInfo {
    model::ColumnInfo {
        Name: model::ast::NewCIStr(name),
        Offset: offset,
        State: model::StatePublic,
        ..Default::default()
    }
}

#[test]
fn go_merge_49_classifies_insert_update_and_replace_paths() {
    assert_eq!(
        classify_add_record(MLogSourceStmt::Insert, false, false),
        MLogDMLType::Insert
    );
    assert_eq!(
        classify_add_record(MLogSourceStmt::Insert, true, false),
        MLogDMLType::Update
    );
    assert_eq!(
        classify_add_record(MLogSourceStmt::Replace, false, true),
        MLogDMLType::Update
    );
    assert_eq!(
        classify_add_record(MLogSourceStmt::Update, false, false),
        MLogDMLType::Update
    );
}

#[test]
fn go_merge_49_validates_log_metadata_and_tracked_offsets() {
    let base = model::TableInfo {
        ID: 10,
        MaterializedViewBase: Some(model::MaterializedViewBaseInfo {
            MLogID: 20,
            ..Default::default()
        }),
        Columns: vec![column("untracked", 0), column("tracked", 1)],
        ..Default::default()
    };
    let mut log = model::TableInfo {
        ID: 20,
        MaterializedViewLog: Some(model::MaterializedViewLogInfo {
            BaseTableID: 10,
            Columns: vec![model::ast::NewCIStr("tracked")],
            ..Default::default()
        }),
        Columns: vec![
            column("tracked", 0),
            column(model::MaterializedViewLogDMLTypeColumnName, 1),
            column(model::MaterializedViewLogOldNewColumnName, 2),
        ],
        ..Default::default()
    };
    assert_eq!(validate_meta(&base, &log).unwrap(), vec![1]);
    log.Columns.swap(0, 1);
    assert!(validate_meta(&base, &log).is_err());
    log.Columns.swap(0, 1);
    log.MaterializedViewLog.as_mut().unwrap().BaseTableID = 99;
    assert!(validate_meta(&base, &log).is_err());
}

#[test]
fn go_merge_49_rejects_embed_text_in_check_constraint() {
    use parser_ast_dependency as ast;
    let constraint = ast::Constraint {
        Name: "ck_embed".to_owned(),
        Tp: ast::ConstraintType::Check,
        Expr: Some(ast::ExprNode::Function(
            ast::NewCIStr(""),
            ast::NewCIStr("embed_text"),
            vec![ast::ExprNode::Value("sample".to_owned())],
        )),
        ..Default::default()
    };
    let (supported, reason) = crate::IsSupportedExpr(&constraint);
    assert!(!supported);
    assert!(reason.is_some());
}

#[test]
fn go_merge_49_projects_old_and_new_log_rows_and_skips_untouched_updates() {
    use types_dependency::datum;
    let row = vec![
        datum::NewIntDatum(3),
        datum::NewStringDatum("tracked".to_owned()),
    ];
    let old = project_log_row(&[1], &row, MLogDMLType::Update, -1).unwrap();
    let new = project_log_row(&[1], &row, MLogDMLType::Update, 1).unwrap();
    assert_eq!(old[0].GetString(), "tracked");
    assert_eq!(old[1].GetString(), "U");
    assert_eq!(old[2].GetInt64(), -1);
    assert_eq!(new[2].GetInt64(), 1);
    assert!(!should_log_update(&[1], &[true, false]));
    assert!(should_log_update(&[1], &[false, true]));
    assert!(should_log_update(&[2], &[false, true]));
    assert!(project_log_row(&[2], &row, MLogDMLType::Insert, 1).is_err());
}
