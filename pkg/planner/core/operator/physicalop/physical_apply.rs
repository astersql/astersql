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

// Apply 物理算子：对外层每一行驱动内层相关子查询（Correlated Subquery），
// 复用 HashJoin 结构但保持独立的计划类型，避免被误判为已解相关的 HashJoin。

use crate::PhysicalHashJoin;
use base::{ContextRef, PhysicalPlan, Plan, Task};
use costusage::{CostVer2, PlanCostOption};
use expression::CorrelatedColumn;
use std::collections::HashSet;

/// PhysicalApply 对应 Go 的 Apply：嵌套循环式相关执行，内层依赖外层绑定列。
pub struct PhysicalApply {
    /// 内嵌的 HashJoin 骨架，承载连接类型、键与子计划树。
    pub PhysicalHashJoin: PhysicalHashJoin,
    /// 是否允许缓存内层结果，减少重复探测。
    pub CanUseCache: bool,
    /// 并行度；代价估算中作为分母降低内层重复代价。
    pub Concurrency: i32,
    /// 是否要求输出保持外层输入顺序。
    pub KeepOrder: bool,
    /// 外层向内层传递的相关列（Correlated Column）集合。
    pub OuterSchema: Vec<CorrelatedColumn>,
    /// 为 true 时禁止解相关（Decorrelate）改写，强制保留 Apply。
    pub NoDecorrelate: bool,
}

impl PhysicalApply {
    /// 用给定 HashJoin 骨架构造默认 Apply（串行、不缓存、不强制保序）。
    pub fn New(join: PhysicalHashJoin) -> Self {
        Self {
            PhysicalHashJoin: join,
            CanUseCache: false,
            Concurrency: 1,
            KeepOrder: false,
            OuterSchema: Vec::new(),
            NoDecorrelate: false,
        }
    }
    /// 安装 TypeApply 基类、统计信息与子节点物理属性要求。
    pub fn Init(
        mut self,
        ctx: ContextRef,
        stats: property::StatsInfo,
        offset: i32,
        props: Vec<Box<property::PhysicalProperty>>,
    ) -> Self {
        // Go initializes Apply with its own plan type instead of delegating to
        // PhysicalHashJoin::Init, which would incorrectly expose this operator
        // as a decorrelated HashJoin in diagnostics and EXPLAIN.
        // 必须自建 TypeApply，不能委托 HashJoin::Init，否则 EXPLAIN 会显示成 HashJoin。
        let mut plan = crate::NewBasePhysicalPlan(ctx, plancodec::TypeApply, offset);
        plan.set_stats(stats);
        plan.SetChildrenReqProps(props);
        self.PhysicalHashJoin
            .BasePhysicalJoin
            .PhysicalSchemaProducer
            .BasePhysicalPlan = plan;
        self
    }
    /// 将两侧转成 RootTask，挂载子计划并重建 Join 输出 Schema。
    pub fn Attach2Task(&self, tasks: Vec<Box<dyn Task>>) -> Box<dyn Task> {
        let [left, right]: [Box<dyn Task>; 2] = tasks
            .try_into()
            .unwrap_or_else(|_| panic!("PhysicalApply requires exactly two child tasks"));
        let context = self.s_ctx().clone();
        let left = left.convert_to_root_task(context.clone());
        let right = right.convert_to_root_task(context.clone());
        let children = vec![
            left.plan()
                .clone_physical(left.plan().s_ctx().clone())
                .expect("clone Apply left child"),
            right
                .plan()
                .clone_physical(right.plan().s_ctx().clone())
                .expect("clone Apply right child"),
        ];
        let mut apply = self.Clone(context).expect("clone PhysicalApply");
        apply.set_children(children);
        let schema = crate::BuildPhysicalJoinSchema(
            apply.PhysicalHashJoin.BasePhysicalJoin.JoinType,
            &apply,
        );
        apply
            .PhysicalHashJoin
            .BasePhysicalJoin
            .PhysicalSchemaProducer
            .SetSchema(schema);
        Box::new(crate::RootTask::New(Box::new(apply), None))
    }
    /// Apply 不是普通物理 Join 实现标记，供类型分发区分。
    pub fn PhysicalJoinImplement(&self) -> bool {
        false
    }
    /// 深克隆 Join 骨架与相关列；标量配置按值复制。
    pub fn Clone(&self, new_ctx: ContextRef) -> Result<Self, expression::Error> {
        Ok(Self {
            PhysicalHashJoin: self.PhysicalHashJoin.Clone(new_ctx)?,
            CanUseCache: self.CanUseCache,
            Concurrency: self.Concurrency,
            KeepOrder: self.KeepOrder,
            OuterSchema: self
                .OuterSchema
                .iter()
                .map(CorrelatedColumn::Clone)
                .collect(),
            NoDecorrelate: self.NoDecorrelate,
        })
    }
    /// 提取仍需向上传递的相关列；已由外层孩子提供的列从结果中剔除。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        let mut result = self.PhysicalHashJoin.ExtractCorrelatedCols();
        // 外层孩子 schema 已覆盖的列不再视为未绑定相关列。
        if let Some(outer) = self
            .PhysicalHashJoin
            .BasePhysicalJoin
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children()
            .first()
        {
            result.retain(|column| !outer.schema().Contains(&column.column));
        }
        result
    }
    /// Apply 代价：条件求值 + 外层代价 + 外层行数 × 内层代价。
    pub fn GetCost(&self, mut left: f64, mut right: f64, left_cost: f64, right_cost: f64) -> f64 {
        let join = &self.PhysicalHashJoin.BasePhysicalJoin;
        let cpu = join
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .s_ctx()
            .GetSessionVars()
            .GetCPUFactor();
        let mut cpu_cost = 0.0;
        if !join.LeftConditions.is_empty() {
            cpu_cost += left * cpu;
            left *= cardinality::SelectionFactor;
        }
        if !join.RightConditions.is_empty() {
            cpu_cost += left * right * cpu;
            right *= cardinality::SelectionFactor;
        }
        if !self.PhysicalHashJoin.EqualConditions.is_empty()
            || !join.OtherConditions.is_empty()
            || !self.PhysicalHashJoin.NAEqualConditions.is_empty()
        {
            let semi_factor = if matches!(
                join.JoinType,
                base::JoinType::SemiJoin
                    | base::JoinType::AntiSemiJoin
                    | base::JoinType::LeftOuterSemiJoin
                    | base::JoinType::AntiLeftOuterSemiJoin
            ) {
                0.5
            } else {
                1.0
            };
            cpu_cost += left * right * cpu * semi_factor;
        }
        cpu_cost + left_cost + left * right_cost
    }
    /// 代价模型 v1：委托内嵌 HashJoin 的估算。
    pub fn GetPlanCostVer1(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.PhysicalHashJoin.GetPlanCostVer1(task, option)
    }
    /// 代价模型 v2：委托内嵌 HashJoin 的 CostVer2 估算。
    pub fn GetPlanCostVer2(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
        inl: &[bool],
    ) -> Result<CostVer2, expression::Error> {
        self.PhysicalHashJoin.GetPlanCostVer2(task, option, inl)
    }
    /// 估算内存：HashJoin 占用 + Apply 标量/切片 + OuterSchema 各相关列。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalHashJoin.MemoryUsage()
            + (3 * std::mem::size_of::<bool>()) as i64
            + std::mem::size_of::<Vec<CorrelatedColumn>>() as i64
            + (self.OuterSchema.capacity() * std::mem::size_of::<CorrelatedColumn>()) as i64
            + self
                .OuterSchema
                .iter()
                .map(|column| column.MemoryUsage() - std::mem::size_of::<CorrelatedColumn>() as i64)
                .sum::<i64>()
    }
    /// 先解析 Join 侧索引，再按外层 schema 去重并重解 OuterSchema 列索引。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.PhysicalHashJoin.ResolveIndices()?;
        let children = self
            .PhysicalHashJoin
            .BasePhysicalJoin
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children();
        let Some(outer) = children.first() else {
            return Ok(());
        };
        let outer_schema = outer.schema().Clone();
        let inner_schema = children.get(1).map(|inner| inner.schema().Clone());
        let mut seen = HashSet::new();
        // 按 UniqueID 去重，避免同一相关列重复绑定。
        // Go's map assignment keeps the last column for a duplicated UniqueID.
        self.OuterSchema.reverse();
        self.OuterSchema
            .retain(|column| seen.insert(column.column.UniqueID));
        self.OuterSchema.reverse();
        for column in &mut self.OuterSchema {
            column.column = column.column.ResolveIndices(&outer_schema)?;
        }
        if let Some(inner_schema) = inner_schema {
            let joined_schema = expression::MergeSchema(Some(&outer_schema), Some(&inner_schema))
                .expect("two child schemas always produce a joined schema");
            for condition in &mut self.PhysicalHashJoin.EqualConditions {
                let resolved = condition.ResolveIndices(&joined_schema)?;
                *condition = resolved
                    .as_any()
                    .downcast_ref::<expression::ScalarFunction>()
                    .expect("resolving a scalar function preserves its type")
                    .clone_scalar();
            }
            for condition in &mut self.PhysicalHashJoin.NAEqualConditions {
                let resolved = condition.ResolveIndices(&joined_schema)?;
                *condition = resolved
                    .as_any()
                    .downcast_ref::<expression::ScalarFunction>()
                    .expect("resolving a scalar function preserves its type")
                    .clone_scalar();
            }
        }
        Ok(())
    }
}
