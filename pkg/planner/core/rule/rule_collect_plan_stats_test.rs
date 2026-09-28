// Copyright 2026 AsterSQL.

use super::rule_collect_plan_stats::CollectPredicateColumnsPoint;
use super::rule_init::LogicalRule;

#[test]
fn collect_predicate_columns_rule_name_matches_go_contract() {
    let rule = CollectPredicateColumnsPoint {
        collect_index_pruning_columns: false,
    };

    assert_eq!(rule.name(), "collect_predicate_columns_point");
}
