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
