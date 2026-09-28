// Copyright 2026 AsterSQL.

use crate::rule_aggregation_elimination::{
    AggFuncDesc, AggFuncName, AggMode, AggregationEliminator, LogicalAggregation, LogicalPlan,
    aggregationEliminateChecker, rewriteExpr,
};
use crate::task::{Expression, FieldType, JoinType, PlanKind, PlanNode, TypeCode};

fn field_type(code: TypeCode) -> FieldType {
    FieldType {
        code,
        flen: 0,
        decimal: 0,
        unsigned: false,
    }
}

fn column(index: usize, tp: &FieldType) -> Expression {
    Expression {
        name: format!("col_{index}"),
        column: Some(index),
        return_type: Some(tp.clone()),
        ..Expression::default()
    }
}

fn leaf(unique_keys: Vec<Vec<usize>>) -> LogicalPlan {
    LogicalPlan::Node {
        node: PlanNode {
            kind: PlanKind::TableScan,
            schema: vec![field_type(TypeCode::Int), field_type(TypeCode::String)],
            ..PlanNode::default()
        },
        unique_keys,
        max_one_row: false,
    }
}

fn agg(function: AggFuncDesc) -> LogicalAggregation {
    let tp = field_type(TypeCode::String);
    LogicalAggregation {
        agg_funcs: vec![function],
        group_by_items: vec![column(0, &tp)],
        child: Box::new(leaf(vec![vec![0]])),
        schema: vec![tp],
        output_columns: vec![0],
        no_eliminate: false,
    }
}

fn function(name: AggFuncName, args: Vec<Expression>, return_type: FieldType) -> AggFuncDesc {
    AggFuncDesc {
        name,
        args,
        distinct: false,
        mode: AggMode::Complete,
        return_type,
        order_by: Vec::new(),
    }
}

#[test]
fn group_concat_is_not_eliminated() {
    let tp = field_type(TypeCode::String);
    let plan = LogicalPlan::Aggregation(agg(function(
        AggFuncName::GroupConcat,
        vec![column(1, &tp)],
        tp,
    )));

    let (optimized, changed) = AggregationEliminator.Optimize(plan).unwrap();
    assert!(!changed);
    assert!(matches!(optimized, LogicalPlan::Aggregation(_)));
}

#[test]
fn distinct_with_any_non_column_argument_is_not_eliminated() {
    let tp = field_type(TypeCode::Int);
    let mut descriptor = function(
        AggFuncName::Count,
        vec![
            column(0, &tp),
            Expression {
                name: "1".into(),
                ..Expression::default()
            },
        ],
        tp,
    );
    descriptor.distinct = true;
    let mut aggregation = agg(descriptor);

    aggregationEliminateChecker::default().tryToEliminateDistinct(&mut aggregation);
    assert!(aggregation.agg_funcs[0].distinct);
}

#[test]
fn valid_semi_join_inner_distinct_does_not_require_a_unique_key() {
    let tp = field_type(TypeCode::Int);
    let mut aggregation = agg(function(AggFuncName::FirstRow, vec![column(1, &tp)], tp));
    aggregation.child = Box::new(leaf(Vec::new()));

    assert!(aggregationEliminateChecker::default().canEliminateSemiJoinInnerDistinct(&aggregation));
}

#[test]
fn max_rewrite_casts_when_argument_and_result_types_differ() {
    let input = field_type(TypeCode::Int);
    let output = field_type(TypeCode::String);
    let expression = rewriteExpr(&function(
        AggFuncName::Max,
        vec![column(1, &input)],
        output.clone(),
    ))
    .unwrap();

    assert_eq!(expression.name, "cast(col_1)");
    assert_eq!(expression.return_type, Some(output));
}

#[test]
fn optimizer_removes_valid_distinct_aggregation_from_semi_join_inner_side() {
    let tp = field_type(TypeCode::Int);
    let mut inner = agg(function(
        AggFuncName::FirstRow,
        vec![column(1, &tp)],
        tp.clone(),
    ));
    inner.child = Box::new(leaf(Vec::new()));
    let plan = LogicalPlan::Join {
        join_type: JoinType::Semi,
        left: Box::new(leaf(Vec::new())),
        right: Box::new(LogicalPlan::Aggregation(inner)),
        equal_conditions: Vec::new(),
        other_conditions: Vec::new(),
        schema: vec![tp],
    };

    let (optimized, changed) = AggregationEliminator.Optimize(plan).unwrap();
    assert!(changed);
    let LogicalPlan::Join { right, .. } = optimized else {
        panic!("semi join must remain a join")
    };
    assert!(matches!(*right, LogicalPlan::Node { .. }));
}
