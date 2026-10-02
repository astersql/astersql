// Copyright 2026 AsterSQL.

// 优化器规则（rule）casetest 包入口。
//
// 通过 `#[path]` 挂载各规则测试模块，并提供构造 JoinPlan / Expression 的测试辅助函数。
//
// 规则（rule）：逻辑/物理计划改写步骤，如谓词下推、连接重排、空 Selection 消除等。

#![allow(dead_code)]

/// Dual / TableDual 折叠相关用例。
#[cfg(test)]
#[path = "dual_test.rs"]
mod dual_test;
/// TestMain / BookKeeper suite 对照与优化器入口回归。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
/// CDC / DP / order-aware join reorder 用例。
#[cfg(test)]
#[path = "rule_cdc_join_reorder_test.rs"]
mod rule_cdc_join_reorder_test;
/// CommonHandle 二级索引排序用例。
#[cfg(test)]
#[path = "rule_common_handle_ordering_test.rs"]
mod rule_common_handle_ordering_test;
/// CommonHandle 二级索引范围与查询结果用例。
#[cfg(test)]
#[path = "rule_common_handle_range_test.rs"]
mod rule_common_handle_range_test;
/// CorrelateSolver / Apply 相关用例。
#[cfg(test)]
#[path = "rule_correlate_test.rs"]
mod rule_correlate_test;
/// 从 Window 派生 TopN 的用例。
#[cfg(test)]
#[path = "rule_derive_topn_from_window_test.rs"]
mod rule_derive_topn_from_window_test;
/// 消除无条件 Selection 的用例。
#[cfg(test)]
#[path = "rule_eliminate_empty_selection_test.rs"]
mod rule_eliminate_empty_selection_test;
/// 消除多余 Projection 的用例。
#[cfg(test)]
#[path = "rule_eliminate_projection_test.rs"]
mod rule_eliminate_projection_test;
/// 注入额外 Projection 的用例。
#[cfg(test)]
#[path = "rule_inject_extra_projection_test.rs"]
mod rule_inject_extra_projection_test;
/// Join reorder 通用用例。
#[cfg(test)]
#[path = "rule_join_reorder_test.rs"]
mod rule_join_reorder_test;
/// Outer join 转 Inner 的用例。
#[cfg(test)]
#[path = "rule_outer2inner_test.rs"]
mod rule_outer2inner_test;
/// Outer join 转 SemiJoin 的用例。
#[cfg(test)]
#[path = "rule_outer_to_semi_join_test.rs"]
mod rule_outer_to_semi_join_test;
/// 谓词下推用例。
#[cfg(test)]
#[path = "rule_predicate_pushdown_test.rs"]
mod rule_predicate_pushdown_test;
/// 谓词化简用例。
#[cfg(test)]
#[path = "rule_predicate_simplification_test.rs"]
mod rule_predicate_simplification_test;

/// 规则测试共享的 JoinPlan / Expression 构造辅助。
#[cfg(test)]
mod support {
    use std::path::PathBuf;

    use astersql_planner_core::rule_join_reorder::{JoinEdge, JoinNode, JoinPlan};
    use astersql_planner_core::task::{Expression, FieldType, JoinType, TypeCode};
    use astersql_testkit::testdata::TestData;

    /// 构造带名称与可选列下标的表达式节点。
    pub fn expr(name: &str, column: Option<usize>) -> Expression {
        Expression {
            name: name.into(),
            column,
            ..Default::default()
        }
    }

    /// 返回测试用的 INT 字段类型（flen=11）。
    pub fn int_type() -> FieldType {
        FieldType {
            code: TypeCode::Int,
            flen: 11,
            decimal: 0,
            unsigned: false,
        }
    }

    /// 构造叶子 JoinPlan（表扫描侧），并写入估算行数。
    pub fn leaf(id: usize, name: &str, schema: Vec<usize>, rows: f64) -> JoinPlan {
        JoinPlan {
            id,
            node: JoinNode::Leaf {
                name: name.into(),
                predicates: Vec::new(),
                unique_keys: Vec::new(),
                correlated_columns: Vec::new(),
            },
            schema,
            row_count: rows,
        }
    }

    /// 构造二元 Join：合并左右 schema，可选等值边写入 equal_conditions。
    pub fn join(
        id: usize,
        join_type: JoinType,
        left: JoinPlan,
        right: JoinPlan,
        edge: Option<(usize, usize)>,
    ) -> JoinPlan {
        // 输出 schema 为左右列并集，保持左列优先顺序。
        let mut schema = left.schema.clone();
        for column in &right.schema {
            if !schema.contains(column) {
                schema.push(*column);
            }
        }
        JoinPlan {
            id,
            node: JoinNode::Join {
                join_type,
                left: Box::new(left),
                right: Box::new(right),
                equal_conditions: edge
                    .map(|(left_column, right_column)| {
                        vec![JoinEdge {
                            left_column,
                            right_column,
                            null_equal: false,
                        }]
                    })
                    .unwrap_or_default(),
                other_conditions: Vec::new(),
                preferred_method: None,
            },
            schema,
            row_count: 10.0,
        }
    }

    /// Load one Go rule suite and return the input/output case counts for the
    /// named test functions.  This keeps the Rust tests tied to the same
    /// golden inventory as the Go package without duplicating the JSON data.
    pub fn fixture_case_counts(suite_name: &str, names: &[&str]) -> Vec<(usize, usize)> {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
        let suite = TestData::load_with_cascades(&directory, suite_name, true)
            .unwrap_or_else(|error| panic!("load {suite_name} fixture: {error}"));

        names
            .iter()
            .map(|name| {
                let (input, output) = suite
                    .LoadTestCasesByName(name, false)
                    .unwrap_or_else(|error| panic!("load {suite_name}/{name}: {error}"));
                let input = input
                    .as_array()
                    .unwrap_or_else(|| panic!("{suite_name}/{name} input is not an array"));
                let output = output
                    .as_array()
                    .unwrap_or_else(|| panic!("{suite_name}/{name} output is not an array"));
                assert_eq!(input.len(), output.len(), "{suite_name}/{name} case count");

                let (cascades_input, cascades_output) = suite
                    .LoadTestCasesByName(name, true)
                    .unwrap_or_else(|error| panic!("load {suite_name}/{name} cascades: {error}"));
                assert_eq!(
                    cascades_input
                        .as_array()
                        .unwrap_or_else(|| panic!(
                            "{suite_name}/{name} cascades input is not an array"
                        ))
                        .len(),
                    input.len(),
                    "{suite_name}/{name} cascades input count"
                );
                assert_eq!(
                    cascades_output
                        .as_array()
                        .unwrap_or_else(|| panic!(
                            "{suite_name}/{name} cascades output is not an array"
                        ))
                        .len(),
                    output.len(),
                    "{suite_name}/{name} cascades output count"
                );
                (input.len(), output.len())
            })
            .collect()
    }
}
