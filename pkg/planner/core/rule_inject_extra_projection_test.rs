// Copyright 2026 AsterSQL.

use crate::rule_inject_extra_projection::{
    InjectProjBelowAgg, InjectProjBelowSort, TurnNominalSortIntoProj, injectProjBelowUnion,
};
use crate::task::{Expression, FieldType, PlanKind, PlanNode, StoreType, TypeCode};

fn ty(code: TypeCode, unsigned: bool) -> FieldType {
    FieldType {
        code,
        flen: 0,
        decimal: 0,
        unsigned,
    }
}

fn column(index: usize, return_type: FieldType) -> Expression {
    Expression {
        name: format!("col_{index}"),
        column: Some(index),
        return_type: Some(return_type),
        ..Default::default()
    }
}

fn scalar(name: &str, return_type: FieldType) -> Expression {
    Expression {
        name: name.into(),
        function_count: 1,
        return_type: Some(return_type),
        ..Default::default()
    }
}

fn leaf(schema: Vec<FieldType>) -> PlanNode {
    let mut node = PlanNode::new(PlanKind::TableScan);
    node.schema = schema;
    node
}

#[test]
fn union_projection_is_mpp_only_and_casts_only_mismatched_columns() {
    let signed = ty(TypeCode::Int, false);
    let unsigned = ty(TypeCode::Int, true);
    let string = ty(TypeCode::String, false);

    let mut root = PlanNode::new(PlanKind::UnionAll);
    root.schema = vec![signed.clone(), string.clone()];
    root.children = vec![leaf(vec![unsigned, string.clone()])];
    let unchanged = injectProjBelowUnion(root.clone());
    assert_eq!(unchanged.children[0].kind, PlanKind::TableScan);

    root.store = StoreType::TiFlash;
    let rewritten = injectProjBelowUnion(root);
    let projection = &rewritten.children[0];
    assert_eq!(projection.kind, PlanKind::Projection);
    assert_eq!(projection.expressions.len(), 2);
    assert_eq!(projection.expressions[0].name, "cast(col_0)");
    assert_eq!(projection.expressions[1].column, Some(1));
    assert_eq!(projection.schema, rewritten.schema);
}

#[test]
fn aggregate_ignores_constants_and_materializes_non_constants_in_go_order() {
    let int = ty(TypeCode::Int, false);
    let constant = Expression {
        name: "1".into(),
        return_type: Some(int.clone()),
        ..Default::default()
    };
    let function = scalar("plus(col_0, 1)", int.clone());
    let mut agg = PlanNode::new(PlanKind::HashAgg);
    agg.schema = vec![int.clone()];
    agg.children = vec![leaf(vec![int.clone()])];

    let unchanged = InjectProjBelowAgg(agg.clone(), &[constant.clone()], &[]);
    assert_eq!(unchanged.children[0].kind, PlanKind::TableScan);

    let rewritten = InjectProjBelowAgg(agg, &[function.clone(), constant], &[function.clone()]);
    let projection = &rewritten.children[0];
    assert_eq!(projection.kind, PlanKind::Projection);
    assert_eq!(
        projection.expressions.len(),
        1,
        "duplicate group key is reused"
    );
    assert_eq!(projection.expressions[0].name, function.name);
    assert_eq!(rewritten.agg_funcs[0].column, Some(0));
    assert_eq!(rewritten.group_items[0].column, Some(0));
}

#[test]
fn sort_scalar_key_gets_bottom_materialization_and_top_pruning_projection() {
    let int = ty(TypeCode::Int, false);
    let key = scalar("neg(col_0)", int.clone());
    let mut sort = PlanNode::new(PlanKind::Sort);
    sort.schema = vec![int.clone()];
    sort.children = vec![leaf(vec![int.clone()])];

    let rewritten = InjectProjBelowSort(sort, &[key.clone()]);
    assert_eq!(rewritten.kind, PlanKind::Projection);
    assert_eq!(rewritten.schema, vec![int.clone()]);
    let sort = &rewritten.children[0];
    assert_eq!(sort.kind, PlanKind::Sort);
    assert_eq!(sort.by_items[0].column, Some(1));
    let bottom = &sort.children[0];
    assert_eq!(bottom.kind, PlanKind::Projection);
    assert_eq!(bottom.expressions.len(), 2);
    assert_eq!(bottom.expressions[1].name, key.name);
}

#[test]
fn nominal_sort_matches_go_passthrough_and_two_projection_paths() {
    let int = ty(TypeCode::Int, false);
    let child = leaf(vec![int.clone()]);
    let mut nominal = PlanNode::new(PlanKind::NominalSort);
    nominal.children = vec![child.clone()];
    let key = scalar("neg(col_0)", int);

    let passthrough = TurnNominalSortIntoProj(nominal.clone(), true, &[key.clone()]);
    assert_eq!(passthrough.kind, PlanKind::TableScan);

    let rewritten = TurnNominalSortIntoProj(nominal, false, &[key]);
    assert_eq!(rewritten.kind, PlanKind::Projection);
    assert_eq!(rewritten.children[0].kind, PlanKind::Projection);
    assert_eq!(rewritten.children[0].expressions.len(), 2);
    assert_eq!(rewritten.children[0].children[0].kind, PlanKind::TableScan);
}
