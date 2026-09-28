// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 计划器侧统计信息（Statistics）推导辅助。
//
// 为测试与简化代价估计提供行数、选择率（Selectivity）与访问路径属性推导。
// 选择率表示谓词过滤后剩余行数占原表行数的比例；NDV（Number of Distinct
// Values）表示列或分组的不同值个数，用于基数估计。

use crate::{PlanKind, PlanNode};

/// 算子输出的统计摘要：行数、列 NDV 与分组 NDV。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatsInfo {
    /// 估计输出行数。
    pub row_count: f64,
    /// 各输出列的不同值个数估计。
    pub column_ndv: Vec<f64>,
    /// 分组键列下标列表及其 NDV。
    pub group_ndv: Vec<(Vec<usize>, f64)>,
}

/// 统计推导所需的访问路径属性。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatsAccessPath {
    /// 是否为表扫描路径；表路径按访问后行数计算选择率。
    pub table_path: bool,
    /// 是否为 IndexMerge 路径；IndexMerge 同样按访问后行数计算选择率。
    pub partial_index_paths: Vec<StatsAccessPath>,
    /// hint 是否强制保留此路径。
    pub forced: bool,
    /// range 访问后的估计行数。
    pub count_after_access: f64,
    /// 普通索引过滤后的估计行数。
    pub count_after_index: f64,
}

/// DataSource（数据源）算子的统计：表行数、选择率、访问路径与派生 StatsInfo。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DataSourceStats {
    /// 表级行数估计。
    pub table_rows: f64,
    /// 谓词选择率。
    pub selectivity: f64,
    /// 可选访问路径（索引/全表扫描等）。
    pub paths: Vec<StatsAccessPath>,
    /// 派生后的统计摘要。
    pub stats: StatsInfo,
}

/// 自底向上递归推导计划节点行数（测试用简化公式），返回统计与是否变更。
pub fn RecursiveDeriveStats4Test(plan: &mut PlanNode) -> (StatsInfo, bool) {
    let child_stats = plan
        .children
        .iter_mut()
        .map(RecursiveDeriveStats4Test)
        .map(|v| v.0)
        .collect::<Vec<_>>();
    // 按算子类型用启发式公式估计行数。
    let rows = match &plan.kind {
        PlanKind::TableScan { .. } | PlanKind::DataSource { .. } => plan.estimated_rows.max(1.0),
        // 每个过滤条件乘以 0.8 的默认选择率。
        PlanKind::Selection { conditions } => {
            child_stats.first().map_or(1.0, |s| s.row_count) * 0.8_f64.powi(conditions.len() as i32)
        }
        PlanKind::Limit { offset, count } => child_stats.first().map_or(0.0, |s| {
            (s.row_count - *offset as f64).max(0.0).min(*count as f64)
        }),
        // Join 行数 ≈ 子节点行数乘积 × 0.1。
        PlanKind::Join { .. } | PlanKind::HashJoin { .. } => {
            child_stats
                .iter()
                .map(|s| s.row_count)
                .product::<f64>()
                .max(1.0)
                * 0.1
        }
        // 聚合输出行数 ≈ sqrt(输入行数)。
        PlanKind::Aggregation { .. } | PlanKind::HashAgg | PlanKind::StreamAgg => child_stats
            .first()
            .map_or(1.0, |s| s.row_count.sqrt().max(1.0)),
        _ => child_stats
            .first()
            .map_or(plan.estimated_rows, |s| s.row_count),
    };
    let changed = (plan.estimated_rows - rows).abs() > f64::EPSILON;
    plan.estimated_rows = rows;
    (
        StatsInfo {
            row_count: rows,
            ..Default::default()
        },
        changed,
    )
}

/// 截断每个 range 边界描述，仅保留前 `keep` 个元素（用于 EXPLAIN 展示）。
pub fn pruneEstimateRange(ranges: &[Vec<String>], keep: usize) -> Vec<Vec<String>> {
    ranges
        .iter()
        .map(|range| range.iter().take(keep).cloned().collect())
        .collect()
}

/// 从访问路径汇总一般属性：最小选择率，以及是否存在强制路径。
pub fn getGeneralAttributesFromPaths(paths: &[StatsAccessPath], total_rows: f64) -> (f64, bool) {
    let mut min_selectivity = 1.0_f64;
    let mut index_force = false;
    for path in paths {
        if total_rows > 0.0 {
            let count = if path.table_path || !path.partial_index_paths.is_empty() {
                path.count_after_access
            } else {
                path.count_after_index
            };
            min_selectivity = min_selectivity.min(count / total_rows);
        }
        if !index_force && path.forced {
            index_force = true;
        }
    }
    (min_selectivity, index_force)
}

/// 按过滤条件个数更新 DataSource 选择率与行数（每条件默认选择率 0.8）。
pub fn deriveStatsByFilter(source: &mut DataSourceStats, conditions: usize) -> StatsInfo {
    source.selectivity = 0.8_f64.powi(conditions as i32);
    source.stats.row_count = source.table_rows * source.selectivity;
    source.stats.clone()
}
