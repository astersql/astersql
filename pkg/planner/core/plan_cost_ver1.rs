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

// 代价模型 Ver1：基于物理计划树估算执行代价。
//
// 代价模型（cost model）用数值衡量扫描、网络、CPU、内存等资源开销，供优化器
// 比较候选执行计划。本文件含与 Go 对齐的 canonical 物理计划入口，以及基于
// `PlanNode` 的 Ver1 分算子代价计算。

use base::{PhysicalPlan as _, Plan as _};
use base_dependency as base;
use physicalop_dependency as physicalop;

const DISTINCT_FACTOR: f64 = 0.8;

struct CardinalityContextAdapter<'a>(&'a dyn base::PlanContext);

impl cardinality_dependency::CardinalityContext for CardinalityContextAdapter<'_> {
    fn GetSessionVars(&self) -> &variable_dependency::session::SessionVars {
        self.0.GetSessionVars()
    }

    fn GetExprCtx(&self) -> &dyn expression_dependency::exprctx::ExprContext {
        self.0.GetExprCtx()
    }

    fn GetRangerCtx(&self) -> &base::RangerContext<'_> {
        self.0.GetRangerCtx()
    }
}

/// 估算计划输出行宽（字节）；伪统计按 Go 的每列 8 字节估算。
fn canonical_row_size(plan: &dyn base::PhysicalPlan) -> f64 {
    if let Some(scan) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalTableScan>()
    {
        if scan.stats_info().StatsVersion == statistics_dependency::PseudoVersion {
            let columns = if scan.StoreType == kv_dependency::StoreType::TiKV {
                scan.Columns.len().max(scan.schema().Len())
            } else {
                scan.schema().Len()
            };
            let key_size = if scan.StoreType == kv_dependency::StoreType::TiKV {
                // Go's GetTableAvgRowSize adds the record-key prefix and
                // removes the row ID already present in the TiKV columns.
                11.0 // RecordRowKeyLen (19) minus row ID (8)
            } else if scan.StoreType == kv_dependency::StoreType::TiFlash
                && !scan
                    .schema()
                    .Columns
                    .iter()
                    .any(|column| column.ID == expression_dependency::model::ExtraHandleID)
            {
                8.0
            } else {
                0.0
            };
            return (columns as f64 * 8.0 + key_size).max(1.0);
        }
        if !scan.IsMPPOrBatchCop
            && let Some(table) = scan.Table.as_ref()
        {
            return table
                .Columns
                .iter()
                .filter(|column| column.ID != expression_dependency::model::ExtraCommitTSID)
                .map(|column| {
                    let declared = column.FieldType.GetFlen();
                    if declared > 0 { declared as f64 } else { 8.0 }
                })
                .sum::<f64>()
                .max(1.0);
        }
        return scan.GetScanRowSize().max(1.0);
    }
    if let Some(scan) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalIndexScan>()
    {
        return scan.GetScanRowSize().max(1.0);
    }
    plan.schema()
        .Columns
        .iter()
        .map(|column| {
            let declared = column.RetType.as_ref().map_or(-1, |field| field.GetFlen());
            if declared > 0 { declared as f64 } else { 8.0 }
        })
        .sum::<f64>()
        .max(1.0)
}

/// 递归累计扫描 Ranges 的 seek 代价（每 range 约 20）。
fn canonical_seek_cost(plan: &dyn base::PhysicalPlan) -> f64 {
    let own = if let Some(scan) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalTableScan>()
    {
        scan.Ranges.len().max(1) as f64 * 20.0
    } else if let Some(scan) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalIndexScan>()
    {
        scan.Ranges.len().max(1) as f64 * 20.0
    } else {
        0.0
    };
    own + plan
        .children()
        .into_iter()
        .map(canonical_seek_cost)
        .sum::<f64>()
}

/// 读取 DistSQL 扫描并发度会话变量，缺省用默认值。
fn canonical_dist_sql_scan_concurrency(plan: &dyn base::PhysicalPlan) -> f64 {
    plan.s_ctx()
        .GetSessionVars()
        .GetSystemVar(vardef_dependency::TiDBDistSQLScanConcurrency)
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(vardef_dependency::DefDistSQLScanConcurrency)
        .max(1) as f64
}

/// 读取网络代价因子；与 Go `GetNetworkFactor(nil)` 的全局回退一致。
fn canonical_network_factor(plan: &dyn base::PhysicalPlan) -> f64 {
    plan.s_ctx()
        .GetSessionVars()
        .GetSystemVar(vardef_dependency::TiDBOptNetworkFactor)
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(vardef_dependency::DefOptNetworkFactor)
}

/// 按任务类型映射子任务（MPP/CopMulti/CopSingle），递归求子计划代价之和。
fn canonical_children_cost(
    plan: &dyn base::PhysicalPlan,
    task: property_dependency::TaskType,
    option: &costusage_dependency::PlanCostOption,
) -> Result<f64, expression_dependency::Error> {
    let child_task = if task == property_dependency::MppTaskType {
        task
    } else if plan.as_any().is::<physicalop::PhysicalIndexLookUpReader>() {
        property_dependency::CopMultiReadTaskType
    } else if plan.as_any().is::<physicalop::PhysicalTableReader>()
        || plan.as_any().is::<physicalop::PhysicalIndexReader>()
    {
        property_dependency::CopSingleReadTaskType
    } else {
        task
    };
    plan.children()
        .into_iter()
        .map(|child| GetCanonicalPlanCostVer1(child, child_task, option))
        .sum()
}

/// Go-equivalent v1 cost entry for the real physical-plan hierarchy.
/// 与 Go 对齐的 v1 代价入口，面向真实物理计划层次结构。
pub fn GetCanonicalPlanCostVer1(
    plan: &dyn base::PhysicalPlan,
    task: property_dependency::TaskType,
    option: &costusage_dependency::PlanCostOption,
) -> Result<f64, expression_dependency::Error> {
    let rows = plan.stats_count().max(0.0);
    let width = canonical_row_size(plan);
    let children = canonical_children_cost(plan, task, option)?;
    let any = plan.as_any();

    let own = if let Some(scan) = any.downcast_ref::<physicalop::PhysicalTableScan>() {
        let scan_factor = if scan.IsMPPOrBatchCop {
            0.6
        } else if scan.Desc && rows > 1_000.0 {
            1.2
        } else {
            1.0
        };
        rows * width * scan_factor
    } else if let Some(scan) = any.downcast_ref::<physicalop::PhysicalIndexScan>() {
        rows * width
    } else if any.is::<physicalop::PhysicalTableReader>()
        || any.is::<physicalop::PhysicalIndexReader>()
    {
        let table_reader = any.downcast_ref::<physicalop::PhysicalTableReader>();
        let (network_rows, network_width) =
            plan.children().first().map_or((rows, width), |child| {
                let histogram = physicalop::GetTblStats(Some(*child));
                let fallback = statistics_dependency::NewHistColl(0, 0, 0, 0, 0);
                let histogram = histogram
                    .as_deref()
                    .and_then(|value| value.downcast_ref::<statistics_dependency::HistColl>())
                    .unwrap_or(&fallback);
                let columns = child.schema().Columns.iter().collect::<Vec<_>>();
                let row_size = cardinality_dependency::GetAvgRowSize(
                    &CardinalityContextAdapter(plan.s_ctx().as_ref()),
                    histogram,
                    &columns,
                    any.is::<physicalop::PhysicalIndexReader>(),
                    false,
                );
                (child.stats_count().max(0.0), row_size)
            });
        let mut cost = (children + network_rows * network_width + canonical_seek_cost(plan))
            / canonical_dist_sql_scan_concurrency(plan);
        fn contains_avg_partial(plan: &dyn base::PhysicalPlan) -> bool {
            let aggregate = plan
                .as_any()
                .downcast_ref::<physicalop::PhysicalHashAgg>()
                .map(|agg| &agg.BasePhysicalAgg)
                .or_else(|| {
                    plan.as_any()
                        .downcast_ref::<physicalop::PhysicalStreamAgg>()
                        .map(|agg| &agg.BasePhysicalAgg)
                });
            aggregate.is_some_and(|agg| {
                agg.GroupByItems.is_empty()
                    && agg
                        .AggFuncs
                        .iter()
                        .any(|function| function.Name == crate::ast::AggFuncCount)
                    && agg
                        .AggFuncs
                        .iter()
                        .any(|function| function.Name == crate::ast::AggFuncSum)
            }) || plan.children().into_iter().any(contains_avg_partial)
        }
        if table_reader.is_some_and(|reader| {
            reader.StoreType == kv_dependency::StoreType::TiFlash
                && reader.ReadReqType == physicalop::ReadReqType::BatchCop
        }) && plan.children().into_iter().any(contains_avg_partial)
        {
            cost *= 0.4;
        }
        return Ok(cost);
    } else if let Some(reader) = any.downcast_ref::<physicalop::PhysicalIndexLookUpReader>() {
        let workers = 4.0;
        let batch = (rows.max(1.0)).min(20_000.0);
        let ordering = if reader.KeepOrder && batch > 2.0 {
            rows * batch.log2() / workers
        } else {
            0.0
        };
        return Ok((children + rows * width + canonical_seek_cost(plan))
            / canonical_dist_sql_scan_concurrency(plan)
            + rows * 5.0
            + ordering
            + 120.0);
    } else if let Some(projection) = any.downcast_ref::<physicalop::PhysicalProjection>() {
        rows * projection.Exprs.len().max(1) as f64 / 4.0 + 15.0
    } else if let Some(selection) = any.downcast_ref::<physicalop::PhysicalSelection>() {
        if selection.FromDataSource {
            0.0
        } else {
            plan.children()
                .first()
                .map_or(rows, |child| child.stats_count().max(0.0))
                * if task == property_dependency::RootTaskType
                    || task == property_dependency::MppTaskType
                {
                    plan.s_ctx().GetSessionVars().GetCPUFactor()
                } else {
                    plan.s_ctx().GetSessionVars().GetCopCPUFactor()
                }
        }
    } else if any.is::<physicalop::PhysicalSort>() {
        let child_rows = plan
            .children()
            .first()
            .map_or(rows, |child| child.stats_count().max(0.0));
        child_rows * child_rows.max(2.0).log2() * 3.0 + child_rows * 0.001
    } else if let Some(top_n) = any.downcast_ref::<physicalop::PhysicalTopN>() {
        let input_rows = plan
            .children()
            .first()
            .map_or(rows, |child| child.stats_count().max(0.0));
        let heap_size = top_n.Offset.saturating_add(top_n.Count).max(2) as f64;
        let variables = plan.s_ctx().GetSessionVars();
        let cpu_factor = if task == property_dependency::RootTaskType {
            variables.GetCPUFactor()
        } else {
            variables.GetCopCPUFactor()
        };
        input_rows * heap_size.log2() * cpu_factor + heap_size * variables.GetMemoryFactor()
    } else if let Some(hash) = any.downcast_ref::<physicalop::PhysicalHashAgg>() {
        let input_rows = plan
            .children()
            .first()
            .map_or(rows, |child| child.stats_count().max(0.0));
        let distinct = hash.BasePhysicalAgg.NumDistinctFunc();
        let factor = hash
            .BasePhysicalAgg
            .GetAggFuncCostFactor(task == property_dependency::MppTaskType);
        let vars = plan.s_ctx().GetSessionVars();
        let cpu_factor = if task == property_dependency::RootTaskType {
            vars.GetCPUFactor()
        } else {
            vars.GetCopCPUFactor()
        };
        let mut cpu = input_rows * cpu_factor * factor;
        // HashAgg：无 DISTINCT 时按 final/partial 并发分摊 CPU，并加并发调度开销。
        if task == property_dependency::RootTaskType && distinct == 0 {
            let concurrency = |name: &str| {
                vars.GetSystemVar(name)
                    .and_then(|value| value.parse::<f64>().ok())
                    .filter(|value| *value > 0.0)
                    .unwrap_or(vardef_dependency::DefExecutorConcurrency as f64)
            };
            let final_concurrency = concurrency(vardef_dependency::TiDBHashAggFinalConcurrency);
            let partial_concurrency = concurrency(vardef_dependency::TiDBHashAggPartialConcurrency);
            if final_concurrency != 1.0 || partial_concurrency != 1.0 {
                cpu /= final_concurrency.min(partial_concurrency);
                let concurrency_factor = vars
                    .GetSystemVar(vardef_dependency::TiDBOptConcurrencyFactor)
                    .and_then(|value| value.parse::<f64>().ok())
                    .unwrap_or(vardef_dependency::DefOptConcurrencyFactor);
                cpu += (final_concurrency + partial_concurrency + 1.0) * concurrency_factor;
            }
        }
        let memory_factor = vars.GetMemoryFactor();
        let memory = rows * memory_factor * hash.BasePhysicalAgg.AggFuncs.len() as f64
            + input_rows * DISTINCT_FACTOR * memory_factor * distinct as f64;
        cpu + memory
    } else if let Some(stream) = any.downcast_ref::<physicalop::PhysicalStreamAgg>() {
        let input_rows = plan
            .children()
            .first()
            .map_or(rows, |child| child.stats_count().max(0.0));
        let vars = plan.s_ctx().GetSessionVars();
        let cpu_factor = if task == property_dependency::RootTaskType {
            vars.GetCPUFactor()
        } else {
            vars.GetCopCPUFactor()
        };
        let cpu = input_rows * cpu_factor * stream.BasePhysicalAgg.GetAggFuncCostFactor(false);
        let rows_per_group = input_rows / rows.max(1.0);
        cpu + rows_per_group
            * DISTINCT_FACTOR
            * vars.GetMemoryFactor()
            * stream.BasePhysicalAgg.NumDistinctFunc() as f64
    } else if let Some(join) = any.downcast_ref::<physicalop::PhysicalHashJoin>() {
        let child_rows = plan
            .children()
            .into_iter()
            .map(|child| child.stats_count().max(0.0))
            .collect::<Vec<_>>();
        let left = child_rows.first().copied().unwrap_or_default();
        let right = child_rows.get(1).copied().unwrap_or_default();
        let (build, _probe) = if join.RightIsBuildSide() {
            (right, left)
        } else {
            (left, right)
        };
        let variables = plan.s_ctx().GetSessionVars();
        let workers = join.Concurrency.max(1) as f64;
        let concurrency_factor = variables
            .GetSystemVar(vardef_dependency::TiDBOptConcurrencyFactor)
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(vardef_dependency::DefOptConcurrencyFactor);
        build * variables.GetCPUFactor()
            + build * variables.GetMemoryFactor()
            + rows * variables.GetCPUFactor() / workers
            + if join.BasePhysicalJoin.JoinType == base::JoinType::FullOuterJoin {
                build * variables.GetCPUFactor() / workers
            } else {
                0.0
            }
            + (workers + 1.0) * concurrency_factor
    } else if any.is::<physicalop::PhysicalExchangeSender>() {
        0.0
    } else if any.is::<physicalop::PhysicalExchangeReceiver>() {
        let child_rows = plan
            .children()
            .first()
            .map_or(rows, |child| child.stats_count().max(0.0));
        child_rows * canonical_network_factor(plan)
    } else if plan.children().is_empty() {
        rows.max(1.0)
    } else {
        0.0
    };
    Ok(children + own)
}

use crate::task::{
    PlanKind, PlanNode, StoreType, TaskType, accumulateNetSeekCost4MPP, collectRowSizeFromMPPPlan,
};

/// 代价模型版本号：Ver1。
pub const modelVer1: i32 = 1;
/// 代价模型版本号：Ver2。
pub const modelVer2: i32 = 2;
/// 代价标志：强制重新计算（忽略缓存的 cost_v1）。
pub const CostFlagRecalculate: u64 = 1;
/// 代价标志：使用真实基数而非估计行数。
pub const CostFlagUseTrueCardinality: u64 = 2;

#[derive(Clone, Debug)]
/// Ver1 各类资源代价因子与并发度配置。
pub struct CostFactors {
    pub cpu: f64,
    pub memory: f64,
    pub disk: f64,
    pub network: f64,
    pub seek: f64,
    pub request: f64,
    pub concurrency: f64,
    pub scan: f64,
    pub desc_scan: f64,
    pub tiflash_scan: f64,
    pub temporary_scan: f64,
    pub dist_sql_concurrency: usize,
    pub projection_concurrency: usize,
    pub hash_join_concurrency: usize,
    pub index_lookup_concurrency: usize,
    pub index_lookup_size: usize,
}
impl Default for CostFactors {
    fn default() -> Self {
        Self {
            cpu: 1.0,
            memory: 0.2,
            disk: 1.5,
            network: 1.0,
            seek: 20.0,
            request: 8.0,
            concurrency: 3.0,
            scan: 1.0,
            desc_scan: 1.2,
            tiflash_scan: 0.6,
            temporary_scan: 0.0,
            dist_sql_concurrency: 15,
            projection_concurrency: 4,
            hash_join_concurrency: 5,
            index_lookup_concurrency: 4,
            index_lookup_size: 20_000,
        }
    }
}

#[derive(Clone, Debug, Default)]
/// 代价计算选项：标志位、因子表与是否记录 trace。
pub struct PlanCostOption {
    pub CostFlag: u64,
    pub factors: CostFactors,
    pub trace: bool,
}
/// 判断 cost_flag 是否包含指定标志位。
pub fn hasCostFlag(cost_flag: u64, flag: u64) -> bool {
    cost_flag & flag != 0
}
/// 取计划基数；Ver1 保留零基数，真实基数按 probe 次数均摊。
pub fn getCardinality(plan: &PlanNode, flag: u64) -> f64 {
    if hasCostFlag(flag, CostFlagUseTrueCardinality) {
        let probes = plan
            .labels
            .get("actual_probe_count")
            .copied()
            .unwrap_or(1.0);
        if probes == 0.0 {
            return 0.0;
        }
        return (getOperatorActRows(plan) / probes).max(0.0);
    }
    plan.rows()
}
/// 取第 id 个子节点基数；无则退回自身。
fn child_rows(plan: &PlanNode, id: usize, flag: u64) -> f64 {
    plan.children
        .get(id)
        .map(|p| getCardinality(p, flag))
        .unwrap_or_else(|| getCardinality(plan, flag))
}
/// 递归计算第 id 个子节点的 Ver1 代价。
fn child_cost(plan: &mut PlanNode, id: usize, task: TaskType, option: &PlanCostOption) -> f64 {
    plan.children
        .get_mut(id)
        .map(|p| GetPlanCostVer1(p, task, option))
        .unwrap_or(0.0)
}
/// 所有子节点 Ver1 代价之和。
fn all_child_cost(plan: &mut PlanNode, task: TaskType, option: &PlanCostOption) -> f64 {
    plan.children
        .iter_mut()
        .map(|p| GetPlanCostVer1(p, task, option))
        .sum()
}
/// 并发度转为 f64，至少为 1。
fn concurrency(n: usize) -> f64 {
    n.max(1) as f64
}

/// Projection 算子自身 CPU/并发开销。
pub fn getCost4PhysicalProjection(plan: &PlanNode, count: f64, option: &PlanCostOption) -> f64 {
    let n = concurrency(plan.concurrency.max(option.factors.projection_concurrency));
    count * option.factors.cpu / n + (1.0 + n) * option.factors.concurrency
}

/// IndexLookUp：索引回表、批处理排序与 keep_order 归并开销。
pub fn getCost4PhysicalIndexLookUpReader(plan: &PlanNode, option: &PlanCostOption) -> f64 {
    let index_rows = plan
        .children
        .first()
        .map(PlanNode::rows)
        .unwrap_or(plan.rows())
        .max(1.0);
    let table_rows = plan
        .children
        .get(1)
        .map(PlanNode::rows)
        .unwrap_or(plan.rows());
    let workers = concurrency(option.factors.index_lookup_concurrency);
    let batch = (option.factors.index_lookup_size as f64).min(index_rows);
    let mut cost = index_rows * option.factors.cpu + (workers + 1.0) * option.factors.concurrency;
    if plan.flags.paging && plan.expected_count.is_finite() {
        cost = cost.min(plan.expected_count * option.factors.seek);
    }
    if batch > 2.0 {
        cost += index_rows * batch.log2() * option.factors.cpu / workers;
    }
    let ordered = (batch * table_rows / index_rows).min(table_rows);
    if plan.flags.keep_order && ordered > 2.0 {
        cost += table_rows * ordered.log2() * option.factors.cpu / workers;
    }
    cost
}

/// 按临时表/TiFlash/降序扫描选择扫描因子。
fn scan_factor(plan: &PlanNode, option: &PlanCostOption) -> f64 {
    if plan.flags.temporary_table {
        option.factors.temporary_scan
    } else if plan.store == StoreType::TiFlash {
        option.factors.tiflash_scan
    } else if plan.flags.desc && plan.rows() > 1_000.0 {
        option.factors.desc_scan
    } else {
        option.factors.scan
    }
}
/// 行数 × 行宽 × 扫描因子。
fn scan_cost(plan: &PlanNode, option: &PlanCostOption) -> f64 {
    getCardinality(plan, option.CostFlag) * plan.row_size().max(1.0) * scan_factor(plan, option)
}
/// 单节点网络 seek 代价：ranges × seek 因子。
fn net_seek_cost(plan: &PlanNode, option: &PlanCostOption) -> f64 {
    plan.ranges.max(1) as f64 * option.factors.seek
}
/// 递归估计整棵子树的网络 seek 代价。
pub fn estimateNetSeekCost(plan: &PlanNode, option: &PlanCostOption) -> f64 {
    net_seek_cost(plan, option)
        + plan
            .children
            .iter()
            .map(|p| estimateNetSeekCost(p, option))
            .sum::<f64>()
}
/// 临时表网络因子为 0，否则用 network 因子。
pub fn getTableNetFactor(plan: &PlanNode, option: &PlanCostOption) -> f64 {
    if plan.flags.temporary_table {
        0.0
    } else {
        option.factors.network
    }
}

/// IndexJoin：外层驱动内层查找与请求开销。
pub fn getCost4PhysicalIndexJoin(
    plan: &PlanNode,
    outer: f64,
    inner: f64,
    outer_cost: f64,
    inner_cost: f64,
    option: &PlanCostOption,
) -> f64 {
    let batch = (option.factors.index_lookup_size as f64).min(outer.max(1.0));
    let lookup = outer * plan.ranges.max(1) as f64 * option.factors.request;
    outer_cost
        + inner_cost * outer
        + lookup
        + outer * batch.log2().max(1.0) * option.factors.cpu
        + outer * inner * option.factors.cpu
}
/// IndexHashJoin：在 IndexJoin 基础上加哈希内存与探测 CPU。
pub fn getCost4PhysicalIndexHashJoin(
    plan: &PlanNode,
    outer: f64,
    inner: f64,
    outer_cost: f64,
    inner_cost: f64,
    option: &PlanCostOption,
) -> f64 {
    getCost4PhysicalIndexJoin(plan, outer, inner, outer_cost, inner_cost, option)
        + outer * option.factors.memory
        + inner * option.factors.cpu
}
/// IndexMergeJoin：在 IndexJoin 基础上加外层排序开销。
pub fn getCost4PhysicalIndexMergeJoin(
    plan: &PlanNode,
    outer: f64,
    inner: f64,
    outer_cost: f64,
    inner_cost: f64,
    option: &PlanCostOption,
) -> f64 {
    getCost4PhysicalIndexJoin(plan, outer, inner, outer_cost, inner_cost, option)
        + outer * outer.max(2.0).log2() * option.factors.cpu
}
/// Apply：相关子查询逐行执行右子树的代价。
pub fn getCost4PhysicalApply(
    left: f64,
    right: f64,
    left_cost: f64,
    right_cost: f64,
    option: &PlanCostOption,
) -> f64 {
    left_cost + left * right_cost + left * right * option.factors.cpu
}
/// MergeJoin：左右有序归并的 CPU 代价。
pub fn getCost4PhysicalMergeJoin(left: f64, right: f64, option: &PlanCostOption) -> f64 {
    (left + right) * option.factors.cpu
}
/// HashJoin：Build 侧建表 + Probe 侧探测（可并发）。
pub fn getCost4PhysicalHashJoin(
    plan: &PlanNode,
    left: f64,
    right: f64,
    option: &PlanCostOption,
) -> f64 {
    let build = if plan.inner_child == 0 { left } else { right };
    let probe = if plan.inner_child == 0 { right } else { left };
    let workers = concurrency(plan.concurrency.max(option.factors.hash_join_concurrency));
    build * option.factors.cpu
        + build * option.factors.memory
        + probe * option.factors.cpu / workers
        + (workers + 1.0) * option.factors.concurrency
}
/// StreamAgg：按输入行与聚合/分组表达式计 CPU。
pub fn getCost4PhysicalStreamAgg(
    plan: &PlanNode,
    input: f64,
    root: bool,
    option: &PlanCostOption,
) -> f64 {
    let factor = if root {
        option.factors.cpu
    } else {
        option.factors.cpu * 0.8
    };
    input * (plan.agg_funcs.len() + plan.group_items.len()).max(1) as f64 * factor
}
/// HashAgg：聚合 CPU（MPP 可按并发分摊）+ 输出行内存。
pub fn getCost4PhysicalHashAgg(
    plan: &PlanNode,
    input: f64,
    root: bool,
    mpp: bool,
    option: &PlanCostOption,
) -> f64 {
    let cpu = if root {
        option.factors.cpu
    } else {
        option.factors.cpu * 0.8
    };
    let div = if mpp {
        concurrency(plan.concurrency)
    } else {
        1.0
    };
    input * plan.agg_funcs.len().max(1) as f64 * cpu / div + plan.rows() * option.factors.memory
}
/// Sort：n·log(n) CPU + 行宽内存。
pub fn getCost4PhysicalSort(plan: &PlanNode, count: f64, option: &PlanCostOption) -> f64 {
    let count = count.max(2.0);
    let _ = plan;
    count * count.log2() * option.factors.cpu + count * option.factors.memory
}
/// Batch Point Get：多点 seek + 网络传输。
pub fn getCost4BatchPointGetPlan(plan: &PlanNode, option: &PlanCostOption) -> f64 {
    let rows = plan.rows().max(plan.ranges as f64);
    rows * (option.factors.seek + plan.row_size() * option.factors.network)
}
/// 单点 Point Get：一次 seek + 一行网络。
pub fn getCost4PointGetPlan(plan: &PlanNode, option: &PlanCostOption) -> f64 {
    option.factors.seek + plan.row_size() * option.factors.network
}

/// IndexMergeReader：各部分扫描代价与网络，再除以 DistSQL 并发。
pub fn GetPlanCostVer14PhysicalIndexMergeReader(
    plan: &mut PlanNode,
    option: &PlanCostOption,
) -> f64 {
    let mut total = 0.0;
    for child in &mut plan.children {
        total += GetPlanCostVer1(child, TaskType::CopSingleRead, option)
            + child.rows() * child.row_size() * getTableNetFactor(child, option);
    }
    if plan.count > 0 {
        total *= 0.99;
    }
    total / concurrency(option.factors.dist_sql_concurrency)
}

/// Ver1 主入口：按算子种类分派，结果写入 plan.cost_v1。
pub fn GetPlanCostVer1(plan: &mut PlanNode, task: TaskType, option: &PlanCostOption) -> f64 {
    if let Some(cost) = plan.cost_v1 {
        if !hasCostFlag(option.CostFlag, CostFlagRecalculate) {
            return cost;
        }
    }
    let rows = getCardinality(plan, option.CostFlag);
    let root = task == TaskType::Root;
    let mpp = task == TaskType::Mpp;
    let cost = match plan.kind {
        PlanKind::Selection => {
            let input = child_rows(plan, 0, option.CostFlag);
            child_cost(plan, 0, task, option)
                + if plan.flags.from_data_source {
                    0.0
                } else {
                    input * plan.conditions.len().max(1) as f64 * option.factors.cpu
                }
        }
        PlanKind::Projection => {
            let input = child_rows(plan, 0, option.CostFlag);
            child_cost(plan, 0, task, option)
                + getCost4PhysicalProjection(
                    plan,
                    input * plan.expressions.len().max(1) as f64,
                    option,
                )
        }
        PlanKind::IndexLookupReader => {
            let children = all_child_cost(plan, TaskType::CopMultiRead, option);
            let network = plan
                .children
                .iter()
                .map(|c| {
                    c.rows() * c.row_size() * getTableNetFactor(c, option)
                        + net_seek_cost(c, option)
                })
                .sum::<f64>();
            (children + network) / concurrency(option.factors.dist_sql_concurrency)
                + getCost4PhysicalIndexLookUpReader(plan, option)
        }
        PlanKind::IndexReader => {
            let child = child_cost(plan, 0, TaskType::CopSingleRead, option);
            let p = plan.children.first().unwrap_or(plan);
            (child
                + p.rows() * p.row_size() * getTableNetFactor(p, option)
                + net_seek_cost(p, option))
                / concurrency(option.factors.dist_sql_concurrency)
        }
        PlanKind::TableReader => {
            let child_task = if plan.store == StoreType::TiFlash {
                TaskType::Mpp
            } else {
                TaskType::CopSingleRead
            };
            let child = child_cost(plan, 0, child_task, option);
            let p = plan.children.first().unwrap_or(plan);
            let size = if child_task == TaskType::Mpp {
                collectRowSizeFromMPPPlan(p)
            } else {
                p.row_size()
            };
            let seek = if child_task == TaskType::Mpp {
                accumulateNetSeekCost4MPP(p) * option.factors.seek
            } else {
                net_seek_cost(p, option)
            };
            let mut c = (child + p.rows() * size * getTableNetFactor(p, option) + seek)
                / concurrency(option.factors.dist_sql_concurrency);
            // MPP 强制执行且未要求重算时，将代价缩小以便优先选中该计划。
            if mpp && plan.flags.mpp_enforced && !hasCostFlag(option.CostFlag, CostFlagRecalculate)
            {
                c /= 1_000_000_000.0;
            }
            c
        }
        PlanKind::IndexMergeReader => GetPlanCostVer14PhysicalIndexMergeReader(plan, option),
        PlanKind::TableScan | PlanKind::IndexScan => {
            scan_cost(plan, option) + rows * plan.conditions.len() as f64 * option.factors.cpu
        }
        PlanKind::IndexJoin | PlanKind::IndexHashJoin | PlanKind::IndexMergeJoin => {
            let l = child_rows(plan, 0, option.CostFlag);
            let r = child_rows(plan, 1, option.CostFlag);
            let lc = child_cost(plan, 0, task, option);
            let rc = child_cost(plan, 1, task, option);
            match plan.kind {
                PlanKind::IndexHashJoin => {
                    getCost4PhysicalIndexHashJoin(plan, l, r, lc, rc, option)
                }
                PlanKind::IndexMergeJoin => {
                    getCost4PhysicalIndexMergeJoin(plan, l, r, lc, rc, option)
                }
                _ => getCost4PhysicalIndexJoin(plan, l, r, lc, rc, option),
            }
        }
        PlanKind::Apply => {
            let l = child_rows(plan, 0, option.CostFlag);
            let r = child_rows(plan, 1, option.CostFlag);
            let lc = child_cost(plan, 0, task, option);
            let rc = child_cost(plan, 1, task, option);
            getCost4PhysicalApply(l, r, lc, rc, option)
        }
        PlanKind::MergeJoin => {
            all_child_cost(plan, task, option)
                + getCost4PhysicalMergeJoin(
                    child_rows(plan, 0, option.CostFlag),
                    child_rows(plan, 1, option.CostFlag),
                    option,
                )
        }
        PlanKind::HashJoin => {
            all_child_cost(plan, task, option)
                + getCost4PhysicalHashJoin(
                    plan,
                    child_rows(plan, 0, option.CostFlag),
                    child_rows(plan, 1, option.CostFlag),
                    option,
                )
        }
        PlanKind::StreamAgg => {
            child_cost(plan, 0, task, option)
                + getCost4PhysicalStreamAgg(
                    plan,
                    child_rows(plan, 0, option.CostFlag),
                    root,
                    option,
                )
        }
        PlanKind::HashAgg => {
            child_cost(plan, 0, task, option)
                + getCost4PhysicalHashAgg(
                    plan,
                    child_rows(plan, 0, option.CostFlag),
                    root,
                    mpp,
                    option,
                )
        }
        PlanKind::Sort => {
            child_cost(plan, 0, task, option)
                + getCost4PhysicalSort(plan, child_rows(plan, 0, option.CostFlag), option)
        }
        PlanKind::TopN => {
            let input = child_rows(plan, 0, option.CostFlag);
            let n = (plan.offset + plan.count).max(2) as f64;
            child_cost(plan, 0, task, option)
                + input * n.log2() * option.factors.cpu
                + n * plan.row_size() * option.factors.memory
        }
        PlanKind::BatchPointGet => getCost4BatchPointGetPlan(plan, option),
        PlanKind::PointGet => getCost4PointGetPlan(plan, option),
        PlanKind::UnionAll => {
            let child_max = plan
                .children
                .iter_mut()
                .map(|child| GetPlanCostVer1(child, task, option))
                .fold(0.0, f64::max);
            let mut cost =
                child_max + (1 + plan.children.len()) as f64 * option.factors.concurrency;
            if mpp && plan.flags.mpp_enforced && !hasCostFlag(option.CostFlag, CostFlagRecalculate)
            {
                cost /= 1_000_000_000.0;
            }
            cost
        }
        PlanKind::ExchangeReceiver => {
            let child_rows = child_rows(plan, 0, option.CostFlag);
            child_cost(plan, 0, task, option) + child_rows * option.factors.network
        }
        _ => all_child_cost(plan, task, option),
    };
    plan.cost_v1 = Some(cost);
    cost
}

/// 取算子实际行数标签，缺省回退到估计行数。
pub fn getOperatorActRows(operator: &PlanNode) -> f64 {
    operator
        .labels
        .get("actual_rows")
        .copied()
        .unwrap_or_else(|| operator.rows())
}
