// Copyright 2026 AsterSQL.

use crate::rule_predicate_push_down::{PPDSolver, addPrefix4ShardIndexes, exprPrefixAdder};
use crate::task::{Expression, PlanKind, PlanNode};

fn expr(name: &str, column: Option<usize>) -> Expression {
    Expression {
        name: name.to_owned(),
        column,
        ..Default::default()
    }
}

#[test]
fn optimize_reports_plan_changed_false_like_go() {
    let scan = PlanNode::new(PlanKind::TableScan);
    let mut selection = PlanNode::new(PlanKind::Selection).with_children(vec![scan]);
    selection.conditions = vec![expr("eq:a:1", Some(0))];

    let (result, changed) = PPDSolver.Optimize(selection);

    assert!(!changed, "Go PPDSolver always returns planChanged=false");
    assert_eq!(result.kind, PlanKind::TableScan);
    assert_eq!(result.conditions[0].name, "eq:a:1");
}

#[test]
fn shard_prefixes_only_matching_eq_and_in_conditions() {
    let conditions = vec![
        expr("eq:a:1", Some(0)),
        expr("gt:a:0", Some(0)),
        expr("in:b:2,3", Some(1)),
    ];
    let adders = [exprPrefixAdder {
        shardColumn: 0,
        shardBits: 4,
    }];

    let result = addPrefix4ShardIndexes(&conditions, &adders);
    let names = result
        .iter()
        .map(|item| item.name.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        names,
        vec!["eq:a:1", "gt:a:0", "in:b:2,3", "shard(eq:a:1,4)"]
    );
}

#[test]
fn non_or_expression_is_preserved_by_dnf_helper() {
    let condition = expr("eq:a:1", Some(0));
    let adder = exprPrefixAdder {
        shardColumn: 0,
        shardBits: 4,
    };

    let result = adder.addExprPrefix4DNFCond(&condition);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, condition.name);
    assert_eq!(result[0].column, condition.column);
}
