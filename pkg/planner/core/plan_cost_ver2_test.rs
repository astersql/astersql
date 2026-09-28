// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 代价模型 Ver2 单元测试。
//
// 验证扫描/网络/过滤/Hash Build 等代价分量公式、trace 汇总、因子缩放、
// TiFlash 表扫惩罚、IndexLookup 范围计数，以及 HashAgg 内存代价不受并发度整除。

use crate::plan_cost_ver1::PlanCostOption;
use crate::plan_cost_ver2::{
    CostVer2Factor, GetPlanCostVer2, aggCostVer2, canonical_index_join_batch_ratio,
    defaultVer2Factors, filterCostVer2, getNumberOfRanges, getTableScanPenalty, hashBuildCostVer2,
    hashProbeCostVer2, indexJoinSeekingCostVer2, netCostVer2, numFunctions, orderCostVer2,
    scanCostVer2,
};
use crate::task::{
    Expression, FieldType, PlanFlags, PlanKind, PlanNode, StatsInfo, StoreType, TaskType, TypeCode,
};

/// 构造代价选项，控制是否记录 trace 公式。
fn option(trace: bool) -> PlanCostOption {
    PlanCostOption {
        trace,
        ..Default::default()
    }
}

/// 构造仅指定函数复杂度计数的表达式。
fn expr(function_count: usize) -> Expression {
    Expression {
        function_count,
        ..Default::default()
    }
}

/// 构造带行数与平均行宽的统计信息。
fn stats(rows: f64, row_size: f64) -> StatsInfo {
    StatsInfo {
        row_count: rows,
        avg_row_size: row_size,
        histogram_row_size: None,
    }
}

/// 扫描代价 = rows * log2(row_size) * factor，并检查 trace 公式字符串。
#[test]
fn test_cost_model_ver2_scan_row_size_and_trace_formula() {
    let factor = CostVer2Factor {
        Name: "cpu",
        Value: 40.7,
    };
    let cost = scanCostVer2(&option(true), 1.0, 32.0, factor);
    assert!((cost.GetCost() - 203.5).abs() < 1e-9);
    assert_eq!(cost.traces.len(), 1);
    assert_eq!(cost.traces[0].factor, "cpu");
    assert_eq!(cost.traces[0].formula, "scan(1*log2(32))");
}

/// 多分量相加后，各 trace.value 之和应等于总代价。
#[test]
fn test_cost_model_trace_components_sum_to_total() {
    let factors = defaultVer2Factors();
    let option = option(true);
    let cost = scanCostVer2(&option, 10.0, 8.0, factors.tikv_scan)
        + netCostVer2(&option, 10.0, 8.0, factors.network)
        + filterCostVer2(&option, 10.0, &[expr(2)], factors.tidb_cpu)
        + hashBuildCostVer2(&option, 10.0, 8.0, 1.0, factors.tidb_cpu, factors.tidb_mem);
    let traced: f64 = cost.traces.iter().map(|trace| trace.value).sum();
    assert!((traced - cost.GetCost()).abs() < 1e-9);
    assert!(cost.cpu > 0.0);
    assert!(cost.memory > 0.0);
    assert!(cost.network > 0.0);
}

/// 代价因子增大则扫描代价增大，减小则代价减小。
#[test]
fn test_optimizer_cost_factors_increase_then_decrease() {
    let option = option(false);
    let low = scanCostVer2(
        &option,
        100.0,
        16.0,
        CostVer2Factor {
            Name: "cpu",
            Value: 1.0,
        },
    )
    .GetCost();
    let high = scanCostVer2(
        &option,
        100.0,
        16.0,
        CostVer2Factor {
            Name: "cpu",
            Value: 10.0,
        },
    )
    .GetCost();
    let reduced = scanCostVer2(
        &option,
        100.0,
        16.0,
        CostVer2Factor {
            Name: "cpu",
            Value: 0.1,
        },
    )
    .GetCost();
    assert!(high > low);
    assert!(reduced < low);
}

/// 表扫惩罚仅作用于 TiFlash 有序扫描；TiKV 或临时表不受罚。
#[test]
fn test_table_scan_penalty_only_targets_tiflash() {
    let mut scan = PlanNode::new(PlanKind::TableScan);
    scan.store = StoreType::TiFlash;
    scan.flags.keep_order = true;
    scan.schema = (0..25)
        .map(|_| FieldType {
            code: TypeCode::Int,
            flen: 20,
            decimal: 0,
            unsigned: false,
        })
        .collect();
    assert!(getTableScanPenalty(&scan, 100.0) > 0.0);

    scan.store = StoreType::TiKv;
    assert_eq!(getTableScanPenalty(&scan, 100.0), 0.0);
    scan.store = StoreType::TiFlash;
    scan.flags.temporary_table = true;
    assert_eq!(getTableScanPenalty(&scan, 100.0), 0.0);
}

/// 行数更多扫描更贵；IndexLookup 的 ranges 为自身与子节点 ranges 之和。
#[test]
fn test_index_lookup_ranges_and_limit_rows_affect_cost() {
    let option = option(false);
    let factor = defaultVer2Factors().tikv_scan;
    let five_rows = scanCostVer2(&option, 5.0, 48.0, factor).GetCost();
    let twenty_rows = scanCostVer2(&option, 20.0, 48.0, factor).GetCost();
    assert!(twenty_rows > five_rows);

    let child = PlanNode {
        ranges: 3,
        ..PlanNode::new(PlanKind::IndexScan)
    };
    let root = PlanNode {
        ranges: 2,
        children: vec![child],
        ..PlanNode::new(PlanKind::IndexLookupReader)
    };
    assert_eq!(getNumberOfRanges(&root), 5);
}

/// HashAgg 内存代价不随 concurrency 分摊；CPU 代价随并发下降。
#[test]
fn test_hash_agg_memory_cost_is_not_divided_by_concurrency() {
    let child = PlanNode {
        stats: stats(1_000.0, 16.0),
        ..PlanNode::new(PlanKind::TableScan)
    };
    let mut serial = PlanNode {
        stats: stats(100.0, 80.0),
        agg_funcs: vec![expr(2)],
        group_items: vec![expr(1)],
        concurrency: 1,
        children: vec![child.clone()],
        ..PlanNode::new(PlanKind::HashAgg)
    };
    let mut parallel = PlanNode {
        concurrency: 8,
        children: vec![child],
        ..serial.clone()
    };
    let serial_cost = GetPlanCostVer2(&mut serial, TaskType::Root, &option(false));
    let parallel_cost = GetPlanCostVer2(&mut parallel, TaskType::Root, &option(false));
    assert_eq!(parallel_cost.memory, serial_cost.memory);
    assert!(parallel_cost.cpu < serial_cost.cpu);
}

/// Go 只区分顶层标量函数与列/常量；嵌套函数数量不重复计费。
#[test]
fn test_order_aggregation_and_filter_cost_use_function_counts() {
    let factors = defaultVer2Factors();
    let option = option(false);
    let simple = filterCostVer2(&option, 10.0, &[expr(1)], factors.tidb_cpu).GetCost();
    let complex = filterCostVer2(&option, 10.0, &[expr(3)], factors.tidb_cpu).GetCost();
    assert_eq!(complex, simple);
    assert_eq!(
        aggCostVer2(&option, 10.0, &[expr(2)], factors.tidb_cpu).GetCost(),
        simple
    );
    assert!(orderCostVer2(&option, 100.0, 100.0, &[expr(1)], factors.tidb_cpu).GetCost() > simple);
}

/// IndexJoin 的内侧批量 lookup 只按外侧行数放大并除以 batch ratio；
/// semi join 读取多行不能再次乘以内侧平均行数。
#[test]
fn test_index_join_probe_multiplier_matches_go_batch_lookup() {
    assert_eq!(canonical_index_join_batch_ratio(), 6.0);
}

/// Go permits a zero scan cost when row width is one byte because log2(1) is zero.
#[test]
fn scan_cost_does_not_force_a_positive_logarithm() {
    let factor = CostVer2Factor {
        Name: "cpu",
        Value: 40.7,
    };
    assert_eq!(
        scanCostVer2(&option(false), 10.0, 1.0, factor).GetCost(),
        0.0
    );
}

/// Go assigns columns/constants the empirical 0.01 weight and an empty list weight zero.
#[test]
fn non_function_expression_weights_match_go() {
    assert_eq!(numFunctions(&[]), 0.0);
    assert_eq!(numFunctions(&[expr(0)]), 0.01);
    assert_eq!(numFunctions(&[expr(0), expr(2)]), 1.01);
}

/// Go charges expression evaluation separately from the comparison ordering work.
#[test]
fn order_cost_separates_scalar_expression_and_comparison_work() {
    let cpu = CostVer2Factor {
        Name: "cpu",
        Value: 2.0,
    };
    let cost = orderCostVer2(&option(false), 10.0, 4.0, &[expr(1), expr(0)], cpu);
    assert_eq!(cost.cpu, 60.0);
}

/// Go charges hash-key CPU plus one fixed build/probe CPU unit per row.
#[test]
fn hash_costs_include_go_fixed_per_row_cpu_work() {
    let cpu = CostVer2Factor {
        Name: "cpu",
        Value: 2.0,
    };
    let mem = CostVer2Factor {
        Name: "memory",
        Value: 0.0,
    };
    assert_eq!(
        hashBuildCostVer2(&option(false), 3.0, 8.0, 2.0, cpu, mem).cpu,
        18.0
    );
    assert_eq!(hashProbeCostVer2(&option(false), 3.0, 2.0, cpu).cpu, 18.0);
}

/// Go ignores seeking unless both the build cardinality and range count exceed one.
#[test]
fn index_join_seeking_threshold_and_units_match_go() {
    let scan = CostVer2Factor {
        Name: "cpu",
        Value: 2.0,
    };
    assert_eq!(
        indexJoinSeekingCostVer2(&option(false), 1.0, 8.0, scan).GetCost(),
        0.0
    );
    assert_eq!(
        indexJoinSeekingCostVer2(&option(false), 8.0, 1.0, scan).GetCost(),
        0.0
    );
    assert_eq!(
        indexJoinSeekingCostVer2(&option(false), 2.0, 3.0, scan).GetCost(),
        360.0
    );
}

/// Go sums the exact range counts; nodes with no ranges contribute zero.
#[test]
fn range_count_does_not_invent_ranges() {
    let child = PlanNode {
        ranges: 0,
        ..PlanNode::new(PlanKind::IndexScan)
    };
    let root = PlanNode {
        ranges: 0,
        children: vec![child],
        ..PlanNode::new(PlanKind::IndexLookupReader)
    };
    assert_eq!(getNumberOfRanges(&root), 0);
}
