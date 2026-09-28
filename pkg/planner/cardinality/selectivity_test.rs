// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 选择率相关辅助逻辑的单元测试。
//
// 覆盖 StatsNode 类型排序、索引前缀查找、贪心覆盖、常量列识别、
// out-of-range 启发式、MV Index 路径选择率以及最后桶末值判定。

use crate::*;

/// 轻量 CardinalityContext：只提供 SessionVars，表达式/Range 路径会 panic。
struct TestContext {
    vars: variable::SessionVars,
}

impl Default for TestContext {
    fn default() -> Self {
        Self {
            vars: variable::SessionVars::default(),
        }
    }
}

impl CardinalityContext for TestContext {
    fn GetSessionVars(&self) -> &variable::SessionVars {
        &self.vars
    }

    fn GetExprCtx(&self) -> &dyn planctx_dependency::exprctx::ExprContext {
        panic!("this estimator test does not evaluate expressions")
    }

    fn GetRangerCtx(&self) -> &planctx_dependency::rangerctx::RangerContext<'_> {
        panic!("this estimator test does not build ranges")
    }
}

/// 构造用于贪心算法测试的 StatsNode。
fn stats_node(
    id: i64,
    node_type: i32,
    mask: i64,
    selectivity: f64,
    num_columns: usize,
) -> StatsNode {
    StatsNode {
        ID: id,
        Tp: node_type,
        mask,
        Selectivity: selectivity,
        numCols: num_columns,
        ..Default::default()
    }
}

/// 包装 StatsNode 为贪心候选，并设置当前覆盖位数。
fn choice(node: StatsNode, cover_count: i32) -> statsNodeForGreedyChoice {
    statsNodeForGreedyChoice {
        StatsNode: node,
        idx: 0,
        coverCount: cover_count,
    }
}

/// 浮点近似相等断言，容忍 1e-12 绝对误差。
fn assert_close(expected: f64, actual: f64) {
    assert!(
        (expected - actual).abs() <= 1e-12,
        "expected {expected}, got {actual}"
    );
}

/// 构造整型列表达式盒，列 ID 与 UniqueID 相同。
fn column_expression(id: i64) -> expression::ExprBox {
    Box::new(expression::Column::new(
        *types::NewFieldType(mysql::TypeLonglong),
        id,
        id,
        0,
    ))
}

/// 构造整型列元数据，可分别指定 schema ID 与 UniqueID。
fn column(id: i64, unique_id: i64) -> expression::Column {
    expression::Column::new(*types::NewFieldType(mysql::TypeLonglong), id, unique_id, 0)
}

/// 构造 int64 常量表达式盒。
fn constant_expression(value: i64) -> expression::ExprBox {
    Box::new(expression::NewInt64Const(value))
}

#[test]
/// 核对 compareType 与 Go 对列/索引/主键的相对排序一致。
fn test_compare_type_preserves_go_ordering() {
    crate::main_test::setup_for_cardinality_test();

    assert_eq!(0, compareType(IndexType, IndexType));
    assert_eq!(0, compareType(PkType, PkType));
    assert_eq!(0, compareType(ColType, ColType));
    assert_eq!(-1, compareType(ColType, IndexType));
    assert_eq!(-1, compareType(ColType, PkType));
    assert_eq!(1, compareType(IndexType, ColType));
    assert_eq!(1, compareType(PkType, ColType));
    assert_eq!(-1, compareType(IndexType, PkType));
    assert_eq!(1, compareType(PkType, IndexType));
}

#[test]
/// 核对索引列 UniqueID 前缀查找：完整前缀、遇缺口截断、首列缺失。
fn test_find_prefix_of_index() {
    crate::main_test::setup_for_cardinality_test();

    let columns = vec![column(20, 2), column(10, 1), column(40, 4)];

    let prefix = findPrefixOfIndex(&columns, &[1, 2, 4]);
    assert_eq!(
        vec![1, 2, 4],
        prefix.iter().map(|c| c.UniqueID).collect::<Vec<_>>()
    );

    let stopped_at_gap = findPrefixOfIndex(&columns, &[1, 3, 4]);
    assert_eq!(
        vec![1],
        stopped_at_gap
            .iter()
            .map(|c| c.UniqueID)
            .collect::<Vec<_>>()
    );

    assert!(findPrefixOfIndex(&columns, &[3, 1]).is_empty());
    assert!(findPrefixOfIndex(&columns, &[]).is_empty());
}

#[test]
/// 贪心覆盖应稳定选出覆盖最多且排序优先的集合（与 Go 用例对齐）。
fn test_selectivity_greedy_algo_matches_go_case() {
    crate::main_test::setup_for_cardinality_test();

    let mut nodes = vec![
        stats_node(1, ColType, 0b0011, 0.0, 2),
        stats_node(2, ColType, 0b0101, 0.0, 2),
        stats_node(3, ColType, 0b1001, 0.0, 2),
    ];
    let used_sets = GetUsableSetsByGreedy(&mut nodes);
    assert_eq!(1, used_sets.len());
    assert_eq!(1, used_sets[0].ID);

    nodes.swap(0, 1);
    let used_sets = GetUsableSetsByGreedy(&mut nodes);
    assert_eq!(1, used_sets.len());
    assert_eq!(
        1, used_sets[0].ID,
        "selection must be stable across input order"
    );
}

#[test]
/// 互不重叠的 mask 应全部被选中，且最终 mask 无交集。
fn test_selectivity_greedy_uses_all_disjoint_sets() {
    crate::main_test::setup_for_cardinality_test();

    let mut nodes = vec![
        stats_node(10, IndexType, 0b0011, 0.05, 2),
        stats_node(11, IndexType, 0b1100, 0.10, 2),
        stats_node(12, IndexType, 0b0001, 0.01, 1),
    ];
    let used_sets = GetUsableSetsByGreedy(&mut nodes);
    assert_eq!(
        vec![10, 11],
        used_sets.iter().map(|node| node.ID).collect::<Vec<_>>()
    );
    assert_eq!(0, used_sets[0].mask & used_sets[1].mask);
}

#[test]
/// 逐条验证 isBetterThan 的六级优先级链。
fn test_greedy_choice_priority_chain() {
    crate::main_test::setup_for_cardinality_test();

    let column = choice(stats_node(1, ColType, 1, 0.01, 1), 1);
    let index = choice(stats_node(2, IndexType, 1, 0.90, 8), 1);
    assert!(index.isBetterThan(&column));

    let one_bit = choice(stats_node(3, IndexType, 1, 0.01, 1), 1);
    let two_bits = choice(stats_node(4, IndexType, 3, 0.90, 8), 2);
    assert!(two_bits.isBetterThan(&one_bit));

    let mut partial = stats_node(5, IndexType, 3, 0.01, 1);
    partial.partCover = true;
    let complete = stats_node(6, IndexType, 3, 0.90, 8);
    assert!(choice(complete, 2).isBetterThan(&choice(partial, 2)));

    let mut fewer_access = stats_node(7, IndexType, 3, 0.01, 1);
    fewer_access.minAccessCondsForDNFCond = 1;
    let mut more_access = stats_node(8, IndexType, 3, 0.90, 8);
    more_access.minAccessCondsForDNFCond = 2;
    assert!(choice(more_access, 2).isBetterThan(&choice(fewer_access, 2)));

    let narrow = stats_node(9, IndexType, 3, 0.90, 1);
    let wide = stats_node(10, IndexType, 3, 0.01, 2);
    assert!(choice(narrow, 2).isBetterThan(&choice(wide, 2)));

    let selective = stats_node(11, IndexType, 3, 0.01, 2);
    let unselective = stats_node(12, IndexType, 3, 0.10, 2);
    assert!(choice(selective, 2).isBetterThan(&choice(unselective, 2)));
}

#[test]
/// 两侧分别为列与常量时应返回列 ID，其它形态返回 unknownColumnID。
fn test_get_constant_column_id() {
    crate::main_test::setup_for_cardinality_test();

    assert_eq!(
        7,
        getConstantColumnID(&[column_expression(7), constant_expression(1)])
    );
    assert_eq!(
        9,
        getConstantColumnID(&[constant_expression(1), column_expression(9)])
    );
    assert_eq!(
        unknownColumnID,
        getConstantColumnID(&[column_expression(1)])
    );
    assert_eq!(
        unknownColumnID,
        getConstantColumnID(&[constant_expression(1), constant_expression(2)])
    );
    assert_eq!(
        unknownColumnID,
        getConstantColumnID(&[column_expression(1), column_expression(2)])
    );
}

#[test]
/// out-of-range 等值选择率：无增量行、NDV 下限与增量裁剪。
fn test_out_of_range_equal_selectivity() {
    crate::main_test::setup_for_cardinality_test();
    let context = TestContext::default();

    assert_close(0.0, outOfRangeEQSelectivity(&context, 100, 1_000, 1_000));
    assert_close(0.01, outOfRangeEQSelectivity(&context, 10, 1_100, 1_000));
    assert_close(
        0.001,
        outOfRangeEQSelectivity(&context, 1_000, 1_100, 1_000),
    );
    assert_close(0.001, outOfRangeEQSelectivity(&context, 100, 1_001, 1_000));
}

#[test]
/// TopN 覆盖全部 NDV 时未命中行数估算的边界分支。
fn test_out_of_range_full_ndv() {
    crate::main_test::setup_for_cardinality_test();

    assert_close(0.0, outOfRangeFullNDV(10.0, 100.0, 100.0, 200.0, 1.0, 0));
    assert_close(1.0, outOfRangeFullNDV(10.0, 100.0, 100.0, 150.0, 1.0, 50));
    assert_close(
        9.0,
        outOfRangeFullNDV(100.0, 100.0, 100.0, 1_000.0, 1.0, 900),
    );
    assert_close(
        6.0,
        outOfRangeFullNDV(100.0, 1_000.0, 800.0, 600.0, 1.0, 400),
    );
    assert_close(1.0, outOfRangeFullNDV(10.0, 100.0, 100.0, 200.0, 2.0, 100));
    assert_close(1.0, outOfRangeFullNDV(0.0, 100.0, 0.0, 200.0, 1.0, 100));
}

#[test]
/// MV Index partial path：intersection 乘法与 union 容斥公式。
fn test_mv_index_path_selectivity_intersection_and_union() {
    crate::main_test::setup_for_cardinality_test();

    let coll = statistics::NewHistColl(1, 100, 0, 0, 0);
    let first = planutil::AccessPath {
        IsIntHandlePath: true,
        CountAfterAccess: 25.0,
        ..Default::default()
    };
    let second = planutil::AccessPath {
        IsCommonHandlePath: true,
        CountAfterAccess: 50.0,
        ..Default::default()
    };

    assert_close(
        0.125,
        CalcTotalSelectivityForMVIdxPath(&coll, &[&first, &second], true),
    );
    assert_close(
        0.625,
        CalcTotalSelectivityForMVIdxPath(&coll, &[&first, &second], false),
    );
}

#[test]
/// CountAfterAccess 越界时应夹紧到 [0,1]，空路径有确定默认值。
fn test_mv_index_path_selectivity_clamps_counts() {
    crate::main_test::setup_for_cardinality_test();

    let coll = statistics::NewHistColl(1, 100, 0, 0, 0);
    let above_total = planutil::AccessPath {
        IsIntHandlePath: true,
        CountAfterAccess: 120.0,
        ..Default::default()
    };
    let below_zero = planutil::AccessPath {
        IsIntHandlePath: true,
        CountAfterAccess: -10.0,
        ..Default::default()
    };

    assert_close(
        0.0,
        CalcTotalSelectivityForMVIdxPath(&coll, &[&above_total, &below_zero], true),
    );
    assert_close(
        1.0,
        CalcTotalSelectivityForMVIdxPath(&coll, &[&above_total, &below_zero], false),
    );
    assert_close(1.0, CalcTotalSelectivityForMVIdxPath(&coll, &[], true));
    assert_close(0.0, CalcTotalSelectivityForMVIdxPath(&coll, &[], false));
}

#[test]
/// 最后桶末值低估启发式：仅在新增行足够且 Repeat 过小时触发。
fn test_last_bucket_end_value_heuristic() {
    crate::main_test::setup_for_cardinality_test();
    let context = TestContext::default();
    let field_type = types::NewFieldType(mysql::TypeLonglong);
    let mut histogram = statistics::NewHistogram(1, 11, 0, 0, &field_type, 2, 0);
    histogram.AppendBucketWithNDV(
        &types::NewIntDatum(1),
        &types::NewIntDatum(10),
        1_000,
        100,
        10,
    );
    histogram.AppendBucketWithNDV(
        &types::NewIntDatum(11),
        &types::NewIntDatum(11),
        1_001,
        1,
        1,
    );

    assert!(IsLastBucketEndValueUnderrepresented(
        &context,
        &histogram,
        types::NewIntDatum(11),
        1.0,
        11.0,
        1_101,
        100,
    ));
    assert!(!IsLastBucketEndValueUnderrepresented(
        &context,
        &histogram,
        types::NewIntDatum(11),
        1.0,
        11.0,
        1_101,
        0,
    ));
    assert!(!IsLastBucketEndValueUnderrepresented(
        &context,
        &histogram,
        types::NewIntDatum(10),
        1.0,
        11.0,
        1_101,
        100,
    ));
    assert!(!IsLastBucketEndValueUnderrepresented(
        &context,
        &histogram,
        types::NewIntDatum(11),
        50.0,
        11.0,
        1_101,
        100,
    ));
    assert!(!IsLastBucketEndValueUnderrepresented(
        &context,
        &histogram,
        types::NewIntDatum(11),
        1.0,
        11.0,
        1_010,
        10,
    ));
}

#[test]
/// 默认 StatsNode 不应声称已覆盖任何谓词位。
fn test_stats_node_default_does_not_claim_predicate_coverage() {
    crate::main_test::setup_for_cardinality_test();

    let node = StatsNode::default();
    assert_eq!(0, node.mask);
    assert_eq!(0, node.numCols);
    assert!(!node.partCover);
    assert!(node.Ranges.is_empty());
}

#[test]
/// 伪统计唯一键判断必须保留 Go 的 UniqueKeyFlag 语义，而不只识别主键。
fn test_pseudo_unique_key_uses_unique_key_flag() {
    crate::main_test::setup_for_cardinality_test();

    let mut field_type = *types::NewFieldType(mysql::TypeLonglong);
    field_type.AddFlag(mysql::UniqueKeyFlag);
    let unique_info = statistics::ColumnInfo {
        ID: 1,
        Name: "unique_col".to_owned(),
        FieldType: field_type,
        IsPrimaryKey: false,
    };
    assert!(crate::pseudo::pseudoColumnHasUniqueKey(&unique_info));

    let primary_info = statistics::ColumnInfo {
        IsPrimaryKey: true,
        ..unique_info.clone()
    };
    assert!(crate::pseudo::pseudoColumnHasUniqueKey(&primary_info));

    let ordinary_info = statistics::ColumnInfo {
        FieldType: *types::NewFieldType(mysql::TypeLonglong),
        IsPrimaryKey: false,
        ..unique_info
    };
    assert!(!crate::pseudo::pseudoColumnHasUniqueKey(&ordinary_info));
}

#[test]
/// 统计过滤估算应把实际表达式树中的列索引归一化为单列 chunk 的索引 0。
fn test_prepare_filter_for_stats_evaluation_updates_expression_column() {
    crate::main_test::setup_for_cardinality_test();

    let mut col = column(1, 1);
    col.Index = 7;
    let mut filter: expression::ExprBox = Box::new(col);
    crate::selectivity::prepareFilterForStatsEvaluation(filter.as_mut());
    assert_eq!(0, filter.as_column().unwrap().Index);
}
