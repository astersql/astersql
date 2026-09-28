// Copyright 2026 AsterSQL.

use crate::rule_eliminate_projection::{
    ProjectionEliminator, canProjectionBeEliminatedLoose, canProjectionBeEliminatedStrict,
};
use crate::task::{Expression, FieldType, PlanKind, PlanNode, TypeCode};

fn column(offset: usize) -> Expression {
    Expression {
        column: Some(offset),
        ..Default::default()
    }
}

fn field() -> FieldType {
    FieldType {
        code: TypeCode::Int,
        flen: 11,
        decimal: 0,
        unsigned: false,
    }
}

fn projection(expressions: Vec<Expression>, child: PlanNode) -> PlanNode {
    let mut node = PlanNode::new(PlanKind::Projection).with_children(vec![child]);
    node.expressions = expressions;
    node
}

#[test]
fn loose_check_accepts_every_pure_column_projection_like_go() {
    let scan = PlanNode::new(PlanKind::TableScan);
    let reordered = projection(vec![column(1), column(0)], scan);
    assert!(canProjectionBeEliminatedLoose(&reordered));

    let mut computed = reordered;
    computed.expressions[1] = Expression {
        name: "plus".into(),
        function_count: 1,
        ..Default::default()
    };
    assert!(!canProjectionBeEliminatedLoose(&computed));
}

#[test]
fn strict_check_keeps_go_empty_schema_special_case() {
    let mut child = PlanNode::new(PlanKind::TableScan);
    child.schema = vec![field()];
    let projection = projection(Vec::new(), child);
    assert!(canProjectionBeEliminatedStrict(&projection));
}

#[test]
fn logical_elimination_obeys_root_union_cte_and_parent_boundaries() {
    let scan = PlanNode::new(PlanKind::TableScan);
    let identity = projection(vec![column(0)], scan.clone());

    let (root, changed) = ProjectionEliminator.Optimize(identity.clone());
    assert_eq!(
        root.kind,
        PlanKind::Projection,
        "Go starts with canEliminate=false"
    );
    assert!(!changed, "Go's optimizer rule reports planChanged=false");

    let reordered_child = projection(vec![column(1), column(0)], scan.clone());
    let outer = projection(vec![column(0)], reordered_child);
    let (optimized, changed) = ProjectionEliminator.Optimize(outer);
    assert_eq!(optimized.kind, PlanKind::Projection);
    assert_eq!(optimized.children[0].kind, PlanKind::TableScan);
    assert_eq!(optimized.expressions[0].column, Some(1));
    assert!(!changed);

    let union = PlanNode::new(PlanKind::UnionAll).with_children(vec![identity.clone()]);
    let (optimized, _) = ProjectionEliminator.Optimize(union);
    assert_eq!(optimized.children[0].kind, PlanKind::Projection);

    let cte = PlanNode::new(PlanKind::Cte).with_children(vec![identity]);
    let (optimized, _) = ProjectionEliminator.Optimize(cte);
    assert_eq!(optimized.children[0].kind, PlanKind::Projection);
}
