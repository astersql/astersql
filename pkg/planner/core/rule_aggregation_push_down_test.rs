// Copyright 2026 AsterSQL.

use crate::rule_aggregation_elimination::{
    AggFuncDesc, AggFuncName, AggMode, LogicalAggregation, LogicalPlan,
};
use crate::rule_aggregation_push_down::AggregationPushDownSolver;
use crate::task::{Expression, FieldType, JoinType, PlanKind, PlanNode, TypeCode};

fn field_type() -> FieldType {
    FieldType {
        code: TypeCode::Int,
        flen: 0,
        decimal: 0,
        unsigned: false,
    }
}

fn column(index: usize) -> Expression {
    Expression {
        name: format!("col_{index}"),
        column: Some(index),
        return_type: Some(field_type()),
        ..Expression::default()
    }
}

fn function(name: AggFuncName, column_index: Option<usize>) -> AggFuncDesc {
    AggFuncDesc {
        name,
        args: vec![column_index.map_or_else(
            || Expression {
                name: "1".into(),
                return_type: Some(field_type()),
                ..Expression::default()
            },
            column,
        )],
        distinct: false,
        mode: AggMode::Complete,
        return_type: field_type(),
        order_by: Vec::new(),
    }
}

fn leaf(columns: usize) -> LogicalPlan {
    LogicalPlan::Node {
        node: PlanNode {
            kind: PlanKind::TableScan,
            schema: vec![field_type(); columns],
            ..PlanNode::default()
        },
        unique_keys: Vec::new(),
        max_one_row: false,
    }
}

fn join_aggregation(functions: Vec<AggFuncDesc>) -> LogicalPlan {
    LogicalPlan::Aggregation(LogicalAggregation {
        agg_funcs: functions,
        group_by_items: Vec::new(),
        child: Box::new(LogicalPlan::Join {
            join_type: JoinType::Inner,
            left: Box::new(leaf(1)),
            right: Box::new(leaf(1)),
            equal_conditions: vec![(0, 0)],
            other_conditions: Vec::new(),
            schema: vec![field_type(); 2],
        }),
        schema: vec![field_type()],
        output_columns: vec![0],
        no_eliminate: false,
    })
}

#[test]
fn decomposable_function_lists_match_go() {
    let solver = AggregationPushDownSolver::default();
    let mut distinct_max = function(AggFuncName::Max, Some(0));
    distinct_max.distinct = true;
    assert!(solver.isDecomposableWithJoin(&distinct_max));
    assert!(!solver.isDecomposableWithJoin(&function(AggFuncName::BitAnd, Some(0))));
    assert!(solver.isDecomposableWithUnion(&function(AggFuncName::Avg, Some(0))));
    assert!(solver.isDecomposableWithUnion(&function(AggFuncName::ApproxCountDistinct, Some(0),)));
    assert!(!solver.isDecomposableWithUnion(&function(AggFuncName::BitOr, Some(0))));
}

#[test]
fn join_pushdown_rewrites_parent_to_final_aggregate() {
    let plan = join_aggregation(vec![function(AggFuncName::Count, Some(0))]);
    let (optimized, changed) = AggregationPushDownSolver::default().Optimize(plan).unwrap();
    assert!(changed);
    let LogicalPlan::Aggregation(aggregation) = optimized else {
        panic!("aggregation must remain the root")
    };
    assert_eq!(aggregation.agg_funcs[0].name, AggFuncName::Sum);
    assert_eq!(aggregation.agg_funcs[0].mode, AggMode::Final);
    assert_eq!(aggregation.agg_funcs[0].args[0].name, "partial_0");
}

#[test]
fn count_on_one_join_side_prevents_pushdown_on_the_other_side() {
    let plan = join_aggregation(vec![
        function(AggFuncName::Count, Some(0)),
        function(AggFuncName::Max, Some(1)),
    ]);
    let (optimized, changed) = AggregationPushDownSolver::default().Optimize(plan).unwrap();
    assert!(changed);
    let LogicalPlan::Aggregation(aggregation) = optimized else {
        panic!("aggregation must remain the root")
    };
    let LogicalPlan::Join { left, right, .. } = *aggregation.child else {
        panic!("aggregation child must remain a join")
    };
    assert!(matches!(*left, LogicalPlan::Aggregation(_)));
    assert!(!matches!(*right, LogicalPlan::Aggregation(_)));
}

#[test]
fn constant_aggregate_is_assigned_to_right_side_of_inner_join() {
    let plan = join_aggregation(vec![function(AggFuncName::Count, None)]);
    let (optimized, changed) = AggregationPushDownSolver::default().Optimize(plan).unwrap();
    assert!(changed);
    let LogicalPlan::Aggregation(aggregation) = optimized else {
        panic!("aggregation must remain the root")
    };
    let LogicalPlan::Join { left, right, .. } = *aggregation.child else {
        panic!("aggregation child must remain a join")
    };
    assert!(!matches!(*left, LogicalPlan::Aggregation(_)));
    assert!(matches!(*right, LogicalPlan::Aggregation(_)));
    assert_eq!(aggregation.agg_funcs[0].args[0].column, Some(1));
}
