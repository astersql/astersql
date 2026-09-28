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

// 名义排序（NominalSort）：在物理属性中声明排序需求，但不一定真正插入 Sort 算子。
//
// 当 ORDER BY 仅含列引用（`OnlyColumn`）时，子任务可能已满足排序属性，
// `Attach2Task` 直接透传子任务；含标量函数时则需物化为真正的 Root 排序计划。

use base::{ContextRef, PhysicalPlan, Task};
use costusage::{CostVer2, PlanCostOption};
use logicalop::LogicalPlan as _;

use crate::{BasePhysicalPlan, GetPropByOrderByItemsContainScalarFunc, PhysicalSchemaProducer};

/// 名义排序物理算子：携带排序项与是否“仅列引用”标志。
pub struct NominalSort {
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    pub ByItems: Vec<planner_util::ByItems>,
    /// 为真表示排序键全是列，可依赖已有物理属性而跳过真实 Sort。
    pub OnlyColumn: bool,
}

impl NominalSort {
    /// 以 TypeSort 编码创建空名义排序节点。
    pub fn New(ctx: ContextRef) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeSort,
                0,
            )),
            ByItems: Vec::new(),
            OnlyColumn: false,
        }
    }

    /// 绑定统计信息、查询块偏移与子节点要求的物理属性。
    pub fn Init(
        mut self,
        ctx: ContextRef,
        stats: property::StatsInfo,
        offset: i32,
        props: Vec<Box<property::PhysicalProperty>>,
    ) -> Self {
        self.PhysicalSchemaProducer.BasePhysicalPlan =
            BasePhysicalPlan::New(ctx, plancodec::TypeSort, offset);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(stats);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .SetChildrenReqProps(props);
        self
    }

    /// 从逻辑 Sort 推导名义排序；MPP 仅列模式在含标量函数时返回 None。
    pub fn FromLogical(
        logical: &logicalop::LogicalSort,
        required: &property::PhysicalProperty,
        mpp_only_columns: bool,
    ) -> Option<Self> {
        let ctx = logical.SCtx()?.clone();
        let (property, only_columns) = GetPropByOrderByItemsContainScalarFunc(&logical.ByItems);
        let mut property = property?;
        // MPP 路径若要求“仅列排序”，含标量函数则无法生成 NominalSort。
        if mpp_only_columns && !only_columns {
            return None;
        }
        if mpp_only_columns {
            // 保留 required 的 MPP 本质字段，只替换排序项。
            let mut mpp = required.CloneEssentialFields();
            mpp.SortItems = property.SortItems;
            property = mpp;
        } else {
            property.ExpectedCnt = required.ExpectedCnt;
            property.NoCopPushDown = required.NoCopPushDown;
        }
        let stats = logical
            .StatsInfo()
            .map(|stats| stats.ScaleByExpectCnt(ctx.GetSessionVars(), required.ExpectedCnt))
            .unwrap_or_default();
        let mut nominal = Self::New(ctx.clone());
        nominal.ByItems = logical
            .ByItems
            .iter()
            .map(planner_util::ByItems::Clone)
            .collect();
        nominal.OnlyColumn = only_columns;
        nominal
            .PhysicalSchemaProducer
            .SetSchema(logical.Schema().Clone());
        Some(nominal.Init(
            ctx,
            stats,
            logical.QueryBlockOffset(),
            vec![Box::new(property)],
        ))
    }

    /// 克隆到新 PlanContext，并复制 Schema 与排序项。
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
            ByItems: self
                .ByItems
                .iter()
                .map(planner_util::ByItems::Clone)
                .collect(),
            OnlyColumn: self.OnlyColumn,
        })
    }

    /// 将排序表达式中的列下标解析到子节点 Schema。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.PhysicalSchemaProducer.ResolveIndices()?;
        let Some(schema) = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children()
            .first()
            .map(|child| child.schema().Clone())
        else {
            return Ok(());
        };
        for item in &mut self.ByItems {
            item.Expr = item.Expr.ResolveIndices(&schema)?;
        }
        Ok(())
    }

    /// 统计生产者与排序项的内存占用，并计入 OnlyColumn 布尔字段大小。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + std::mem::size_of::<Vec<planner_util::ByItems>>() as i64
            + (self.ByItems.capacity() * std::mem::size_of::<*const planner_util::ByItems>()) as i64
            + self
                .ByItems
                .iter()
                .map(planner_util::ByItems::MemoryUsage)
                .sum::<i64>()
            + std::mem::size_of::<bool>() as i64
    }

    /// 仅列排序时透传子任务；否则物化为 RootTask 上的真实排序计划。
    pub fn Attach2Task(&self, tasks: Vec<Box<dyn Task>>) -> Box<dyn Task> {
        if self.OnlyColumn {
            return tasks
                .into_iter()
                .next()
                .expect("NominalSort requires one child task");
        }
        let children = tasks
            .iter()
            .map(|task| {
                task.plan()
                    .clone_physical(base::Plan::s_ctx(task.plan()).clone())
            })
            .collect::<Result<Vec<_>, _>>()
            .expect("NominalSort child task plan clone");
        let mut plan = self
            .Clone(base::Plan::s_ctx(self).clone())
            .expect("NominalSort clone");
        base::PhysicalPlan::set_children(&mut plan, children);
        Box::new(crate::RootTask::New(Box::new(plan), None))
    }

    /// 委托给 BasePhysicalPlan 的代价模型 v1。
    pub fn GetPlanCostVer1(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer1(task, option)
    }

    /// 委托给 BasePhysicalPlan 的代价模型 v2。
    pub fn GetPlanCostVer2(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
        inl: &[bool],
    ) -> Result<CostVer2, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer2(task, option, inl)
    }
}
