// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::{
    ColumnName, ExprKind, ExprNode, ExprNodeVisitor, InPlaceVisitor, NewCIStr, Node,
    PartitionDefinition, PartitionDefinitionClause, PartitionMethod, ShowStmt, ShowStmtType,
    Visitor, dml, functions, walk::Children, walk::MutChildren,
};

#[derive(Default)]
struct PartitionTrace {
    events: Vec<String>,
    skip: Option<String>,
    stop: Option<String>,
}

impl PartitionTrace {
    fn enter_name(&mut self, name: &str) -> bool {
        self.events.push(format!("enter {name}"));
        self.skip.as_deref() == Some(name)
    }

    fn leave_name(&mut self, name: &str) -> bool {
        self.events.push(format!("leave {name}"));
        self.stop.as_deref() != Some(name)
    }
}

impl Visitor for PartitionTrace {
    fn enter(&mut self, node: &dyn Node) -> bool {
        if let Some(expr) = node.as_any().downcast_ref::<ExprNode>() {
            return self.enter_name(&expr.Text());
        }
        false
    }

    fn leave(&mut self, node: &dyn Node) -> bool {
        if let Some(expr) = node.as_any().downcast_ref::<ExprNode>() {
            return self.leave_name(&expr.Text());
        }
        true
    }

    fn enter_column_name(&mut self, column: &ColumnName) -> bool {
        self.enter_name(&column.Name.O)
    }

    fn leave_column_name(&mut self, column: &ColumnName) -> bool {
        self.leave_name(&column.Name.O)
    }
}

impl InPlaceVisitor for PartitionTrace {
    fn enter(&mut self, node: &mut dyn Node) -> bool {
        if let Some(expr) = node.as_any().downcast_ref::<ExprNode>() {
            return self.enter_name(&expr.Text());
        }
        false
    }

    fn leave(&mut self, node: &mut dyn Node) -> bool {
        if let Some(expr) = node.as_any().downcast_ref::<ExprNode>() {
            return self.leave_name(&expr.Text());
        }
        true
    }

    fn enter_column_name(&mut self, column: &mut ColumnName) -> bool {
        self.enter_name(&column.Name.O)
    }

    fn leave_column_name(&mut self, column: &mut ColumnName) -> bool {
        self.leave_name(&column.Name.O)
    }
}

fn named_expr(name: &str) -> ExprNode {
    let mut expr = ExprNode {
        Kind: ExprKind::Column(ColumnName {
            Name: NewCIStr(name),
            ..ColumnName::default()
        }),
        ..ExprNode::default()
    };
    expr.SetText(None, name.as_bytes());
    expr
}

#[test]
fn go_merge_13_partition_visitors_preserve_order_skip_and_stop() {
    let mut method = PartitionMethod {
        Expr: Some(named_expr("expr")),
        ColumnNames: vec![ColumnName {
            Name: NewCIStr("column"),
            ..ColumnName::default()
        }],
        ..PartitionMethod::default()
    };
    let expected = [
        "enter expr",
        "enter expr",
        "leave expr",
        "leave expr",
        "enter column",
        "leave column",
    ];
    let mut legacy = PartitionTrace::default();
    assert!(method.visit_children(&mut legacy));
    assert_eq!(legacy.events, expected);
    let mut in_place = PartitionTrace::default();
    assert!(method.visit_children_mut(&mut in_place));
    assert_eq!(in_place.events, expected);

    let mut clause = PartitionDefinition {
        Clause: PartitionDefinitionClause::In(vec![
            vec![named_expr("first"), named_expr("second")],
            vec![named_expr("third")],
        ]),
        ..PartitionDefinition::default()
    };
    let mut stopped = PartitionTrace {
        stop: Some("second".into()),
        ..PartitionTrace::default()
    };
    assert!(!clause.visit_children_mut(&mut stopped));
    assert!(!stopped.events.iter().any(|event| event.contains("third")));
}

struct ReplacePartitionExpr;

impl ExprNodeVisitor for ReplacePartitionExpr {
    fn Enter(&mut self, input: &ExprNode) -> (ExprNode, bool) {
        (input.clone(), false)
    }

    fn Leave(&mut self, input: &ExprNode) -> (ExprNode, bool) {
        if input.Text() == "original" {
            (named_expr("replacement"), true)
        } else {
            (input.clone(), true)
        }
    }
}

#[test]
fn go_merge_13_legacy_partition_visitor_writes_back_replacements() {
    let mut method = PartitionMethod {
        Expr: Some(named_expr("original")),
        ..PartitionMethod::default()
    };
    let mut definition = PartitionDefinition {
        Clause: PartitionDefinitionClause::LessThan(vec![named_expr("original")]),
        ..PartitionDefinition::default()
    };
    assert!(method.Accept(&mut ReplacePartitionExpr));
    assert!(definition.Accept(&mut ReplacePartitionExpr));
    assert_eq!(method.Expr.as_ref().unwrap().Text(), "replacement");
    match definition.Clause {
        PartitionDefinitionClause::LessThan(exprs) => {
            assert_eq!(exprs[0].Text(), "replacement");
        }
        _ => panic!("expected less-than clause"),
    }
}

#[derive(Default)]
struct LegacyTrace {
    events: Vec<String>,
    skip: Option<String>,
    stop: Option<String>,
}

impl ExprNodeVisitor for LegacyTrace {
    fn Enter(&mut self, input: &ExprNode) -> (ExprNode, bool) {
        let name = input.Text();
        self.events.push(format!("enter {name}"));
        (input.clone(), self.skip.as_deref() == Some(name.as_str()))
    }

    fn Leave(&mut self, input: &ExprNode) -> (ExprNode, bool) {
        let name = input.Text();
        self.events.push(format!("leave {name}"));
        (input.clone(), self.stop.as_deref() != Some(name.as_str()))
    }

    fn EnterColumn(&mut self, input: &ColumnName) -> (ColumnName, bool) {
        self.events.push(format!("enter {}", input.Name.O));
        (input.clone(), false)
    }

    fn LeaveColumn(&mut self, input: &ColumnName) -> (ColumnName, bool) {
        self.events.push(format!("leave {}", input.Name.O));
        (input.clone(), true)
    }
}

#[test]
fn go_merge_13_legacy_partition_order_and_stop() {
    let mut clause = PartitionDefinitionClause::In(vec![
        vec![named_expr("first"), named_expr("second")],
        vec![named_expr("third")],
    ]);
    let mut trace = LegacyTrace {
        stop: Some("second".into()),
        ..LegacyTrace::default()
    };
    assert!(!clause.Accept(&mut trace));
    assert_eq!(
        trace.events,
        [
            "enter first",
            "enter first",
            "leave first",
            "leave first",
            "enter second",
            "enter second",
            "leave second",
            "leave second",
        ]
    );
    assert!(PartitionDefinitionClause::None.Accept(&mut LegacyTrace::default()));
    assert!(
        PartitionDefinitionClause::History { Current: false }.Accept(&mut LegacyTrace::default())
    );
}

#[test]
fn go_merge_13_legacy_partition_skip_and_column_replacement() {
    struct ReplaceColumn;
    impl ExprNodeVisitor for ReplaceColumn {
        fn Enter(&mut self, input: &ExprNode) -> (ExprNode, bool) {
            (input.clone(), false)
        }
        fn Leave(&mut self, input: &ExprNode) -> (ExprNode, bool) {
            (input.clone(), true)
        }
        fn LeaveColumn(&mut self, input: &ColumnName) -> (ColumnName, bool) {
            let mut replacement = input.clone();
            replacement.Name = NewCIStr("changed");
            (replacement, true)
        }
    }
    let mut method = PartitionMethod {
        Expr: Some(named_expr("expr")),
        ColumnNames: vec![ColumnName {
            Name: NewCIStr("column"),
            ..ColumnName::default()
        }],
        ..PartitionMethod::default()
    };
    let mut skipped = LegacyTrace {
        skip: Some("expr".into()),
        ..LegacyTrace::default()
    };
    assert!(method.Accept(&mut skipped));
    assert_eq!(
        skipped.events,
        ["enter expr", "leave expr", "enter column", "leave column"]
    );
    assert!(method.Accept(&mut ReplaceColumn));
    assert_eq!(method.ColumnNames[0].Name.O, "changed");
    match &method.Expr.as_ref().unwrap().Kind {
        ExprKind::Column(column) => assert_eq!(column.Name.O, "changed"),
        _ => panic!("expected column expression"),
    }
}

#[test]
fn go_merge_13_partition_in_place_mutates_children() {
    struct Rename;
    impl InPlaceVisitor for Rename {
        fn enter(&mut self, _node: &mut dyn Node) -> bool {
            false
        }
        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }
        fn enter_column_name(&mut self, column: &mut ColumnName) -> bool {
            column.Name = NewCIStr("changed");
            false
        }
    }
    let mut method = PartitionMethod {
        ColumnNames: vec![ColumnName {
            Name: NewCIStr("old"),
            ..ColumnName::default()
        }],
        ..PartitionMethod::default()
    };
    let mut definition = PartitionDefinition {
        Clause: PartitionDefinitionClause::LessThan(vec![named_expr("old")]),
        ..PartitionDefinition::default()
    };
    assert!(method.visit_children_mut(&mut Rename));
    assert!(definition.visit_children_mut(&mut Rename));
    assert_eq!(method.ColumnNames[0].Name.O, "changed");
    match definition.Clause {
        PartitionDefinitionClause::LessThan(values) => match &values[0].Kind {
            ExprKind::Column(column) => assert_eq!(column.Name.O, "changed"),
            _ => panic!("expected column expression"),
        },
        _ => panic!("expected less-than clause"),
    }
}

#[test]
fn go_merge_13_full_join_restores_go_spelling() {
    let join = dml::Join::new(
        dml::ResultSet::table("left_table"),
        dml::ResultSet::table("right_table"),
        dml::JoinType::FullJoin,
    );
    assert_eq!(join.restore(), "`left_table` FULL OUTER JOIN `right_table`");
}

#[test]
fn go_merge_13_new_ast_names_match_go() {
    assert_eq!(functions::EmbedText, "embed_text");
    assert_eq!(functions::AggFuncMaxCount, "max_count");
    assert_eq!(functions::AggFuncMinCount, "min_count");
    let statement = ShowStmt {
        Tp: ShowStmtType::StorageClassTransitions,
        ..ShowStmt::default()
    };
    assert_eq!(
        crate::sem::SEMCommand::sem_command(&statement),
        "SHOW STORAGE_CLASS TRANSITIONS"
    );
    assert_eq!(
        crate::sql_restore::restore_node(&statement).unwrap(),
        "SHOW STORAGE_CLASS TRANSITIONS"
    );
}

#[test]
fn go_merge_13_expression_deep_equal_ignores_transient_metadata() {
    let mut left = ExprNode::Function(NewCIStr(""), NewCIStr("LOWER"), vec![named_expr("value")]);
    let mut right = ExprNode::Function(NewCIStr(""), NewCIStr("lower"), vec![named_expr("value")]);
    left.OriginTextPosition = 10;
    right.OriginTextPosition = 20;
    let before_left = left.clone();
    let before_right = right.clone();
    assert!(crate::ExpressionDeepEqual(&left, &right));
    assert_eq!(left, before_left);
    assert_eq!(right, before_right);
    if let ExprKind::Function { FnName, .. } = &mut right.Kind {
        *FnName = NewCIStr("upper");
    }
    assert!(!crate::ExpressionDeepEqual(&left, &right));
}
