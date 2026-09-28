// Copyright 2026 AsterSQL.

use crate::rule_eliminate_unionall_dual_item::EliminateUnionAllDualItem;
use crate::task::{FieldType, PlanKind, PlanNode, TypeCode};

fn dual(rows: f64) -> PlanNode {
    let mut node = PlanNode::new(PlanKind::Other("TableDual".to_owned()));
    node.stats.row_count = rows;
    node
}

#[test]
fn name_matches_go_rule() {
    assert_eq!(
        EliminateUnionAllDualItem.Name(),
        "union_all_eliminate_dual_item"
    );
}

#[test]
fn removes_direct_and_projected_empty_duals_without_collapsing_union() {
    let projected_dual = PlanNode::new(PlanKind::Projection).with_children(vec![dual(0.0)]);
    let live = dual(1.0);
    let union =
        PlanNode::new(PlanKind::UnionAll).with_children(vec![dual(0.0), projected_dual, live]);

    let (optimized, changed) = EliminateUnionAllDualItem.Optimize(union);

    // Go does not report removal of some direct children as a rule change.
    assert!(!changed);
    assert_eq!(optimized.kind, PlanKind::UnionAll);
    assert_eq!(optimized.children.len(), 1);
    assert_eq!(optimized.children[0].stats.row_count, 1.0);
}

#[test]
fn empty_union_becomes_zero_row_dual_and_keeps_schema() {
    let mut union = PlanNode::new(PlanKind::UnionAll).with_children(vec![dual(0.0)]);
    union.schema = vec![FieldType {
        code: TypeCode::Int,
        flen: 11,
        decimal: 0,
        unsigned: false,
    }];

    let (optimized, changed) = EliminateUnionAllDualItem.Optimize(union);

    assert!(changed);
    assert_eq!(optimized.kind, PlanKind::Other("TableDual".to_owned()));
    assert_eq!(optimized.stats.row_count, 0.0);
    assert_eq!(optimized.schema.len(), 1);
}

#[test]
fn filtering_precedes_recursion_like_go() {
    let nested_union = PlanNode::new(PlanKind::UnionAll).with_children(vec![dual(0.0)]);
    let projection = PlanNode::new(PlanKind::Projection).with_children(vec![nested_union]);
    let root = PlanNode::new(PlanKind::UnionAll).with_children(vec![projection]);

    let (optimized, changed) = EliminateUnionAllDualItem.Optimize(root);

    assert!(changed);
    assert_eq!(optimized.kind, PlanKind::UnionAll);
    assert_eq!(optimized.children.len(), 1);
    assert_eq!(optimized.children[0].kind, PlanKind::Projection);
    assert_eq!(
        optimized.children[0].children[0].kind,
        PlanKind::Other("TableDual".to_owned())
    );
}
