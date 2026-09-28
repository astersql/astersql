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

// 查询优化器（optimizer）入口的精简迁移基线。
//
// 将逻辑计划经逻辑优化规则、笛卡尔积检查、物理化与后优化阶段，
// 得到可执行的物理计划树并估算代价。执行计划描述 SQL 如何被算子树执行。

use crate::{PlanKind, PlanNode, PlannerContext, RuntimeFilterGenerator, Trace};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
/// 是否允许无等值条件的笛卡尔积 Join；默认允许。
pub static AllowCartesianProduct: AtomicBool = AtomicBool::new(true);
/// 并行度相关的初始最大核数占位常量。
pub const initialMaxCores: u64 = 10_000;
#[derive(Clone, Debug, Default)]
/// 优化开关：Cascades、运行时过滤器、并行 Apply、禁用规则与追踪。
pub struct OptimizeOptions {
    pub cascades: bool,
    pub enable_runtime_filter: bool,
    pub enable_parallel_apply: bool,
    pub disabled_rules: BTreeSet<String>,
    pub trace: bool,
}
/// 优化入口：逻辑优化 → 笛卡尔积检查 → 物理优化 → 后优化，返回计划与代价。
pub fn DoOptimize(
    ctx: &PlannerContext,
    flag: u64,
    logic: PlanNode,
    options: &OptimizeOptions,
) -> Result<(PlanNode, f64, Trace), String> {
    let mut trace = Trace::default();
    let logical = logicalOptimize(flag, logic, &options.disabled_rules, &mut trace)?;
    if !AllowCartesianProduct.load(Ordering::Relaxed) && existsCartesianProduct(&logical) {
        return Err("cartesian product is unsupported".into());
    }
    let mut physical = physicalOptimize(logical, options.cascades, &mut trace)?;
    postOptimize(ctx, &mut physical, options);
    let cost = total_cost(&physical);
    trace.final_plan = crate::ToString(&physical);
    Ok((physical, cost, trace))
}
/// 按 flag 位掩码依次应用逻辑规则（列裁剪、解相关、谓词下推、Join 重排等）。
fn logicalOptimize(
    flag: u64,
    mut plan: PlanNode,
    disabled: &BTreeSet<String>,
    trace: &mut Trace,
) -> Result<PlanNode, String> {
    let rules = [
        "column_pruning",
        "build_key_info",
        "decorrelate",
        "predicate_push_down",
        "aggregation_eliminate",
        "projection_eliminate",
        "join_reorder",
        "topn_push_down",
    ];
    for (index, rule) in rules.iter().enumerate() {
        if flag & (1 << index) != 0 && !disabled.contains(*rule) {
            trace.AppendLogical(*rule);
            normalize(&mut plan, rule);
        }
    }
    Ok(plan)
}
/// 递归应用单条规则；当前仅实现恒等投影消除的简化形态。
fn normalize(plan: &mut PlanNode, rule: &str) {
    for child in &mut plan.children {
        normalize(child, rule);
    }
    if rule == "projection_eliminate"
        && matches!(plan.kind, PlanKind::Projection)
        && plan.children.len() == 1
        && plan.operator_info.is_empty()
    {
        *plan = plan.children.remove(0);
    }
}
/// 物理优化：记录 Cascades/Volcano 路径后对计划树做物理化。
fn physicalOptimize(
    mut plan: PlanNode,
    cascades: bool,
    trace: &mut Trace,
) -> Result<PlanNode, String> {
    trace.AppendPhysical(if cascades { "cascades" } else { "volcano" });
    physicalize(&mut plan);
    Ok(plan)
}
/// 自底向上物理化；将逻辑 Join 暂映射为 HashJoin。
fn physicalize(plan: &mut PlanNode) {
    for child in &mut plan.children {
        physicalize(child);
    }
    if let PlanKind::Join { equal_conditions } = &plan.kind {
        plan.kind = PlanKind::HashJoin {
            inner_child: 1,
            equal_conditions: equal_conditions.clone(),
        };
    }
}
/// 后优化：合并连续 Selection、消除空 UnionScan/Lock，并可选并行 Apply 与运行时过滤。
fn postOptimize(_ctx: &PlannerContext, plan: &mut PlanNode, options: &OptimizeOptions) {
    mergeContinuousSelections(plan);
    eliminateUnionScanAndLock(plan);
    if options.enable_parallel_apply {
        enableParallelApply(plan);
    }
    if options.enable_runtime_filter {
        let mut generator = RuntimeFilterGenerator::default();
        generator.GenerateRuntimeFilter(plan);
        plan.operator_info
            .push_str(&format!(" runtime-filters:{}", generator.filters.len()));
    }
    propagateProbeParents(plan, 1.0);
}
/// 合并父子连续 Selection，把子条件并入父节点以减少算子层数。
fn mergeContinuousSelections(plan: &mut PlanNode) {
    for child in &mut plan.children {
        mergeContinuousSelections(child);
    }
    if let PlanKind::Selection { conditions } = &mut plan.kind {
        if plan
            .children
            .first()
            .is_some_and(|child| matches!(child.kind, PlanKind::Selection { .. }))
        {
            let mut child = plan.children.remove(0);
            if let PlanKind::Selection {
                conditions: child_conditions,
            } = &mut child.kind
            {
                conditions.append(child_conditions);
            }
            plan.children = child.children;
        }
    }
}
/// 消除无条件 UnionScan 或单子节点 Lock，直接提升子计划。
fn eliminateUnionScanAndLock(plan: &mut PlanNode) {
    for child in &mut plan.children {
        eliminateUnionScanAndLock(child);
    }
    if matches!(plan.kind, PlanKind::UnionScan { ref conditions } if conditions.is_empty())
        || matches!(plan.kind, PlanKind::Lock) && plan.children.len() == 1
    {
        *plan = plan.children.remove(0);
    }
}
/// 为 Apply 算子标记 parallel，提示执行侧可并行驱动相关子查询。
fn enableParallelApply(plan: &mut PlanNode) {
    if matches!(plan.kind, PlanKind::Apply) {
        plan.operator_info.push_str(" parallel");
        // Match Go's nested-Apply limitation: only the outer side may contain
        // additional parallel Apply operators. The compact plan model fixes
        // the inner side at child 1.
        if let Some(outer) = plan.children.first_mut() {
            enableParallelApply(outer);
        }
        return;
    }
    for child in &mut plan.children {
        enableParallelApply(child);
    }
}
/// 沿计划树传播探测侧父节点规模，供运行时过滤器等代价估计使用。
fn propagateProbeParents(plan: &mut PlanNode, probes: f64) {
    plan.probe_count = probes;
    let has_inner_probe_side = matches!(
        plan.kind,
        PlanKind::Apply
            | PlanKind::IndexJoin { .. }
            | PlanKind::IndexHashJoin { .. }
            | PlanKind::IndexMergeJoin { .. }
    );
    for (index, child) in plan.children.iter_mut().enumerate() {
        let child_probes = if has_inner_probe_side && index == 1 {
            probes * plan.estimated_rows.max(1.0)
        } else {
            probes
        };
        propagateProbeParents(child, child_probes);
    }
}
/// 先序遍历物理计划；visitor 返回 false 时停止向下。
pub fn iteratePhysicalPlan(plan: &PlanNode, visitor: &mut impl FnMut(&PlanNode) -> bool) {
    if !visitor(plan) {
        return;
    }
    for child in &plan.children {
        iteratePhysicalPlan(child, visitor);
    }
}
/// 自底向上变换物理计划树：先递归子节点再对当前节点应用 transform。
pub fn transformPhysicalPlan(
    mut plan: PlanNode,
    transform: &mut impl FnMut(PlanNode) -> PlanNode,
) -> PlanNode {
    plan.children = plan
        .children
        .into_iter()
        .map(|child| transformPhysicalPlan(child, transform))
        .collect();
    transform(plan)
}
/// 检测是否存在无等值条件的 Join/HashJoin（笛卡尔积）。
pub fn existsCartesianProduct(plan: &PlanNode) -> bool {
    matches!(&plan.kind, PlanKind::Join { equal_conditions } | PlanKind::HashJoin { equal_conditions, .. } if equal_conditions.is_empty())
        || plan.children.iter().any(existsCartesianProduct)
}
/// 递归累加节点估算代价。
fn total_cost(plan: &PlanNode) -> f64 {
    plan.estimated_cost + plan.children.iter().map(total_cost).sum::<f64>()
}
