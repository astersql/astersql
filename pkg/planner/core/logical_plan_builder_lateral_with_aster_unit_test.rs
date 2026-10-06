// Copyright 2026 AsterSQL.

// LATERAL 与递归 CTE 逻辑计划构建的 Aster 单元测试。
//
// 验证相关 LATERAL 建成 Apply、链式 LATERAL 别名可见性，以及
// 递归 CTE 成员中带 ORDER BY/LIMIT 的 LATERAL 可成功构建。

use logicalop_dependency as logicalop;
use logicalop_dependency::LogicalPlan as _;

use super::main_test::build_logical_for_test;

/// 判断计划树是否包含 `LogicalApply`。
fn contains_apply(plan: &dyn logicalop::LogicalPlan) -> bool {
    plan.as_any().is::<logicalop::LogicalApply>()
        || plan
            .Children()
            .iter()
            .any(|child| contains_apply(child.as_ref()))
}

/// 判断计划树是否包含物化 CTE 消费者。
fn contains_cte_reader(plan: &dyn logicalop::LogicalPlan) -> bool {
    plan.as_any().is::<logicalop::LogicalCTE>()
        || plan
            .Children()
            .iter()
            .any(|child| contains_cte_reader(child.as_ref()))
}

/// 相关 LATERAL 派生表应建成 Apply，而非普通 Join。
#[test]
fn builds_correlated_lateral_as_apply() {
    let (_, plan) = build_logical_for_test(
        "SELECT * FROM t AS outer_t, LATERAL (SELECT outer_t.a + 1 AS x) AS derived_t",
    )
    .expect("valid correlated LATERAL must build");
    assert!(contains_apply(plan.as_ref()));
}

/// 多个 LATERAL 从左到右可见先前别名（如 dt2 可引用 dt1）。
#[test]
fn chains_lateral_aliases_left_to_right() {
    let (_, plan) = build_logical_for_test(
        "SELECT * FROM t, LATERAL (SELECT t.a) AS dt1, LATERAL (SELECT dt1.a) AS dt2",
    )
    .expect("each LATERAL operand must see prior aliases");
    assert_eq!(
        plan.OutputNames()
            .0
            .iter()
            .flatten()
            .filter(|name| name.TblName.L == "dt1")
            .count(),
        1
    );
}

/// 递归 CTE 的递归分支允许 CROSS JOIN LATERAL 带 ORDER BY/LIMIT。
#[test]
fn builds_recursive_cte_with_lateral_member() {
    let sql = r#"
        WITH RECURSIVE cte AS (
            SELECT a FROM t WHERE a = 1
            UNION ALL
            SELECT next_row.a
            FROM cte
            CROSS JOIN LATERAL (
                SELECT a FROM t WHERE a = cte.a + 1 ORDER BY a LIMIT 1
            ) AS next_row
        )
        SELECT * FROM cte
    "#;
    let (_, plan) = build_logical_for_test(sql).expect("recursive CTE with LATERAL must build");
    assert!(plan.Schema().Len() > 0);
}

/// 单消费者 CTE 的消费计数必须在每次构建时重新计算；重复规划同一 SQL
/// 不得把旧计数累加成多消费者并退化为物化 CTE。
#[test]
fn cte_consumer_count_is_recomputed_for_each_plan_build() {
    let single_consumer = "WITH cte AS (SELECT * FROM t) SELECT * FROM cte WHERE a = 1";
    for _ in 0..2 {
        let (_, plan) = build_logical_for_test(single_consumer)
            .expect("single-consumer CTE must build repeatedly");
        assert!(
            !contains_cte_reader(plan.as_ref()),
            "single-consumer CTE must remain inline on every build"
        );
    }

    let (_, plan) = build_logical_for_test(
        "WITH first_cte AS (SELECT * FROM t), second_cte AS (SELECT * FROM first_cte) SELECT * FROM second_cte",
    )
    .expect("a later CTE may consume an earlier inline CTE");
    assert!(
        !contains_cte_reader(plan.as_ref()),
        "consumer counts must be known before later CTE definitions are built"
    );

    let (_, plan) = build_logical_for_test(
        "WITH cte AS (SELECT * FROM t) SELECT * FROM cte c1 JOIN cte c2 ON c1.a = c2.a",
    )
    .expect("multi-consumer CTE must build");
    assert!(
        contains_cte_reader(plan.as_ref()),
        "multi-consumer CTE must remain materialized by default"
    );
}
