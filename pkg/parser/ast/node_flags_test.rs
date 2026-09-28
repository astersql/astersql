// Copyright 2026 AsterSQL.

use super::flag::{
    FLAG_HAS_AGGREGATE_FUNC, FLAG_HAS_FUNC, FLAG_HAS_PARAM_MARKER, FLAG_HAS_SUBQUERY,
    FLAG_HAS_WINDOW_FUNC, FLAG_PRE_EVALUATED,
};
use super::*;

#[test]
fn flags_are_stored_on_real_nodes_and_recomputed_bottom_up() {
    let expr = ExprNode::Function(
        NewCIStr(""),
        NewCIStr("abs"),
        vec![ExprNode::ParamMarker(1)],
    );
    assert_eq!(expr.GetFlag(), 0);
    SetFlag(&expr);
    assert_eq!(expr.GetFlag(), FLAG_HAS_FUNC | FLAG_HAS_PARAM_MARKER);
    expr.SetFlag(FLAG_HAS_AGGREGATE_FUNC | FLAG_HAS_WINDOW_FUNC);
    assert!(HasAggFlag(&expr));
    assert!(HasWindowFlag(&expr));
    let copy = expr.clone();
    SetFlag(&expr);
    assert_eq!(expr.GetFlag(), FLAG_HAS_FUNC | FLAG_HAS_PARAM_MARKER);
    assert_eq!(
        copy.GetFlag(),
        FLAG_HAS_AGGREGATE_FUNC | FLAG_HAS_WINDOW_FUNC
    );
    let value = ExprNode::IntValue(1);
    value.SetFlag(FLAG_PRE_EVALUATED);
    SetFlag(&value);
    assert_eq!(value.GetFlag(), FLAG_PRE_EVALUATED);
}

#[test]
fn shared_subqueries_and_statement_children_are_visited() {
    let select = SelectStmt {
        Fields: FieldList {
            Fields: vec![SelectField {
                Expr: Some(ExprNode::ParamMarker(0)),
                ..Default::default()
            }],
        },
        ..Default::default()
    };
    let query = NodeRef::new(Box::new(select));
    let expr = ExprNode {
        node_text: Default::default(),
        Kind: ExprKind::Subquery {
            Query: query.clone(),
            MultiRows: false,
            Exists: false,
        },
        OriginTextPosition: 0,
        Flag: Default::default(),
    };
    SetFlag(&expr);
    assert_eq!(expr.GetFlag(), FLAG_HAS_SUBQUERY);
    query
        .with_node(|node| {
            let select = node.as_any().downcast_ref::<SelectStmt>().unwrap();
            assert_eq!(
                select.Fields.Fields[0].Expr.as_ref().unwrap().GetFlag(),
                FLAG_HAS_PARAM_MARKER
            );
        })
        .unwrap();
}
