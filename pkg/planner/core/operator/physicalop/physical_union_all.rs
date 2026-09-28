// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 物理 UNION ALL 算子：无去重合并多路子计划结果。
//
// 对应 SQL `UNION ALL`。可在 Root 或 MPP 任务上执行；分区表场景用
// PartitionUnion 类型码区分。有序/特定 Flash 属性不满足时不产出候选。

use base::{ContextRef, PhysicalPlan, Task};
use costusage::{COST_FLAG_RECALCULATE, CostVer2, PlanCostOption, div_cost_ver2, has_cost_flag};

use crate::{BasePhysicalPlan, PhysicalSchemaProducer};

/// 物理 UNION ALL：拼接各子节点行，不消除重复。
pub struct PhysicalUnionAll {
    /// Schema/统计等公共字段。
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    /// 是否作为 MPP 片段内的合并节点。
    pub Mpp: bool,
}

impl PhysicalUnionAll {
    /// 构造类型码为 Union 的空节点。
    pub fn New(ctx: ContextRef) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeUnion,
                0,
            )),
            Mpp: false,
        }
    }

    /// 写入统计、偏移与各子节点所需属性。
    pub fn Init(
        mut self,
        ctx: ContextRef,
        stats: property::StatsInfo,
        offset: i32,
        props: Vec<Box<property::PhysicalProperty>>,
    ) -> Self {
        self.PhysicalSchemaProducer.BasePhysicalPlan =
            BasePhysicalPlan::New(ctx, plancodec::TypeUnion, offset);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(stats);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .SetChildrenReqProps(props);
        self
    }

    /// 克隆 Schema 生产者与 MPP 标记。
    pub fn Clone(&self, new_ctx: ContextRef) -> Result<Self, expression::Error> {
        let mut producer = PhysicalSchemaProducer::New(
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .CloneWithNewCtx(new_ctx)?,
        );
        if let Some(schema) = self.PhysicalSchemaProducer.SchemaRef() {
            producer.SetSchema(schema.Clone());
        }
        Ok(Self {
            PhysicalSchemaProducer: producer,
            Mpp: self.Mpp,
        })
    }

    /// 估算内存占用。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage() + std::mem::size_of::<bool>() as i64
    }

    /// 挂接到执行任务。
    pub fn Attach2Task(&self, tasks: Vec<Box<dyn Task>>) -> Box<dyn Task> {
        base::PhysicalPlan::attach_to_task(self, tasks)
    }

    /// 代价模型 v1。
    pub fn GetPlanCostVer1(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        let base = &mut self.PhysicalSchemaProducer.BasePhysicalPlan;
        if base.PlanCostInit && !has_cost_flag(option.cost_flag, COST_FLAG_RECALCULATE) {
            return Ok(base.PlanCost);
        }
        let mut child_max_cost: f64 = 0.0;
        for child in base.ChildrenMut() {
            child_max_cost = child_max_cost.max(child.get_plan_cost_ver1(task, option)?);
        }
        let vars = base.Plan.SCtx().GetSessionVars();
        let concurrency_factor = vars
            .GetSystemVar(vardef::TiDBOptConcurrencyFactor)
            .and_then(|value| value.parse().ok())
            .unwrap_or(vardef::DefOptConcurrencyFactor);
        base.PlanCost = child_max_cost + (1 + base.Children().len()) as f64 * concurrency_factor;
        if self.Mpp
            && vars.IsMPPEnforced()
            && !has_cost_flag(option.cost_flag, COST_FLAG_RECALCULATE)
        {
            base.PlanCost /= 1_000_000_000.0;
        }
        base.PlanCostInit = true;
        Ok(base.PlanCost)
    }

    /// 代价模型 v2。
    pub fn GetPlanCostVer2(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
        _inl: &[bool],
    ) -> Result<CostVer2, expression::Error> {
        let base = &mut self.PhysicalSchemaProducer.BasePhysicalPlan;
        let vars = base.Plan.SCtx().GetSessionVars();
        let concurrency = vars
            .GetSystemVar(vardef::TiDBExecutorConcurrency)
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| *value > 0.0)
            .unwrap_or(vardef::DefExecutorConcurrency as f64);
        let mpp_enforced = vars.IsMPPEnforced();
        let mut cost = div_cost_ver2(&base.GetPlanCostVer2(task, option, &[])?, concurrency);
        if self.Mpp && mpp_enforced && !has_cost_flag(option.cost_flag, COST_FLAG_RECALCULATE) {
            cost = div_cost_ver2(&cost, 1_000_000_000.0);
        }
        Ok(cost)
    }
}

/// 枚举逻辑 UNION ALL 的物理候选。
pub fn ExhaustPhysicalPlans4LogicalUnionAll(
    logical: &logicalop::LogicalUnionAll,
    required: &property::PhysicalProperty,
) -> Vec<Box<dyn PhysicalPlan>> {
    build_union_plans(logical, required, plancodec::TypeUnion)
        .into_iter()
        .map(|plan| Box::new(plan) as Box<dyn PhysicalPlan>)
        .collect()
}

/// 在 Root/MPP 属性下构造 PhysicalUnionAll；允许时额外产出 MPP 方案。
fn build_union_plans(
    logical: &dyn logicalop::LogicalPlan,
    required: &property::PhysicalProperty,
    plan_type: &str,
) -> Vec<PhysicalUnionAll> {
    if !required.IsSortItemEmpty()
        // 需要有序或非 MPP 的 Flash 属性时，UNION ALL 无法直接满足。
        || (required.IsFlashProp() && required.TaskTp != property::MppTaskType)
        || (required.TaskTp == property::MppTaskType
            && required.MPPPartitionTp != property::AnyType)
    {
        return Vec::new();
    }
    let Some(ctx) = logical.SCtx().cloned() else {
        return Vec::new();
    };
    let can_use_mpp = ctx.GetSessionVars().IsMPPAllowed();
    let child_properties = |task_type| {
        logical
            .Children()
            .iter()
            .map(|_| {
                let mut child = property::PhysicalProperty::default();
                child.ExpectedCnt = required.ExpectedCnt;
                child.TaskTp = task_type;
                child.CTEProducerStatus = required.CTEProducerStatus;
                child.NoCopPushDown = required.NoCopPushDown;
                Box::new(child)
            })
            .collect::<Vec<_>>()
    };
    let stats = logical
        .StatsInfo()
        .map(|stats| stats.ScaleByExpectCnt(ctx.GetSessionVars(), required.ExpectedCnt))
        .unwrap_or_default();
    let primary_task = if can_use_mpp && required.TaskTp == property::MppTaskType {
        property::MppTaskType
    } else {
        property::RootTaskType
    };
    let mut primary = PhysicalUnionAll::New(ctx.clone());
    primary.Mpp = primary_task == property::MppTaskType;
    primary
        .PhysicalSchemaProducer
        .SetSchema(logical.Schema().Clone());
    let primary = primary.Init(
        ctx.clone(),
        stats.clone(),
        logical.QueryBlockOffset(),
        child_properties(primary_task),
    );
    let mut primary = primary;
    primary
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .Plan
        .SetTP(plan_type);
    let mut plans = vec![primary];

    if can_use_mpp && required.TaskTp == property::RootTaskType {
        // Root 需求下额外枚举一个 MPP 子树方案，便于后续 Gather。
        let mut mpp = PhysicalUnionAll::New(ctx.clone());
        mpp.Mpp = true;
        mpp.PhysicalSchemaProducer
            .SetSchema(logical.Schema().Clone());
        let mut mpp = mpp.Init(
            ctx,
            stats,
            logical.QueryBlockOffset(),
            child_properties(property::MppTaskType),
        );
        mpp.PhysicalSchemaProducer
            .BasePhysicalPlan
            .Plan
            .SetTP(plan_type);
        plans.push(mpp);
    }
    plans
}

/// 分区表 UNION ALL：类型码为 PartitionUnion。
pub fn ExhaustPhysicalPlans4LogicalPartitionUnionAll(
    logical: &logicalop::LogicalPartitionUnionAll,
    required: &property::PhysicalProperty,
) -> Vec<Box<dyn PhysicalPlan>> {
    build_union_plans(logical, required, plancodec::TypePartitionUnion)
        .into_iter()
        .map(|plan| Box::new(plan) as Box<dyn PhysicalPlan>)
        .collect()
}
