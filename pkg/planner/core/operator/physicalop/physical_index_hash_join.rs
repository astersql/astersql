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

// 物理算子：索引哈希连接（PhysicalIndexHashJoin）。
//
// 在索引连接（IndexJoin）思路上用哈希探测加速 inner 匹配；
// 可选保持外表（outer）行序。文件前半为 Go 对齐草稿（已注释），后半为可编译骨架实现。

// 优化器计划节点。
//
/// PhysicalIndexHashJoin 对应 Go 的同名结构体，在 PhysicalIndexJoin 基础上补充外表顺序约束。
// pub struct PhysicalIndexHashJoin {
//     pub PhysicalIndexJoin: PhysicalIndexJoin,
/// KeepOuterOrder 为 true 时，输出结果保持 outer side 的行序。
//     pub KeepOuterOrder: bool,
// }
//
// impl PhysicalIndexHashJoin {
/// Init 对应 Go 初始化：设置算子类型、分配会话级 PlanID、绑定上下文，并令 Self 指向本算子。
//     pub fn Init(mut self, ctx: base::PlanContext) -> Self {
//         self.SetTP(plancodec::TypeIndexHashJoin);
// Go 使用原子 Add(1) 分配 ID；这里保留相同的会话级递增时机。
//         self.SetID(ctx.GetSessionVars().PlanID.Add(1) as i32);
//         self.SetSCtx(ctx);
//         self.Self = SelfRef::current(&self);
//         self
//     }
//
/// Clone 对应 Go 的 PhysicalPlan 克隆：基础连接和嵌入的 IndexJoin 均独立克隆。
//     pub fn Clone(
//         &self,
//         new_ctx: base::PlanContext,
//     ) -> Result<Box<dyn base::PhysicalPlan>, errors::Error> {
//         let mut cloned = PhysicalIndexHashJoin::default();
//         cloned.SetSCtx(new_ctx.clone());
//         cloned.PhysicalIndexJoin.BasePhysicalJoin =
//             self.BasePhysicalJoin.CloneWithSelf(new_ctx.clone(), &mut cloned)?;
//
// Go 会断言动态类型确为 *PhysicalIndexJoin；用显式转换保留这一不变量。
//         let physical_index_join = self.PhysicalIndexJoin.Clone(new_ctx)?;
//         cloned.PhysicalIndexJoin = physical_index_join
//             .downcast::<PhysicalIndexJoin>()
//             .expect("PhysicalIndexJoin.Clone must preserve its concrete type");
//         cloned.KeepOuterOrder = self.KeepOuterOrder;
//         Ok(Box::new(cloned))
//     }
//
/// Attach2Task 保留 Go 中由 utilfuncp 统一完成子任务挂接的入口。
//     pub fn Attach2Task(&mut self, tasks: Vec<base::Task>) -> base::Task {
//         utilfuncp::Attach2Task4PhysicalIndexHashJoin(self, tasks)
//     }
//
/// GetCost 计算本算子及其 outer/inner 子计划的旧版成本。
//     pub fn GetCost(
//         &self,
//         outer_cnt: f64,
//         inner_cnt: f64,
//         outer_cost: f64,
//         inner_cost: f64,
//         cost_flag: u64,
//     ) -> f64 {
//         utilfuncp::GetCost4PhysicalIndexHashJoin(
//             self, outer_cnt, inner_cnt, outer_cost, inner_cost, cost_flag,
//         )
//     }
//
/// GetPlanCostVer1 在尚未缓存成本时委托旧版计划成本计算器。
//     pub fn GetPlanCostVer1(
//         &self,
//         task_type: property::TaskType,
//         option: &costusage::PlanCostOption,
//     ) -> Result<f64, errors::Error> {
//         utilfuncp::GetPlanCostVer1PhysicalIndexHashJoin(self, task_type, option)
//     }
//
/// GetPlanCostVer2 复用 IndexJoin V2 成本函数；末尾常量 1 对应 Hash Join 的并发/批次系数。
//     pub fn GetPlanCostVer2(
//         &self,
//         task_type: property::TaskType,
//         option: &costusage::PlanCostOption,
//         _reload: &[bool],
//     ) -> Result<costusage::CostVer2, errors::Error> {
//         utilfuncp::GetIndexJoinCostVer24PhysicalIndexJoin(
//             &self.PhysicalIndexJoin,
//             task_type,
//             option,
//             1,
//         )
//     }
//
/// MemoryUsage 在嵌入 IndexJoin 的估算上增加 KeepOuterOrder 布尔字段。
//     pub fn MemoryUsage(&self) -> i64 {
//         self.PhysicalIndexJoin.MemoryUsage() + size::SizeOfBool
//     }
// }
// */
use crate::physical_common_plans::{PhysicalKind, PhysicalPlanNode};
#[derive(Clone, Debug, PartialEq)]
/// 索引哈希连接骨架：outer/inner 子计划、是否保序与并发度。
pub struct PhysicalIndexHashJoin {
    /// 外表（驱动侧）物理计划节点。
    pub outer: PhysicalPlanNode,
    /// 内表（索引探测侧）物理计划节点。
    pub inner: PhysicalPlanNode,
    /// 为 true 时输出保持 outer 侧行序。
    pub keep_outer_order: bool,
    /// 并行 worker 数（代价估算中作除数）。
    pub concurrency: usize,
    /// 缓存的 V1 计划代价，避免重复计算。
    pub cached_cost: Option<f64>,
}
impl PhysicalIndexHashJoin {
    /// 合并两侧 Schema，组装 IndexHashJoin 计划节点。
    pub fn attach_to_task(&self) -> PhysicalPlanNode {
        let mut schema = self.outer.schema.clone();
        schema.extend(&self.inner.schema);
        PhysicalPlanNode {
            id: self.outer.id.max(self.inner.id) + 1,
            kind: PhysicalKind::IndexHashJoin,
            schema,
            children: vec![self.outer.clone(), self.inner.clone()],
            stats: self.outer.stats.clone(),
            required_properties: Vec::new(),
        }
    }
    /// 估算代价：子代价 + CPU（按并发分摊）+ 内表内存项。
    pub fn cost(
        &self,
        outer_count: f64,
        inner_count: f64,
        outer_cost: f64,
        inner_cost: f64,
        cpu_factor: f64,
        memory_factor: f64,
    ) -> f64 {
        let concurrency = self.concurrency.max(1) as f64;
        outer_cost
            + inner_cost
            + (outer_count + inner_count) * cpu_factor / concurrency
            + inner_count * memory_factor
    }
    /// 计划代价 V1：命中缓存则直接返回。
    pub fn plan_cost_v1(&mut self, cpu: f64, memory: f64) -> f64 {
        if let Some(cost) = self.cached_cost {
            return cost;
        }
        let cost = self.cost(
            self.outer.stats.row_count,
            self.inner.stats.row_count,
            0.0,
            0.0,
            cpu,
            memory,
        );
        self.cached_cost = Some(cost);
        cost
    }
    /// 计划代价 V2：不写缓存，直接按统计行数估算。
    pub fn plan_cost_v2(&self, cpu: f64, memory: f64) -> f64 {
        self.cost(
            self.outer.stats.row_count,
            self.inner.stats.row_count,
            0.0,
            0.0,
            cpu,
            memory,
        )
    }
    /// 估算本结构及子计划内存。
    pub fn memory_usage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64 + self.outer.memory_usage() + self.inner.memory_usage()
    }
}
