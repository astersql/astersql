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

// 物理算子：索引归并连接（PhysicalIndexMergeJoin）。
//
// 要求两侧按连接键有序（或对 outer 先排序），再以归并方式推进匹配；
// 适合索引已提供序的场景。前半为 Go 对齐草稿（已注释），后半为可编译骨架。

// 规划期算子。
//
/// PhysicalIndexMergeJoin 对应 Go 的同名计划，在 PhysicalIndexJoin 上增加 merge 排序信息。
// pub struct PhysicalIndexMergeJoin {
//     pub PhysicalIndexJoin: PhysicalIndexJoin,
/// 将 join key 下标映射到“按索引顺序排列的 join key”下标。
//     pub KeyOff2KeyOffOrderByIdx: Vec<i32>,
/// 比较 outer join key 与 inner join key，供 merge 扫描推进使用。
//     pub CompareFuncs: Vec<expression::CompareFunc>,
/// 比较两条 outer row 的 join key，便于执行器先排序 outer rows。
//     pub OuterCompareFuncs: Vec<expression::CompareFunc>,
//     pub NeedOuterSort: bool,
/// Desc 表示 inner child 是否保持降序。
//     pub Desc: bool,
// }
//
// impl PhysicalIndexMergeJoin {
/// Init 设置算子类型、原子递增的会话 PlanID、上下文和 Self 回指。
//     pub fn Init(mut self, ctx: base::PlanContext) -> Self {
//         self.SetTP(plancodec::TypeIndexMergeJoin);
// 与 Go 一致，ID 在初始化时从会话计数器原子获取。
//         self.SetID(ctx.GetSessionVars().PlanID.Add(1) as i32);
//         self.SetSCtx(ctx);
//         self.Self = SelfRef::current(&self);
//         self
//     }
//
/// MemoryUsage 在基础 IndexJoin 上计入三个 slice、映射容量、两组函数和两个布尔字段。
//     pub fn MemoryUsage(&self) -> i64 {
//         self.PhysicalIndexJoin.MemoryUsage()
//             + size::SizeOfSlice * 3
//             + self.KeyOff2KeyOffOrderByIdx.capacity() as i64 * size::SizeOfInt
//             + (self.CompareFuncs.capacity() + self.OuterCompareFuncs.capacity()) as i64 * size::SizeOfFunc
//             + size::SizeOfBool * 2
//     }
//
/// ExplainInfo 复用 IndexJoin 解释器，并标记为 merge join 以省略普通 hash 等值条件。
//     pub fn ExplainInfo(&self) -> String {
//         self.PhysicalIndexJoin.ExplainInfoInternal(false, true)
//     }
//
/// ExplainNormalizedInfo 输出归一化的 merge join 解释文本。
//     pub fn ExplainNormalizedInfo(&self) -> String {
//         self.PhysicalIndexJoin.ExplainInfoInternal(true, true)
//     }
//
/// GetCost 计算 Index Merge Join 本身与 outer/inner 子计划的旧版成本。
//     pub fn GetCost(
//         &self,
//         outer_cnt: f64,
//         inner_cnt: f64,
//         outer_cost: f64,
//         inner_cost: f64,
//         cost_flag: u64,
//     ) -> f64 {
//         utilfuncp::GetCost4PhysicalIndexMergeJoin(
//             self, outer_cnt, inner_cnt, outer_cost, inner_cost, cost_flag,
//         )
//     }
//
/// GetPlanCostVer1 委托 Index Merge Join 的第一版计划成本计算器。
//     pub fn GetPlanCostVer1(
//         &self,
//         task_type: property::TaskType,
//         option: &costusage::PlanCostOption,
//     ) -> Result<f64, errors::Error> {
//         utilfuncp::GetPlanCostVer14PhysicalIndexMergeJoin(self, task_type, option)
//     }
//
/// GetPlanCostVer2 复用 IndexJoin V2 成本函数；类型常量 2 区分 Merge Join 路径。
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
//             2,
//         )
//     }
//
/// Attach2Task 由公共辅助函数把左右子任务装配成 Index Merge Join。
//     pub fn Attach2Task(&mut self, tasks: Vec<base::Task>) -> base::Task {
//         utilfuncp::Attach2Task4PhysicalIndexMergeJoin(self, tasks)
//     }
// }
// */
use crate::physical_common_plans::{PhysicalKind, PhysicalPlanNode};
#[derive(Clone, Debug, PartialEq)]
/// 索引归并连接骨架：键序映射、比较函数名、是否需 outer 排序与降序。
pub struct PhysicalIndexMergeJoin {
    /// 外表物理计划。
    pub outer: PhysicalPlanNode,
    /// 内表（索引有序）物理计划。
    pub inner: PhysicalPlanNode,
    /// 连接键下标按索引顺序重排后的映射。
    pub key_offset_order: Vec<usize>,
    /// outer 键与 inner 键的比较函数标识。
    pub compare_functions: Vec<String>,
    /// 两条 outer 行之间的键比较函数标识（用于排序）。
    pub outer_compare_functions: Vec<String>,
    /// 为 true 时执行前需对 outer 按连接键排序。
    pub need_outer_sort: bool,
    /// 内表是否按降序扫描。
    pub descending: bool,
    /// 并发度，用于代价分摊。
    pub concurrency: usize,
}
impl PhysicalIndexMergeJoin {
    /// 生成 EXPLAIN；归一化时隐藏具体键序细节。
    pub fn explain_info(&self, normalized: bool) -> String {
        if normalized {
            format!(
                "index merge join, key count:{}, outer sort:{}",
                self.key_offset_order.len(),
                self.need_outer_sort
            )
        } else {
            format!(
                "index merge join, key order:{:?}, outer sort:{}, desc:{}",
                self.key_offset_order, self.need_outer_sort, self.descending
            )
        }
    }
    /// 估算代价；若需 outer 排序则附加 n log n 项。
    pub fn cost(
        &self,
        outer_count: f64,
        inner_count: f64,
        outer_cost: f64,
        inner_cost: f64,
        cpu_factor: f64,
    ) -> f64 {
        // outer 无序时按比较排序代价计入。
        let sorting = if self.need_outer_sort {
            outer_count.max(1.0).log2() * outer_count * cpu_factor
        } else {
            0.0
        };
        outer_cost
            + inner_cost
            + (outer_count + inner_count) * cpu_factor / self.concurrency.max(1) as f64
            + sorting
    }
    /// 合并 Schema，组装 IndexMergeJoin 节点。
    pub fn attach_to_task(&self) -> PhysicalPlanNode {
        let mut schema = self.outer.schema.clone();
        schema.extend(&self.inner.schema);
        PhysicalPlanNode {
            id: self.outer.id.max(self.inner.id) + 1,
            kind: PhysicalKind::IndexMergeJoin,
            schema,
            children: vec![self.outer.clone(), self.inner.clone()],
            stats: self.outer.stats.clone(),
            required_properties: Vec::new(),
        }
    }
    /// 计入结构体、键序向量与比较函数字符串容量。
    pub fn memory_usage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
            + ((self.key_offset_order.capacity() * std::mem::size_of::<usize>())
                + (self.compare_functions.capacity() + self.outer_compare_functions.capacity())
                    * std::mem::size_of::<String>()
                + self
                    .compare_functions
                    .iter()
                    .map(String::capacity)
                    .sum::<usize>()
                + self
                    .outer_compare_functions
                    .iter()
                    .map(String::capacity)
                    .sum::<usize>()) as i64
    }
}
