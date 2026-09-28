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

// 物理流式聚合（Stream Aggregation）算子。
//
// 要求输入已按分组键有序，可边扫边聚合，无需哈希表；与 HashAgg 相对，内存更省但对排序/索引有依赖。

use crate::{BasePhysicalAgg, aggregate_to_pb};
use base::{ContextRef, PhysicalPlan as _, Plan as _, Task};
use costusage::{CostVer2, PlanCostOption};

/// 流式聚合：仅包装 `BasePhysicalAgg`（分组键、聚合函数描述符等共享字段）。
pub struct PhysicalStreamAgg {
    pub BasePhysicalAgg: BasePhysicalAgg,
}

impl PhysicalStreamAgg {
    /// 返回可变借用的底层聚合基类，供共享逻辑原地改写。
    pub fn GetPointer(&mut self) -> &mut BasePhysicalAgg {
        &mut self.BasePhysicalAgg
    }
    /// 以新会话上下文克隆整棵聚合基类子树。
    pub fn Clone(&self, new_ctx: ContextRef) -> Result<Self, expression::Error> {
        Ok(Self {
            BasePhysicalAgg: self.BasePhysicalAgg.CloneWithSelf(new_ctx)?,
        })
    }
    /// 内存估算委托给 `BasePhysicalAgg`。
    pub fn MemoryUsage(&self) -> i64 {
        self.BasePhysicalAgg.MemoryUsage()
    }
    /// 编码为 tipb StreamAgg Executor（ExecType::TypeStreamAgg）。
    pub fn ToPB(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        aggregate_to_pb(
            &self.BasePhysicalAgg,
            ctx,
            store,
            tipb::ExecType::TypeStreamAgg,
        )
    }
    /// 按 Go v1 模型估算 CPU 与 DISTINCT 分组状态的内存成本。
    pub fn GetCost(&self, input_rows: f64, is_root: bool, _flag: u64) -> f64 {
        let plan = &self.BasePhysicalAgg.PhysicalSchemaProducer.BasePhysicalPlan;
        let vars = plan.s_ctx().GetSessionVars();
        let cpu_factor = if is_root {
            vars.GetCPUFactor()
        } else {
            vars.GetCopCPUFactor()
        };
        let cpu_cost = input_rows * cpu_factor * self.BasePhysicalAgg.GetAggFuncCostFactor(false);
        let rows_per_group = input_rows / plan.stats_count();
        let memory_cost = rows_per_group
            * 0.8 // Go cost.DistinctFactor.
            * vars.GetMemoryFactor()
            * self.BasePhysicalAgg.NumDistinctFunc() as f64;
        cpu_cost + memory_cost
    }
    /// v1 代价模型：转发给基础物理计划。
    pub fn GetPlanCostVer1(
        &mut self,
        task_type: property::TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.BasePhysicalAgg
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer1(task_type, option)
    }
    /// v2 代价模型：转发给基础物理计划。
    pub fn GetPlanCostVer2(
        &mut self,
        task_type: property::TaskType,
        option: &PlanCostOption,
        inl: &[bool],
    ) -> Result<CostVer2, expression::Error> {
        self.BasePhysicalAgg
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer2(task_type, option, inl)
    }
    /// 将本算子挂接到执行任务树。
    pub fn Attach2Task(&self, tasks: Vec<Box<dyn Task>>) -> Box<dyn Task> {
        base::PhysicalPlan::attach_to_task(self, tasks)
    }
}
