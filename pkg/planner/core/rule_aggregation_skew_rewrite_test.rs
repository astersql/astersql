// Copyright 2026 AsterSQL.

use crate::rule_aggregation_elimination::{
    AggFuncDesc, AggFuncName, AggMode, LogicalAggregation, LogicalPlan,
};
use crate::rule_aggregation_skew_rewrite::SkewDistinctAggRewriter;
use crate::task::{Expression, FieldType, PlanKind, PlanNode, TypeCode};

fn field_type() -> FieldType {
    FieldType {
        code: TypeCode::Int,
        flen: 0,
        decimal: 0,
        unsigned: false,
    }
}

fn expression(name: &str, column: Option<usize>) -> Expression {
    Expression {
        name: name.into(),
        column,
        return_type: Some(field_type()),
        ..Expression::default()
    }
}

fn function(name: AggFuncName, distinct: bool, mode: AggMode) -> AggFuncDesc {
    AggFuncDesc {
        name,
        args: vec![expression("col_1", Some(1))],
        distinct,
        mode,
        return_type: field_type(),
        order_by: Vec::new(),
    }
}

fn aggregation(functions: Vec<AggFuncDesc>, group_by: Vec<Expression>) -> LogicalAggregation {
    LogicalAggregation {
        schema: vec![field_type(); functions.len()],
        output_columns: (0..functions.len()).collect(),
        agg_funcs: functions,
        group_by_items: group_by,
        child: Box::new(LogicalPlan::Node {
            node: PlanNode {
                kind: PlanKind::TableScan,
                schema: vec![field_type(); 2],
                ..PlanNode::default()
            },
            unique_keys: Vec::new(),
            max_one_row: false,
        }),
        no_eliminate: false,
    }
}

#[test]
fn qualification_matches_go_function_mode_and_argument_contract() {
    let rewriter = SkewDistinctAggRewriter::default();
    assert!(rewriter.isQualifiedAgg(&function(AggFuncName::Avg, true, AggMode::Complete,)));
    assert!(rewriter.isQualifiedAgg(&function(AggFuncName::Sum, true, AggMode::Complete,)));
    assert!(!rewriter.isQualifiedAgg(&function(AggFuncName::BitAnd, false, AggMode::Complete,)));
    assert!(!rewriter.isQualifiedAgg(&function(AggFuncName::Count, false, AggMode::Partial1,)));

    let mut two_args = function(AggFuncName::Count, false, AggMode::Complete);
    two_args.args.push(expression("col_2", Some(2)));
    assert!(!rewriter.isQualifiedAgg(&two_args));

    let mut scalar = function(AggFuncName::Count, false, AggMode::Complete);
    scalar.args[0] = Expression {
        name: "plus(col_1, 1)".into(),
        function_count: 1,
        return_type: Some(field_type()),
        ..Expression::default()
    };
    assert!(!rewriter.isQualifiedAgg(&scalar));
}

#[test]
fn rewrite_preserves_go_modes_and_duplicate_group_items() {
    let distinct_arg = expression("col_1", Some(1));
    let agg = aggregation(
        vec![
            function(AggFuncName::Sum, true, AggMode::Complete),
            function(AggFuncName::Count, false, AggMode::Complete),
        ],
        vec![distinct_arg.clone()],
    );
    let rewritten = SkewDistinctAggRewriter::default()
        .rewriteSkewDistinctAgg(&agg)
        .expect("SUM(DISTINCT) is eligible");
    let LogicalPlan::Projection { child, .. } = rewritten else {
        panic!("non-distinct COUNT requires the Go-compatible cast projection")
    };
    let LogicalPlan::Aggregation(top) = *child else {
        panic!("projection child must be top aggregation")
    };
    assert!(
        top.agg_funcs
            .iter()
            .all(|function| function.mode == AggMode::Complete)
    );
    assert_eq!(top.agg_funcs[1].name, AggFuncName::Sum);
    let LogicalPlan::Aggregation(bottom) = *top.child else {
        panic!("top child must be bottom aggregation")
    };
    assert_eq!(bottom.group_by_items.len(), 2);
    assert!(
        bottom
            .agg_funcs
            .iter()
            .all(|function| function.mode == AggMode::Complete)
    );
}

#[test]
fn optimize_reports_only_descendant_change_like_go_rule_contract() {
    let plan = LogicalPlan::Aggregation(aggregation(
        vec![function(AggFuncName::Count, true, AggMode::Complete)],
        vec![expression("col_0", Some(0))],
    ));
    let (rewritten, changed) = SkewDistinctAggRewriter::default().Optimize(plan).unwrap();
    assert!(matches!(rewritten, LogicalPlan::Aggregation(_)));
    assert!(!changed);
}
