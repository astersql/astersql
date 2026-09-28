// Copyright 2026 AsterSQL.

use crate::main_test::logical_optimize_default_for_test;
use logicalop::LogicalPlan as _;
use logicalop_dependency as logicalop;

fn contains_join(plan: &logicalop::LogicalPlanRef, join_type: logicalop::JoinType) -> bool {
    plan.as_any()
        .downcast_ref::<logicalop::LogicalJoin>()
        .is_some_and(|join| join.JoinType == join_type)
        || plan
            .Children()
            .iter()
            .any(|child| contains_join(child, join_type))
}

#[test]
fn intersect_and_except_build_like_go_set_operators() {
    for (sql, join_type) in [
        ("select 1 intersect select 1", logicalop::JoinType::SemiJoin),
        (
            "select 1 except select 2",
            logicalop::JoinType::AntiSemiJoin,
        ),
        (
            "select 1 union all select 2 intersect select 2 except select 3",
            logicalop::JoinType::AntiSemiJoin,
        ),
    ] {
        let (_, logical) = logical_optimize_default_for_test(sql)
            .unwrap_or_else(|error| panic!("{sql} must build successfully: {error}"));
        assert!(contains_join(&logical, join_type), "wrong plan for {sql}");
    }
}

#[test]
fn unsupported_all_variants_keep_go_errors() {
    for (sql, operator) in [
        ("select 1 intersect all select 1", "INTERSECT ALL"),
        ("select 1 except all select 1", "EXCEPT ALL"),
    ] {
        let error = logical_optimize_default_for_test(sql)
            .err()
            .unwrap_or_else(|| panic!("{sql} must be rejected"));
        assert!(
            error.contains(operator),
            "unexpected {operator} error: {error}"
        );
    }
}
