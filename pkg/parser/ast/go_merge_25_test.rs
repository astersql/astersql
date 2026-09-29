// Copyright 2026 AsterSQL.

use crate::{
    AlterTableSpec, AlterTableStmt, AsOfClause, ColumnName, ColumnPosition, CreateBindingStmt,
    CreateIndexStmt, CreateTableStmt, DoStmt, DropStatsStmt, DropTableStmt, ExprNode, FieldList,
    FlashBackToTimestampStmt, InPlaceVisitor, InsertStmt, Join, KillStmt, Node, PlanReplayerStmt,
    ProcedureBlock, ProcedureDecl, ProcedureElseBlock, ReferenceDef, ResultSetNode, SelectField,
    SelectStmt, SetOprSelectList, StringOrUserVar, TableOption, TableRefsClause, TableSource,
    TimeUnitType, Walk, WithClause,
};

#[test]
fn go_merge_25_reference_visits_table_before_index_columns() {
    struct Trace(Vec<&'static str>);

    impl InPlaceVisitor for Trace {
        fn enter(&mut self, node: &mut dyn Node) -> bool {
            if node.as_any().is::<CreateTableStmt>() {
                self.0.push("create");
            }
            false
        }

        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }

        fn enter_table_name(&mut self, _table: &mut crate::TableName) -> bool {
            self.0.push("table");
            false
        }

        fn enter_embedded(&mut self, input: &mut dyn std::any::Any) -> bool {
            if input.is::<ReferenceDef>() {
                self.0.push("reference");
            }
            false
        }

        fn enter_column_name(&mut self, _column: &mut ColumnName) -> bool {
            self.0.push("column");
            false
        }

        fn enter_on_delete(&mut self, _option: &mut crate::OnDeleteOpt) -> bool {
            self.0.push("on delete");
            false
        }

        fn enter_on_update(&mut self, _option: &mut crate::OnUpdateOpt) -> bool {
            self.0.push("on update");
            false
        }
    }

    let mut statement = CreateTableStmt {
        Constraints: vec![crate::Constraint {
            Refer: Some(ReferenceDef {
                IndexPartSpecifications: vec![crate::IndexPartSpecification {
                    Column: Some(ColumnName::default()),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut trace = Trace(Vec::new());
    assert!(Walk(&mut statement, &mut trace));
    assert_eq!(
        trace.0,
        [
            "create",
            "table",
            "reference",
            "table",
            "column",
            "on delete",
            "on update"
        ]
    );
}

#[test]
fn go_merge_25_drop_stats_visits_table_names() {
    struct Trace(usize);
    impl InPlaceVisitor for Trace {
        fn enter(&mut self, _node: &mut dyn Node) -> bool {
            false
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }
        fn enter_table_name(&mut self, _table: &mut crate::TableName) -> bool {
            self.0 += 1;
            false
        }
    }
    let mut statement = DropStatsStmt {
        Tables: vec![Default::default(), Default::default()],
        ..Default::default()
    };
    let mut trace = Trace(0);
    assert!(Walk(&mut statement, &mut trace));
    assert_eq!(trace.0, 2);
}

#[test]
fn go_merge_25_alter_table_visits_old_and_relative_columns() {
    struct Trace(Vec<&'static str>);
    impl InPlaceVisitor for Trace {
        fn enter(&mut self, _node: &mut dyn Node) -> bool {
            false
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }
        fn enter_table_name(&mut self, _table: &mut crate::TableName) -> bool {
            self.0.push("table");
            false
        }
        fn enter_column_name(&mut self, _column: &mut ColumnName) -> bool {
            self.0.push("column");
            false
        }
    }
    let mut statement = AlterTableStmt {
        Specs: vec![AlterTableSpec {
            OldColumnName: Some(ColumnName::default()),
            Position: ColumnPosition {
                RelativeColumn: Some(ColumnName::default()),
                ..Default::default()
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut trace = Trace(Vec::new());
    assert!(Walk(&mut statement, &mut trace));
    assert_eq!(trace.0, ["table", "column", "column"]);
}

#[test]
fn go_merge_25_create_index_visits_lock_algorithm_leaf() {
    struct Trace(Vec<&'static str>);
    impl InPlaceVisitor for Trace {
        fn enter(&mut self, _node: &mut dyn Node) -> bool {
            false
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }
        fn enter_embedded(&mut self, input: &mut dyn std::any::Any) -> bool {
            if input.is::<crate::IndexLockAndAlgorithm>() {
                self.0.push("lock algorithm");
            }
            false
        }
    }
    let mut statement = CreateIndexStmt {
        LockAlg: Some(Default::default()),
        ..Default::default()
    };
    let mut trace = Trace(Vec::new());
    assert!(Walk(&mut statement, &mut trace));
    assert_eq!(trace.0, ["lock algorithm"]);
}

#[test]
fn go_merge_25_select_visits_fields_before_from() {
    struct Trace(Vec<&'static str>);
    impl InPlaceVisitor for Trace {
        fn enter(&mut self, node: &mut dyn Node) -> bool {
            if node.as_any().is::<ExprNode>() {
                self.0.push("field");
            }
            false
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }
        fn enter_embedded(&mut self, input: &mut dyn std::any::Any) -> bool {
            if input.is::<TableRefsClause>() {
                self.0.push("from");
            }
            false
        }
    }
    let mut statement = SelectStmt {
        Fields: FieldList {
            Fields: vec![SelectField {
                Expr: Some(ExprNode::Value("x".into())),
                ..Default::default()
            }],
        },
        From: Some(TableRefsClause::default()),
        ..Default::default()
    };
    let mut trace = Trace(Vec::new());
    assert!(Walk(&mut statement, &mut trace));
    assert_eq!(trace.0, ["field", "from"]);
}

#[test]
fn go_merge_25_plan_replayer_load_skips_children() {
    struct Trace(usize);
    impl InPlaceVisitor for Trace {
        fn enter(&mut self, node: &mut dyn Node) -> bool {
            if node.as_any().is::<DoStmt>() {
                self.0 += 1;
            }
            false
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }
    }
    let mut statement = PlanReplayerStmt {
        Load: true,
        Stmt: Some(Box::new(DoStmt::default())),
        ..Default::default()
    };
    let mut trace = Trace(0);
    assert!(Walk(&mut statement, &mut trace));
    assert_eq!(trace.0, 0);
}

#[test]
fn go_merge_25_procedure_block_visits_variables_only() {
    struct Trace(Vec<&'static str>);
    impl InPlaceVisitor for Trace {
        fn enter(&mut self, node: &mut dyn Node) -> bool {
            if node.as_any().is::<ExprNode>() {
                self.0.push("default");
            }
            if node.as_any().is::<DoStmt>() {
                self.0.push("statement");
            }
            false
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }
    }
    let mut block = ProcedureBlock {
        ProcedureVars: vec![Box::new(ProcedureDecl {
            DeclNames: vec!["v".into()],
            DeclType: Default::default(),
            DeclDefault: Some(ExprNode::Value("x".into())),
        })],
        ProcedureProcStmts: vec![Box::new(DoStmt::default())],
        ..Default::default()
    };
    let mut trace = Trace(Vec::new());
    assert!(Walk(&mut block, &mut trace));
    assert_eq!(trace.0, ["default"]);
}

#[test]
fn go_merge_25_procedure_block_visits_dynamic_node_variable() {
    struct Trace(usize);
    impl InPlaceVisitor for Trace {
        fn enter(&mut self, node: &mut dyn Node) -> bool {
            if node.as_any().is::<DoStmt>() {
                self.0 += 1;
            }
            false
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }
    }
    let variable: Box<dyn Node> = Box::new(DoStmt::default());
    let mut block = ProcedureBlock {
        ProcedureVars: vec![Box::new(variable)],
        ..Default::default()
    };
    let mut trace = Trace(0);
    assert!(Walk(&mut block, &mut trace));
    assert_eq!(trace.0, 1);
}

#[test]
fn go_merge_25_table_source_visits_named_source() {
    struct Trace(usize);
    impl InPlaceVisitor for Trace {
        fn enter(&mut self, _node: &mut dyn Node) -> bool {
            false
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }
        fn enter_table_name(&mut self, _table: &mut crate::TableName) -> bool {
            self.0 += 1;
            false
        }
    }
    let mut statement = SelectStmt {
        From: Some(TableRefsClause {
            TableRefs: Join {
                Left: Some(Box::new(ResultSetNode::TableSource(TableSource::default()))),
                ..Default::default()
            },
        }),
        ..Default::default()
    };
    let mut trace = Trace(0);
    assert!(Walk(&mut statement, &mut trace));
    assert_eq!(trace.0, 1);
}

#[test]
fn go_merge_25_table_option_visits_time_unit() {
    struct Trace(usize);
    impl InPlaceVisitor for Trace {
        fn enter(&mut self, _node: &mut dyn Node) -> bool {
            false
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }
        fn enter_embedded(&mut self, input: &mut dyn std::any::Any) -> bool {
            if input.is::<TimeUnitType>() {
                self.0 += 1;
            }
            false
        }
    }
    let mut statement = CreateTableStmt {
        Options: vec![TableOption {
            TimeUnitValue: Some(TimeUnitType::Day),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut trace = Trace(0);
    assert!(Walk(&mut statement, &mut trace));
    assert_eq!(trace.0, 1);
}

#[test]
fn go_merge_25_flashback_tso_skips_timestamp_expression() {
    struct Trace(usize);
    impl InPlaceVisitor for Trace {
        fn enter(&mut self, node: &mut dyn Node) -> bool {
            if node.as_any().is::<ExprNode>() {
                self.0 += 1;
            }
            false
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }
    }
    let mut statement = FlashBackToTimestampStmt {
        FlashbackTSO: 42,
        FlashbackTS: Some(ExprNode::Value("ts".into())),
        ..Default::default()
    };
    let mut trace = Trace(0);
    assert!(Walk(&mut statement, &mut trace));
    assert_eq!(trace.0, 0);
}

#[test]
fn go_merge_25_leaf_methods_skip_unvisited_rust_fields() {
    struct Trace(usize);
    impl InPlaceVisitor for Trace {
        fn enter(&mut self, node: &mut dyn Node) -> bool {
            if node.as_any().is::<ExprNode>() || node.as_any().is::<DoStmt>() {
                self.0 += 1;
            }
            false
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }
    }
    let mut kill = KillStmt {
        Expr: Some(ExprNode::Value("id".into())),
        ..Default::default()
    };
    let mut else_block = ProcedureElseBlock {
        ProcedureIfStmts: vec![Box::new(DoStmt::default())],
        ..Default::default()
    };
    let mut trace = Trace(0);
    assert!(Walk(&mut kill, &mut trace));
    assert!(Walk(&mut else_block, &mut trace));
    assert_eq!(trace.0, 0);
}

#[test]
fn go_merge_25_legacy_match_visits_columns_before_against() {
    use crate::expressions::{ColumnName as LegacyColumnName, Expr as LegacyExpr};
    struct Trace(Vec<&'static str>);
    impl crate::expressions::Visitor for Trace {
        fn enter(&mut self, node: &mut LegacyExpr) -> bool {
            if matches!(node, LegacyExpr::Raw(_)) {
                self.0.push("against");
            }
            false
        }
        fn leave(&mut self, _node: &mut LegacyExpr) -> bool {
            true
        }
        fn enter_column_name(&mut self, _column: &mut LegacyColumnName) -> bool {
            self.0.push("column");
            false
        }
    }
    let mut expr = LegacyExpr::MatchAgainst(crate::expressions::MatchAgainst {
        column_names: vec![LegacyColumnName::new("", "", "body")],
        against: Box::new(LegacyExpr::Raw("term".into())),
        modifier: Default::default(),
    });
    let mut trace = Trace(Vec::new());
    assert!(expr.accept(&mut trace));
    assert_eq!(trace.0, ["column", "against"]);
}

#[test]
fn go_merge_25_insert_visits_select_before_columns() {
    struct Trace(Vec<&'static str>);
    impl InPlaceVisitor for Trace {
        fn enter(&mut self, node: &mut dyn Node) -> bool {
            if node.as_any().is::<DoStmt>() {
                self.0.push("select");
            }
            false
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }
        fn enter_column_name(&mut self, _column: &mut ColumnName) -> bool {
            self.0.push("column");
            false
        }
    }
    let mut statement = InsertStmt {
        Select: Some(Box::new(DoStmt::default())),
        Columns: vec![ColumnName::default()],
        ..Default::default()
    };
    let mut trace = Trace(Vec::new());
    assert!(Walk(&mut statement, &mut trace));
    assert_eq!(trace.0, ["select", "column"]);
}

#[test]
fn go_merge_25_drop_table_visits_each_table_and_stops_on_failure() {
    struct Trace {
        visited: usize,
        left_root: bool,
    }
    impl InPlaceVisitor for Trace {
        fn enter(&mut self, _node: &mut dyn Node) -> bool {
            false
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            self.left_root = true;
            true
        }
        fn enter_table_name(&mut self, _table: &mut crate::TableName) -> bool {
            self.visited += 1;
            false
        }
        fn leave_table_name(&mut self, _table: &mut crate::TableName) -> bool {
            self.visited < 2
        }
    }
    let mut statement = DropTableStmt {
        Tables: vec![Default::default(), Default::default(), Default::default()],
        ..Default::default()
    };
    let mut trace = Trace {
        visited: 0,
        left_root: false,
    };
    assert!(!Walk(&mut statement, &mut trace));
    assert_eq!(trace.visited, 2);
    assert!(!trace.left_root);
}

#[test]
fn go_merge_25_create_binding_chooses_sql_or_digest_branch() {
    struct Trace(Vec<&'static str>);
    impl InPlaceVisitor for Trace {
        fn enter(&mut self, node: &mut dyn Node) -> bool {
            if node.as_any().is::<DoStmt>() {
                self.0.push("sql");
            }
            if node.as_any().is::<ExprNode>() {
                self.0.push("digest");
            }
            false
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }
    }
    let mut statement = CreateBindingStmt {
        OriginNode: Some(Box::new(DoStmt::default())),
        HintedNode: Some(Box::new(DoStmt::default())),
        PlanDigests: vec![StringOrUserVar {
            UserVar: Some(ExprNode::Value("digest".into())),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut trace = Trace(Vec::new());
    assert!(Walk(&mut statement, &mut trace));
    assert_eq!(trace.0, ["sql", "sql"]);
}

#[test]
fn go_merge_25_embedded_skip_calls_leave_without_children() {
    struct Trace(Vec<&'static str>);
    impl InPlaceVisitor for Trace {
        fn enter(&mut self, _node: &mut dyn Node) -> bool {
            false
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }
        fn enter_embedded(&mut self, input: &mut dyn std::any::Any) -> bool {
            if input.is::<ReferenceDef>() {
                self.0.push("enter reference");
                return true;
            }
            false
        }
        fn leave_embedded(&mut self, input: &mut dyn std::any::Any) -> bool {
            if input.is::<ReferenceDef>() {
                self.0.push("leave reference");
            }
            true
        }
        fn enter_table_name(&mut self, _table: &mut crate::TableName) -> bool {
            self.0.push("table");
            false
        }
    }
    let mut statement = CreateTableStmt {
        Constraints: vec![crate::Constraint {
            Refer: Some(ReferenceDef::default()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut trace = Trace(Vec::new());
    assert!(Walk(&mut statement, &mut trace));
    assert_eq!(trace.0, ["table", "enter reference", "leave reference"]);
}

#[test]
fn go_merge_25_plan_replayer_chooses_statement_or_filter_children() {
    struct Trace(Vec<&'static str>);
    impl InPlaceVisitor for Trace {
        fn enter(&mut self, node: &mut dyn Node) -> bool {
            if node.as_any().is::<ExprNode>() {
                self.0.push("expr");
            }
            if node.as_any().is::<DoStmt>() {
                self.0.push("stmt");
            }
            false
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }
    }
    let make = || PlanReplayerStmt {
        HistoricalStatsInfo: Some(AsOfClause {
            TsExpr: ExprNode::Value("history".into()),
        }),
        Where: Some(ExprNode::Value("filter".into())),
        ..Default::default()
    };
    let mut statement = PlanReplayerStmt {
        Stmt: Some(Box::new(DoStmt::default())),
        ..make()
    };
    let mut trace = Trace(Vec::new());
    assert!(Walk(&mut statement, &mut trace));
    assert_eq!(trace.0, ["expr", "stmt"]);

    let mut filter = make();
    let mut trace = Trace(Vec::new());
    assert!(Walk(&mut filter, &mut trace));
    assert_eq!(trace.0, ["expr", "expr"]);
}

#[test]
fn go_merge_25_set_operator_visits_with_before_selects() {
    struct Trace(Vec<&'static str>);
    impl InPlaceVisitor for Trace {
        fn enter(&mut self, node: &mut dyn Node) -> bool {
            if node.as_any().is::<DoStmt>() {
                self.0.push("select");
            }
            false
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }
        fn enter_embedded(&mut self, input: &mut dyn std::any::Any) -> bool {
            if input.is::<WithClause>() {
                self.0.push("with");
            }
            false
        }
    }
    let mut list = SetOprSelectList::new(vec![Box::new(DoStmt::default())]);
    list.With = Some(WithClause::default().into_shared());
    let mut trace = Trace(Vec::new());
    assert!(Walk(&mut list, &mut trace));
    assert_eq!(trace.0, ["with", "select"]);
}
