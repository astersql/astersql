// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// LATERAL Join 逻辑计划构建与优化的集成测试。
//
// LATERAL 派生表可引用左侧表的列；相关引用会建成 `LogicalApply`
//（相关子查询的 Apply 算子），而非普通 Join。本文件覆盖计划构建、
// 连接重排、Schema 解析、错误路径、聚合、递归 CTE 与 MySQL 兼容性。

use logicalop_dependency as logicalop;
use logicalop_dependency::LogicalPlan as _;

use super::main_test::{build_logical_for_test, logical_optimize_for_test, logical_plan_string};

/// 统计计划树中 `LogicalApply` 节点个数。
fn apply_count(plan: &dyn logicalop::LogicalPlan) -> usize {
    usize::from(plan.as_any().is::<logicalop::LogicalApply>())
        + plan
            .Children()
            .iter()
            .map(|child| apply_count(child.as_ref()))
            .sum::<usize>()
}

/// 深度优先查找第一个 `LogicalApply`。
fn first_apply(plan: &dyn logicalop::LogicalPlan) -> Option<&logicalop::LogicalApply> {
    plan.as_any()
        .downcast_ref::<logicalop::LogicalApply>()
        .or_else(|| {
            plan.Children()
                .iter()
                .find_map(|child| first_apply(child.as_ref()))
        })
}

/// 单条构建用例：名称、SQL 与期望的 Apply 数量。
struct BuildCase {
    name: &'static str,
    sql: &'static str,
    expected_apply_count: usize,
}

/// 批量断言：计划可构建、Schema 非空，且 Apply 数量符合期望。
fn assert_build_cases(cases: &[BuildCase]) {
    for case in cases {
        let (_, plan) = build_logical_for_test(case.sql)
            .unwrap_or_else(|error| panic!("{}: {error}", case.name));
        assert!(plan.Schema().Len() > 0, "{}: empty schema", case.name);
        assert_eq!(
            apply_count(plan.as_ref()),
            case.expected_apply_count,
            "{}: {}",
            case.name,
            logical_plan_string(plan.as_ref())
        );
    }
}

/// 批量断言：构建失败且错误信息包含指定片段。
fn assert_error_cases(cases: &[(&str, &str, &str)]) {
    for (name, sql, fragment) in cases {
        let error = build_logical_for_test(sql)
            .err()
            .unwrap_or_else(|| panic!("{name}: expected planner error"));
        assert!(error.contains(fragment), "{name}: {error}");
    }
}

/// 基本 LATERAL / 非 LATERAL 构建，以及 LEFT/RIGHT JOIN LATERAL 拒绝。
#[test]
fn test_lateral_join_plan_building() {
    assert_build_cases(&[
        BuildCase {
            name: "comma",
            sql: "SELECT * FROM t, LATERAL (SELECT t.a) AS dt",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "cross",
            sql: "SELECT * FROM t CROSS JOIN LATERAL (SELECT t.a + t.b as sum) AS dt",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "ordinary derived",
            sql: "SELECT * FROM t, (SELECT a FROM t) AS dt",
            expected_apply_count: 0,
        },
        BuildCase {
            name: "correlated",
            sql: "SELECT * FROM t t1, LATERAL (SELECT * FROM t WHERE t.a = t1.a) AS dt",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "multiple",
            sql: "SELECT * FROM t, LATERAL (SELECT t.a) AS dt1, LATERAL (SELECT t.b) AS dt2",
            expected_apply_count: 2,
        },
        BuildCase {
            name: "aggregate",
            sql: "SELECT * FROM t t1, LATERAL (SELECT COUNT(*) FROM t WHERE t.a = t1.a) AS dt",
            expected_apply_count: 1,
        },
    ]);
    assert_error_cases(&[
        (
            "left unsupported",
            "SELECT * FROM t LEFT JOIN LATERAL (SELECT t.b) AS dt ON true",
            "LEFT JOIN is not supported with LATERAL",
        ),
        (
            "right unsupported",
            "SELECT * FROM t RIGHT JOIN LATERAL (SELECT t.a) AS dt ON true",
            "RIGHT JOIN is not supported with LATERAL",
        ),
    ]);
}

/// 常量 / 相关 / 聚合 LATERAL 经逻辑优化后 Schema 仍有效。
#[test]
fn test_lateral_join_optimization() {
    for (name, sql) in [
        ("constant", "SELECT * FROM t, LATERAL (SELECT 1 as x) AS dt"),
        ("correlated", "SELECT * FROM t, LATERAL (SELECT t.a) AS dt"),
        (
            "aggregate",
            "SELECT * FROM t t1, LATERAL (SELECT COUNT(*) FROM t WHERE t.a = t1.a) AS dt",
        ),
    ] {
        let (_, plan) =
            build_logical_for_test(sql).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(plan.Schema().Len() > 0, "{name}: valid schema");
    }
}

/// 多 LATERAL 与多左表场景下 Apply 数量与构建正确性。
#[test]
fn test_lateral_join_reordering() {
    assert_build_cases(&[
        BuildCase {
            name: "multiple",
            sql: "SELECT * FROM t, LATERAL (SELECT t.a) AS dt1, LATERAL (SELECT t.b) AS dt2",
            expected_apply_count: 2,
        },
        BuildCase {
            name: "multiple left",
            sql: "SELECT * FROM t t1, t t2, LATERAL (SELECT t1.a + t2.a) AS dt",
            expected_apply_count: 1,
        },
    ]);
}

/// Schema / 作用域：WHERE、嵌套、USING/NATURAL 合并列与链式 LATERAL 别名。
#[test]
fn test_lateral_join_schema_resolution() {
    assert_build_cases(&[
        BuildCase {
            name: "left column",
            sql: "SELECT * FROM t, LATERAL (SELECT t.a + 1 AS x) AS dt",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "where",
            sql: "SELECT * FROM t t1, LATERAL (SELECT * FROM t WHERE t.a = t1.a) AS dt WHERE dt.b > 10",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "nested",
            sql: "SELECT * FROM t, LATERAL (SELECT * FROM (SELECT t.a) AS inner_dt) AS dt",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "deep ON",
            sql: "SELECT * FROM t AS t1 JOIN t AS t2 ON t1.a=t2.a JOIN t AS t3 ON t2.b=t3.b, LATERAL (SELECT t1.c, t3.d) AS dt",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "deep USING",
            sql: "SELECT * FROM t AS t1 JOIN t AS t2 USING(a) JOIN t AS t3 ON t2.b=t3.b, LATERAL (SELECT t1.c, t3.d) AS dt",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "merged t1.a",
            sql: "SELECT * FROM t AS t1 JOIN t AS t2 USING(a) JOIN t AS t3 ON t2.b=t3.b, LATERAL (SELECT t1.a, t3.d) AS dt",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "natural t1",
            sql: "SELECT * FROM t AS t1 NATURAL JOIN t AS t2 JOIN t AS t3 ON t1.b=t3.b, LATERAL (SELECT t1.c, t3.d) AS dt",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "simple merged",
            sql: "SELECT * FROM t AS t1 JOIN t AS t2 USING(a), LATERAL (SELECT t2.a) AS dt",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "deep merged",
            sql: "SELECT * FROM t AS t1 JOIN t AS t2 USING(a) JOIN t AS t3 ON t1.b=t3.b, LATERAL (SELECT t2.a, t3.d) AS dt",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "natural t2",
            sql: "SELECT * FROM t AS t1 NATURAL JOIN t AS t2 JOIN t AS t3 ON t1.b=t3.b, LATERAL (SELECT t2.a, t3.d) AS dt",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "nested lateral",
            sql: "SELECT * FROM t, LATERAL (SELECT t.a) AS dt1, LATERAL (SELECT dt1.a) AS dt2",
            expected_apply_count: 2,
        },
    ]);
}

/// 计划字符串非空，且相关 LATERAL 产生 Apply。
#[test]
fn test_lateral_join_explain() {
    let (_, plan) = build_logical_for_test("SELECT * FROM t, LATERAL (SELECT t.a) AS dt")
        .expect("explain lateral plan");
    assert!(!logical_plan_string(plan.as_ref()).is_empty());
    assert!(first_apply(plan.as_ref()).is_some());
}

/// 错误路径：外连接 LATERAL 拒绝；CROSS/INNER/逗号 JOIN 仍成功。
#[test]
fn test_lateral_join_error_paths() {
    assert_error_cases(&[
        (
            "right",
            "SELECT * FROM t RIGHT JOIN LATERAL (SELECT t.a) AS dt ON true",
            "RIGHT JOIN is not supported with LATERAL",
        ),
        (
            "left",
            "SELECT * FROM t LEFT JOIN LATERAL (SELECT t.a) AS dt ON true",
            "LEFT JOIN is not supported with LATERAL",
        ),
    ]);
    assert_build_cases(&[
        BuildCase {
            name: "cross",
            sql: "SELECT * FROM t CROSS JOIN LATERAL (SELECT t.a) AS dt",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "inner",
            sql: "SELECT * FROM t JOIN LATERAL (SELECT t.a) AS dt ON true",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "comma",
            sql: "SELECT * FROM t, LATERAL (SELECT t.a) AS dt",
            expected_apply_count: 1,
        },
    ]);
}

/// 边界：常量投影、空结果、UNION 与多列 LATERAL。
#[test]
fn test_lateral_join_edge_cases() {
    assert_build_cases(&[
        BuildCase {
            name: "constant",
            sql: "SELECT * FROM t, LATERAL (SELECT 1) AS dt",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "empty",
            sql: "SELECT * FROM t, LATERAL (SELECT t.a WHERE false) AS dt",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "union",
            sql: "SELECT * FROM t, LATERAL (SELECT t.a UNION SELECT t.b) AS dt",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "columns",
            sql: "SELECT * FROM t, LATERAL (SELECT t.a, t.b, t.c) AS dt",
            expected_apply_count: 1,
        },
    ]);
}

/// LATERAL 内聚合（COUNT/SUM/GROUP BY/MAX/MIN）均建成单个 Apply。
#[test]
fn test_lateral_join_with_aggregates() {
    assert_build_cases(&[
        BuildCase {
            name: "count",
            sql: "SELECT * FROM t t1, LATERAL (SELECT COUNT(*) as cnt FROM t WHERE t.a = t1.a) AS dt",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "sum",
            sql: "SELECT * FROM t t1, LATERAL (SELECT SUM(a) as total FROM t WHERE t.a = t1.a) AS dt",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "group",
            sql: "SELECT * FROM t t1, LATERAL (SELECT t.b, COUNT(*) FROM t WHERE t.a = t1.a GROUP BY t.b) AS dt",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "max min",
            sql: "SELECT * FROM t t1, LATERAL (SELECT MAX(a), MIN(b) FROM t WHERE t.a = t1.a) AS dt",
            expected_apply_count: 1,
        },
    ]);
}

/// 嵌套聚合、多别名与复杂 WHERE 的 LATERAL 构建。
#[test]
fn test_lateral_join_complex_scenarios() {
    assert_build_cases(&[
        BuildCase {
            name: "nested agg",
            sql: "SELECT * FROM t t1, LATERAL (SELECT AVG(cnt) FROM (SELECT COUNT(*) as cnt FROM t WHERE t.a = t1.a GROUP BY t.b) sub) AS dt",
            expected_apply_count: 1,
        },
        BuildCase {
            name: "multiple alias",
            sql: "SELECT * FROM t t1, LATERAL (SELECT t1.a) AS dt1, LATERAL (SELECT t1.b) AS dt2",
            expected_apply_count: 2,
        },
        BuildCase {
            name: "complex where",
            sql: "SELECT * FROM t t1, LATERAL (SELECT * FROM t WHERE t.a = t1.a AND t.b > t1.b OR t.c < t1.c) AS dt",
            expected_apply_count: 1,
        },
    ]);
}

/// 非 LATERAL 派生表不得看见外层列（作用域隔离）。
#[test]
fn test_lateral_join_scope_isolation_for_non_lateral_derived_table() {
    assert_error_cases(&[(
        "scope",
        "SELECT * FROM t AS t1 JOIN ((SELECT t1.a) AS s JOIN LATERAL (SELECT 1) AS l ON true) ON true",
        "Unknown column 't1.a'",
    )]);
}

/// USING 合并列在解相关后仍作为相关列保留在 Apply 上。
#[test]
fn test_lateral_join_decorrelate_with_using_and_on() {
    let sql = "SELECT * FROM t AS t1 JOIN t AS t2 USING(a) JOIN t AS t3 ON t2.b=t3.b, LATERAL (SELECT COUNT(*) AS c FROM t AS t4 WHERE t4.a=t2.a) AS dt";
    let (_, plan) = logical_optimize_for_test(sql, rule_dependency::FLAG_DECORRELATE)
        .expect("USING lateral decorrelation");
    let apply = first_apply(plan.as_ref()).expect("correlated LATERAL remains Apply");
    assert!(
        !apply.CorCols.is_empty(),
        "merged USING column remains correlated"
    );
}

/// 递归 CTE 允许 LATERAL 成员带 ORDER BY/LIMIT；非 LATERAL 则拒绝。
#[test]
fn test_recursive_cte_with_lateral_order_by_limit() {
    let success = [
        r#"WITH RECURSIVE hierarchy AS (SELECT a,b FROM t WHERE a=1 UNION ALL SELECT n.a,n.b FROM hierarchy h CROSS JOIN LATERAL (SELECT a,b FROM t WHERE a=h.a+1 ORDER BY b DESC LIMIT 3) n WHERE h.a<5) SELECT * FROM hierarchy"#,
        r#"WITH RECURSIVE cte AS (SELECT a FROM t WHERE a=1 UNION ALL SELECT n.a FROM cte c CROSS JOIN LATERAL (SELECT a FROM t WHERE a=c.a+1 LIMIT 5) n) SELECT * FROM cte"#,
        r#"WITH RECURSIVE cte AS (SELECT a,b FROM t WHERE a=1 UNION ALL SELECT n.a,n.b FROM cte c CROSS JOIN LATERAL (SELECT a,b FROM t WHERE a=c.a+1 ORDER BY b ASC) n) SELECT * FROM cte"#,
        r#"WITH RECURSIVE hierarchy AS (SELECT a,b FROM t WHERE a=1 UNION ALL SELECT n.a,n.b FROM hierarchy h, LATERAL (SELECT a,b FROM t WHERE a=h.a+1 ORDER BY b DESC LIMIT 2) n) SELECT * FROM hierarchy"#,
        r#"WITH RECURSIVE cte AS (SELECT a FROM t WHERE a=1 UNION ALL SELECT n2.a FROM cte c, LATERAL (SELECT a FROM t WHERE a=c.a+1 ORDER BY a LIMIT 2) n1, LATERAL (SELECT a FROM t WHERE a=n1.a+1 ORDER BY a DESC LIMIT 1) n2) SELECT * FROM cte"#,
    ];
    for sql in success {
        let (_, plan) =
            build_logical_for_test(sql).unwrap_or_else(|error| panic!("{sql}: {error}"));
        assert!(plan.Schema().Len() > 0);
    }
    assert_error_cases(&[
        (
            "non-lateral order",
            r#"WITH RECURSIVE cte AS (SELECT a FROM t WHERE a=1 UNION ALL (SELECT t.a FROM t,cte WHERE t.a=cte.a+1 ORDER BY t.a)) SELECT * FROM cte"#,
            "ORDER BY / LIMIT in recursive query block",
        ),
        (
            "non-lateral limit",
            r#"WITH RECURSIVE cte AS (SELECT a FROM t WHERE a=1 UNION ALL (SELECT t.a FROM t,cte WHERE t.a=cte.a+1 LIMIT 10)) SELECT * FROM cte"#,
            "ORDER BY / LIMIT in recursive query block",
        ),
        (
            "non-lateral derived",
            r#"WITH RECURSIVE cte AS (SELECT a FROM t WHERE a=1 UNION ALL SELECT a FROM (SELECT a FROM t,cte WHERE t.a=cte.a+1 ORDER BY a) sub) SELECT * FROM cte"#,
            "ORDER BY / LIMIT in recursive query block",
        ),
    ]);
}

/// MySQL 兼容：拒绝 RIGHT JOIN LATERAL，且内部派生别名不可被外层 LATERAL 穿透。
#[test]
fn test_lateral_join_mysql_compatibility() {
    assert_error_cases(&[
        (
            "right",
            "SELECT * FROM t RIGHT JOIN LATERAL (SELECT 1 AS x) AS dt ON true",
            "RIGHT JOIN is not supported with LATERAL",
        ),
        (
            "inner t1",
            "SELECT * FROM (SELECT t1.a FROM t AS t1 JOIN t AS t2 USING(a)) AS j, LATERAL (SELECT t1.a) AS dt",
            "Unknown column",
        ),
        (
            "inner t2",
            "SELECT * FROM (SELECT t1.a FROM t AS t1 JOIN t AS t2 USING(a)) AS j, LATERAL (SELECT t2.a) AS dt",
            "Unknown column",
        ),
    ]);
    assert_build_cases(&[BuildCase {
        name: "derived alias",
        sql: "SELECT * FROM (SELECT a FROM t) AS j, LATERAL (SELECT j.a) AS dt",
        expected_apply_count: 1,
    }]);
}
