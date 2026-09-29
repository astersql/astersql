// Copyright 2026 AsterSQL.

use crate::*;

fn table(name: &str) -> TableName {
    TableName {
        Name: CIStr {
            O: name.into(),
            L: name.to_lowercase(),
        },
        ..TableName::default()
    }
}

fn expression(text: &str) -> ExprNode {
    let mut expr = ExprNode::default();
    expr.SetText(None, text.as_bytes());
    expr
}

fn now_function() -> ExprNode {
    ExprNode {
        Kind: ExprKind::Function {
            Schema: CIStr::default(),
            FnName: CIStr {
                O: "now".into(),
                L: "now".into(),
            },
            Args: vec![],
        },
        ..ExprNode::default()
    }
}

#[test]
fn go_merge_7_materialized_view_restore() {
    let mut select = SelectStmt::default();
    select.SetText(None, b"SELECT 1");
    let stmt = CreateMaterializedViewStmt {
        node_text: base::AstNode::default(),
        ViewName: Some(table("mv")),
        Cols: vec![CIStr {
            O: "c".into(),
            L: "c".into(),
        }],
        Comment: "sample".into(),
        Refresh: Some(MViewRefreshClause {
            Method: MViewRefreshMethod::Fast,
            StartWith: Some(expression("NOW()")),
            Next: Some(expression("INTERVAL 1 HOUR")),
        }),
        Attributes: "attr".into(),
        Options: vec![
            TableOption {
                Tp: TableOptionType::ShardRowID,
                UintValue: 2,
                ..TableOption::default()
            },
            TableOption {
                Tp: TableOptionType::PreSplitRegion,
                UintValue: 3,
                ..TableOption::default()
            },
            TableOption {
                Tp: TableOptionType::StorageClass,
                StrValue: "hot".into(),
                ..TableOption::default()
            },
        ],
        Select: Some(Box::new(select)),
    };
    assert_eq!(
        stmt.restore().unwrap(),
        "CREATE MATERIALIZED VIEW `mv` (`c`) COMMENT = 'sample' SHARD_ROW_ID_BITS = 2 PRE_SPLIT_REGIONS = 3 STORAGE_CLASS = 'hot' REFRESH FAST START WITH NOW() NEXT INTERVAL 1 HOUR ATTRIBUTES = 'attr' AS SELECT 1"
    );
}

#[test]
fn go_merge_7_materialized_log_and_actions_restore() {
    assert_eq!(MViewRefreshMethod::Fast.to_string(), "REFRESH FAST");
    assert_eq!(MViewRefreshMethod::Unknown(9).to_string(), "UNKNOWN");
    assert_eq!(
        MViewRefreshClause {
            Method: MViewRefreshMethod::Unknown(9),
            ..MViewRefreshClause::default()
        }
        .restore()
        .unwrap(),
        "UNKNOWN"
    );
    let log = CreateMaterializedViewLogStmt {
        Table: Some(table("base")),
        Cols: vec![CIStr {
            O: "id".into(),
            L: "id".into(),
        }],
        Purge: Some(MLogPurgeClause {
            Immediate: true,
            ..MLogPurgeClause::default()
        }),
        AccumulationAlert: Some(MLogAccumulationAlertClause { Rows: 12 }),
        ..CreateMaterializedViewLogStmt::default()
    };
    assert_eq!(
        log.restore().unwrap(),
        "CREATE MATERIALIZED VIEW LOG ON `base` (`id`) PURGE IMMEDIATE ALERT ROWS 12"
    );
    let alter = AlterMaterializedViewStmt {
        ViewName: Some(table("mv")),
        Actions: vec![AlterMaterializedViewAction {
            Tp: AlterMaterializedViewActionType::Comment,
            Comment: "new".into(),
            ..AlterMaterializedViewAction::default()
        }],
        ..AlterMaterializedViewStmt::default()
    };
    assert_eq!(
        alter.restore().unwrap(),
        "ALTER MATERIALIZED VIEW `mv` COMMENT = 'new'"
    );
    let drop_view = DropMaterializedViewStmt {
        IfExists: true,
        ViewName: Some(table("mv")),
        ..DropMaterializedViewStmt::default()
    };
    assert_eq!(
        drop_view.restore().unwrap(),
        "DROP MATERIALIZED VIEW IF EXISTS `mv`"
    );
    let drop_log = DropMaterializedViewLogStmt {
        IfExists: true,
        Table: Some(table("base")),
        ..DropMaterializedViewLogStmt::default()
    };
    assert_eq!(
        drop_log.restore().unwrap(),
        "DROP MATERIALIZED VIEW LOG IF EXISTS ON `base`"
    );
}

#[test]
fn go_merge_7_materialized_action_branches_and_errors() {
    let refresh = AlterMaterializedViewAction {
        Tp: AlterMaterializedViewActionType::Refresh,
        Refresh: Some(MViewRefreshClause {
            Next: Some(now_function()),
            ..MViewRefreshClause::default()
        }),
        ..AlterMaterializedViewAction::default()
    };
    assert_eq!(refresh.restore().unwrap(), "REFRESH NEXT NOW()");
    let attributes = AlterMaterializedViewAction {
        Tp: AlterMaterializedViewActionType::Attributes,
        Attributes: "a".into(),
        ..AlterMaterializedViewAction::default()
    };
    assert_eq!(attributes.restore().unwrap(), "ATTRIBUTES = 'a'");
    let alter_log = AlterMaterializedViewLogStmt {
        Table: Some(table("base")),
        Actions: vec![
            AlterMaterializedViewLogAction {
                Tp: AlterMaterializedViewLogActionType::Purge,
                Purge: Some(MLogPurgeClause {
                    Next: Some(expression("NOW()")),
                    ..MLogPurgeClause::default()
                }),
                ..AlterMaterializedViewLogAction::default()
            },
            AlterMaterializedViewLogAction {
                Tp: AlterMaterializedViewLogActionType::AddColumn,
                Cols: vec![CIStr {
                    O: "c".into(),
                    L: "c".into(),
                }],
                ..AlterMaterializedViewLogAction::default()
            },
        ],
        ..AlterMaterializedViewLogStmt::default()
    };
    assert_eq!(
        alter_log.restore().unwrap(),
        "ALTER MATERIALIZED VIEW LOG ON `base` PURGE NEXT NOW(), ADD COLUMN (`c`)"
    );
    let missing_select = CreateMaterializedViewStmt {
        node_text: base::AstNode::default(),
        ViewName: Some(table("mv")),
        Cols: vec![],
        Comment: String::new(),
        Refresh: None,
        Attributes: String::new(),
        Options: vec![],
        Select: None,
    };
    assert!(
        missing_select
            .restore()
            .unwrap_err()
            .contains("Select is missing")
    );
}

#[test]
fn go_merge_7_materialized_view_restores_typed_select_without_source_text() {
    let mut select = SelectStmt::default();
    select.Fields.Fields.push(SelectField {
        Expr: Some(NewValueExpr(1_i64, "utf8mb4", "utf8mb4_bin")),
        ..SelectField::default()
    });
    let stmt = CreateMaterializedViewStmt {
        node_text: base::AstNode::default(),
        ViewName: Some(table("mv")),
        Cols: vec![],
        Comment: String::new(),
        Refresh: None,
        Attributes: String::new(),
        Options: vec![],
        Select: Some(Box::new(select)),
    };
    assert_eq!(
        stmt.restore().unwrap(),
        "CREATE MATERIALIZED VIEW `mv` () AS SELECT 1"
    );
}

#[test]
fn go_merge_7_materialized_view_restores_complex_typed_select_without_source_text() {
    fn column() -> ExprNode {
        ExprNode::Column(ColumnName {
            Name: NewCIStr("a"),
            ..ColumnName::default()
        })
    }
    fn value(number: i64) -> ExprNode {
        NewValueExpr(number, "utf8mb4", "utf8mb4_bin")
    }
    let mut select = SelectStmt::default();
    select.Fields.Fields.push(SelectField {
        Expr: Some(ExprNode::Binary(
            "+".into(),
            Box::new(column()),
            Box::new(value(1)),
        )),
        AsName: NewCIStr("total"),
        ..SelectField::default()
    });
    select.From = Some(TableRefsClause {
        TableRefs: Join {
            Left: Some(Box::new(ResultSetNode::TableSource(TableSource {
                Source: table("t"),
                ..TableSource::default()
            }))),
            ..Join::default()
        },
    });
    select.Where = Some(ExprNode::Binary(
        ">".into(),
        Box::new(column()),
        Box::new(value(2)),
    ));
    select.GroupBy.push(ByItem {
        Expr: column(),
        Desc: false,
    });
    select.OrderBy.push(ByItem {
        Expr: column(),
        Desc: true,
    });
    select.Limit = Some(Limit {
        Count: Some(value(5)),
        Offset: None,
    });
    let stmt = CreateMaterializedViewStmt {
        node_text: base::AstNode::default(),
        ViewName: Some(table("mv")),
        Cols: vec![NewCIStr("total")],
        Comment: String::new(),
        Refresh: None,
        Attributes: String::new(),
        Options: vec![],
        Select: Some(Box::new(select)),
    };
    assert_eq!(
        stmt.restore().unwrap(),
        "CREATE MATERIALIZED VIEW `mv` (`total`) AS SELECT `a`+1 AS `total` FROM `t` WHERE `a`>2 GROUP BY `a` ORDER BY `a` DESC LIMIT 5"
    );
}

#[test]
fn go_merge_7_structured_expression_restore_without_source_text() {
    let column = || {
        ExprNode::Column(ColumnName {
            Name: NewCIStr("a"),
            ..ColumnName::default()
        })
    };
    let number = |n| NewValueExpr(n, "utf8mb4", "utf8mb4_bin");
    let between = ExprNode {
        Kind: ExprKind::Between {
            Expr: Box::new(column()),
            Left: Box::new(number(1)),
            Right: Box::new(number(10)),
            Not: false,
        },
        ..ExprNode::default()
    };
    assert_eq!(
        sql_restore::restore_expr(&between).unwrap(),
        "`a` BETWEEN 1 AND 10"
    );
    let case = ExprNode {
        Kind: ExprKind::Case {
            Value: None,
            WhenClauses: vec![WhenClause {
                Expr: between,
                Result: number(1),
            }],
            ElseClause: Some(Box::new(number(0))),
        },
        ..ExprNode::default()
    };
    assert_eq!(
        sql_restore::restore_expr(&case).unwrap(),
        "CASE WHEN `a` BETWEEN 1 AND 10 THEN 1 ELSE 0 END"
    );
    let like = ExprNode {
        Kind: ExprKind::Like {
            Expr: Box::new(column()),
            Pattern: Box::new(NewValueExpr("x%", "utf8mb4", "utf8mb4_bin")),
            Not: true,
            Escape: String::new(),
            Explicit: false,
            IsLike: true,
            Type: parser_types::types::FieldType::default(),
        },
        ..ExprNode::default()
    };
    assert_eq!(
        sql_restore::restore_expr(&like).unwrap(),
        "`a` NOT LIKE _UTF8MB4'x%'"
    );
}

#[test]
fn go_merge_7_structured_cte_and_union_restore_without_source_text() {
    fn select_number(number: i64) -> SelectStmt {
        let mut select = SelectStmt::default();
        select.Fields.Fields.push(SelectField {
            Expr: Some(NewValueExpr(number, "utf8mb4", "utf8mb4_bin")),
            ..SelectField::default()
        });
        select
    }
    let mut with_select = select_number(2);
    with_select.With = Some(
        WithClause {
            CTEs: vec![CommonTableExpression {
                Name: NewCIStr("x"),
                ColNameList: vec![],
                Query: Box::new(select_number(1)),
                IsRecursive: false,
            }],
            ..WithClause::default()
        }
        .into_shared(),
    );
    assert_eq!(
        sql_restore::restore_node(&with_select).unwrap(),
        "WITH `x` AS (SELECT 1) SELECT 2"
    );
    let mut list =
        SetOprSelectList::new(vec![Box::new(select_number(1)), Box::new(select_number(2))]);
    list.operators[1] = Some(SetOprType::UnionAll);
    let union = SetOprStmt::new(list);
    assert_eq!(
        sql_restore::restore_node(&union).unwrap(),
        "SELECT 1 UNION ALL SELECT 2"
    );
}

#[test]
fn go_merge_7_nested_expression_restore_without_source_text() {
    let mut query = SelectStmt::default();
    query.Fields.Fields.push(SelectField {
        Expr: Some(NewValueExpr(1_i64, "utf8mb4", "utf8mb4_bin")),
        ..SelectField::default()
    });
    let subquery = ExprNode {
        Kind: ExprKind::Subquery {
            Query: NodeRef::new(Box::new(query)),
            MultiRows: false,
            Exists: false,
        },
        ..ExprNode::default()
    };
    assert_eq!(sql_restore::restore_expr(&subquery).unwrap(), "(SELECT 1)");
    let compare = ExprNode {
        Kind: ExprKind::CompareSubquery {
            Op: "=".into(),
            L: Box::new(NewValueExpr(1_i64, "utf8mb4", "utf8mb4_bin")),
            R: Box::new(subquery),
            All: true,
        },
        ..ExprNode::default()
    };
    assert_eq!(
        sql_restore::restore_expr(&compare).unwrap(),
        "1 = ALL (SELECT 1)"
    );
    let binary_cast = ExprNode {
        Kind: ExprKind::Cast {
            Expr: Box::new(NewValueExpr(1_i64, "utf8mb4", "utf8mb4_bin")),
            Tp: parser_types::types::FieldType::default(),
            FunctionType: CastFunctionType::Binary,
            ExplicitCharSet: false,
        },
        ..ExprNode::default()
    };
    assert_eq!(sql_restore::restore_expr(&binary_cast).unwrap(), "BINARY 1");
    let match_against = ExprNode {
        Kind: ExprKind::MatchAgainst {
            ColumnNames: vec![ColumnName {
                Name: NewCIStr("a"),
                ..ColumnName::default()
            }],
            Against: Box::new(NewValueExpr("needle", "utf8mb4", "utf8mb4_bin")),
            Modifier: 0x11,
        },
        ..ExprNode::default()
    };
    assert!(
        sql_restore::restore_expr(&match_against)
            .unwrap_err()
            .contains("BOOLEAN MODE")
    );
}
