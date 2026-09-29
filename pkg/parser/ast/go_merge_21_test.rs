// Copyright 2026 AsterSQL.

use crate::{ColumnName, ExprKind, ExprNode, InPlaceVisitor, Node, TableName, Walk};

#[test]
fn go_merge_21_table_name_expression_visits_and_mutates_name() {
    struct Rename {
        visited: usize,
    }

    impl InPlaceVisitor for Rename {
        fn enter(&mut self, _node: &mut dyn Node) -> bool {
            false
        }

        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }

        fn enter_table_name(&mut self, table: &mut TableName) -> bool {
            self.visited += 1;
            table.Name.O = "renamed".into();
            true
        }
    }

    let mut expression = ExprNode {
        Kind: ExprKind::TableName(TableName::default()),
        ..ExprNode::default()
    };
    let mut visitor = Rename { visited: 0 };
    assert!(Walk(&mut expression, &mut visitor));
    assert_eq!(visitor.visited, 1);
    match expression.Kind {
        ExprKind::TableName(table) => assert_eq!(table.Name.O, "renamed"),
        _ => panic!("expression kind changed"),
    }
}

#[test]
fn go_merge_21_match_against_visits_columns_before_against_expression() {
    struct Recorder {
        events: Vec<&'static str>,
    }

    impl InPlaceVisitor for Recorder {
        fn enter(&mut self, _node: &mut dyn Node) -> bool {
            self.events.push("expression");
            false
        }

        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            true
        }

        fn enter_column_name(&mut self, _column: &mut ColumnName) -> bool {
            self.events.push("column");
            false
        }
    }

    let mut expression = ExprNode {
        Kind: ExprKind::MatchAgainst {
            ColumnNames: vec![ColumnName::default()],
            Against: Box::new(ExprNode::default()),
            Modifier: 0,
        },
        ..ExprNode::default()
    };
    let mut visitor = Recorder { events: Vec::new() };
    assert!(Walk(&mut expression, &mut visitor));
    assert_eq!(visitor.events, ["expression", "column", "expression"]);
}
