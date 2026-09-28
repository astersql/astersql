// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// Cascades / Memo 物理实现（Implementation）的公共基础。
//
// 本模块提供：
// - [`BaseImpl`]：缓存算子自身代价（cost）的公共字段与默认代价/代价上限计算；
// - 各类 `*CostPlan` trait：从具体物理计划节点抽取代价估算所需参数
//   （网络因子、扫描因子、行宽等）；
// - 子计划克隆、挂接与子节点代价/行数读取辅助函数；
// - [`impl_implementation`] 宏：把具体 Impl 结构接到 Memo 的
//   `Implementation` trait。
//
// 「实现（Implementation）」是 Cascades 优化器中逻辑算子到物理算子的一种候选，
// 代价用于在等价物理方案中择优。

use std::cell::Cell;

use astersql_expression::{Column, Schema};
use astersql_meta_model::TableInfo;
use astersql_planner_core_base::PhysicalPlan;
use astersql_planner_memo::ImplementationRef;
use astersql_statistics::HistColl;

/// 物理实现的公共代价基座：用 `Cell` 缓存已计算的 cost，便于在不可变引用下更新。
#[derive(Default)]
pub struct BaseImpl {
    /// 已计算或手动设置的算子代价（含已计入的子树代价，语义由具体 Impl 决定）。
    cost: Cell<f64>,
}

impl BaseImpl {
    /// 默认代价：对各子 Implementation 的 `GetCost` 求和并缓存。
    /// `out_count` 在默认实现中未使用，留给具体算子按输出行数加权。
    pub fn CalcCost(&self, _out_count: f64, children: &[ImplementationRef]) -> f64 {
        let cost = children.iter().map(|child| child.borrow().GetCost()).sum();
        self.cost.set(cost);
        cost
    }

    /// 直接写入缓存代价（用于自定义代价公式算完后回写）。
    pub fn SetCost(&self, cost: f64) {
        self.cost.set(cost);
    }

    /// 读取当前缓存代价。
    pub fn GetCost(&self) -> f64 {
        self.cost.get()
    }

    /// 默认不缩放代价上限，原样返回。
    pub fn ScaleCostLimit(&self, cost_limit: f64) -> f64 {
        cost_limit
    }

    /// 计算传给子节点的代价上限：总上限减去已有子节点已消耗的代价之和。
    /// 代价上限用于剪枝：若部分子树已超限，可提前放弃探索。
    pub fn GetCostLimit(&self, cost_limit: f64, children: &[ImplementationRef]) -> f64 {
        cost_limit
            - children
                .iter()
                .map(|child| child.borrow().GetCost())
                .sum::<f64>()
    }
}

/// 访问底层物理计划节点（只读/可变）的统一入口。
pub trait PlanAccess {
    /// 返回不可变物理计划引用。
    fn Plan(&self) -> &dyn PhysicalPlan;
    /// 返回可变物理计划引用（用于挂接子节点等）。
    fn PlanMut(&mut self) -> &mut dyn PhysicalPlan;
}

/// Table/Index Reader 代价所需参数：网络因子、平均行宽、Coprocessor 并发度。
/// Coprocessor 指下推到存储层（如 TiKV）执行的部分算子。
pub trait ReaderCostPlan: PlanAccess {
    /// 网络传输代价系数（与表属性相关）。
    fn NetworkFactor(&self, table: &TableInfo) -> f64;
    /// 按直方图与子计划估算平均行宽；`index` 表示是否按索引列宽计算。
    fn AverageRowSize(&self, histograms: &HistColl, child: &dyn PhysicalPlan, index: bool) -> f64;
    /// Coprocessor 迭代器并发 worker 数，用于摊薄网络+子代价。
    fn CopIteratorWorkers(&self) -> usize;
}

/// TableScan 代价所需参数：行宽、正/逆序扫描因子。
pub trait TableScanCostPlan: PlanAccess {
    /// 按直方图与投影列估算扫描行宽。
    fn AverageRowSize(&self, histograms: &HistColl, columns: &[Column]) -> f64;
    /// 升序扫描代价因子。
    fn ScanFactor(&self) -> f64;
    /// 降序扫描代价因子（通常更高）。
    fn DescScanFactor(&self) -> f64;
    /// 是否降序扫描。
    fn Descending(&self) -> bool;
}

/// IndexScan 代价所需参数：行宽、扫描/Seek 因子、range 段数。
/// Seek 表示定位到某个索引 range 起点的额外开销。
pub trait IndexScanCostPlan: PlanAccess {
    /// 按直方图估算索引行宽。
    fn AverageRowSize(&self, histograms: &HistColl) -> f64;
    /// 升序扫描因子。
    fn ScanFactor(&self) -> f64;
    /// 降序扫描因子。
    fn DescScanFactor(&self) -> f64;
    /// 每个 range 的 Seek 代价因子。
    fn SeekFactor(&self) -> f64;
    /// 是否降序扫描。
    fn Descending(&self) -> bool;
    /// 索引 range 段数量（决定 Seek 次数）。
    fn RangeCount(&self) -> usize;
}

/// 二元 Join（HashJoin / MergeJoin 等）自身代价估算接口。
pub trait BinaryJoinCostPlan: PlanAccess {
    /// 根据左右输入行数估算 Join 自身代价（不含子树）。
    fn SelfCost(&self, left_rows: f64, right_rows: f64) -> f64;
}

/// Projection 自身代价估算接口。
pub trait ProjectionCostPlan: PlanAccess {
    /// 按输入行数估算投影表达式计算代价。
    fn SelfCost(&self, input_rows: f64) -> f64;
}

/// Selection（Filter）代价所需 CPU 因子。
pub trait SelectionCostPlan: PlanAccess {
    /// `coprocessor` 为 true 时使用下推到存储层的 CPU 因子。
    fn CPUFactor(&self, coprocessor: bool) -> f64;
}

/// HashAgg 自身代价估算接口；`root` 区分 TiDB 侧与 TiKV 侧聚合。
pub trait HashAggCostPlan: PlanAccess {
    /// 按输入行数与是否根侧（root）聚合估算自身代价。
    fn SelfCost(&self, input_rows: f64, root: bool) -> f64;
}

/// TopN 自身代价估算接口；`root` 同样区分根侧与下推侧。
pub trait TopNCostPlan: PlanAccess {
    /// 按输入行数与是否根侧估算 TopN 自身代价。
    fn SelfCost(&self, input_rows: f64, root: bool) -> f64;
}

/// UnionAll 并发因子接口。
pub trait UnionAllCostPlan: PlanAccess {
    /// 分支并发执行的代价系数。
    fn ConcurrencyFactor(&self) -> f64;
}

/// Apply（相关子查询/侧向连接）代价估算接口。
/// Apply 对左表每一行驱动右子计划执行，代价与左行数强相关。
pub trait ApplyCostPlan: PlanAccess {
    /// 综合左右行数与子代价估算 Apply 总代价。
    fn SelfCost(&self, left_rows: f64, right_rows: f64, left_cost: f64, right_cost: f64) -> f64;
    /// 左表是否带过滤条件（影响代价上限中左行数的缩放）。
    fn HasLeftConditions(&self) -> bool;
}

/// Sort 代价与计划改写接口。
pub trait SortCostPlan: PlanAccess {
    /// 排序期望处理行数上限（可与统计行数取 min）。
    fn ExpectedCount(&self) -> f64;
    /// 按实际排序行数与 schema 估算排序自身代价。
    fn SelfCost(&self, input_rows: f64, schema: &Schema) -> f64;
    /// 在 Sort 下方注入 Projection（例如补齐排序键列）后返回新的物理计划树。
    fn InjectProjectionBelowSort(&mut self, child: Box<dyn PhysicalPlan>) -> Box<dyn PhysicalPlan>;
}

/// 克隆物理计划（要求可 clone），用于挂接 Memo 子 Implementation。
pub fn ClonePlan(plan: &dyn PhysicalPlan) -> Box<dyn PhysicalPlan> {
    plan.clone_physical(plan.s_ctx().clone())
        .expect("memo implementation child plan must be cloneable")
}

/// 将子 Implementation 列表克隆为物理计划子节点向量。
pub fn CloneChildren(children: &[ImplementationRef]) -> Vec<Box<dyn PhysicalPlan>> {
    children
        .iter()
        .map(|child| ClonePlan(child.borrow().GetPlan()))
        .collect()
}

/// 把子 Implementation 克隆并挂到物理计划的 children 上。
pub fn AttachChildren(plan: &mut dyn PhysicalPlan, children: &[ImplementationRef]) {
    plan.set_children(CloneChildren(children));
}

/// 读取第 `index` 个子 Implementation 的缓存代价。
pub fn ChildCost(children: &[ImplementationRef], index: usize) -> f64 {
    children[index].borrow().GetCost()
}

/// 读取第 `index` 个子计划统计信息中的行数（RowCount）。
pub fn ChildRows(children: &[ImplementationRef], index: usize) -> f64 {
    children[index].borrow().GetPlan().stats_info().RowCount
}

/// 为具体 Impl 类型实现 Memo 的 `Implementation` trait。
/// 要求该类型提供 `calc_cost` / `plan` / `attach_children` / `cost_limit`，
/// 以及字段 `base: BaseImpl`。
macro_rules! impl_implementation {
    ($name:ty) => {
        impl astersql_planner_memo::Implementation for $name {
            fn CalcCost(
                &self,
                out_count: f64,
                children: &[astersql_planner_memo::ImplementationRef],
            ) -> f64 {
                self.calc_cost(out_count, children)
            }

            fn SetCost(&mut self, cost: f64) {
                self.base.SetCost(cost);
            }

            fn GetCost(&self) -> f64 {
                self.base.GetCost()
            }

            fn GetPlan(&self) -> &dyn astersql_planner_core_base::PhysicalPlan {
                self.plan()
            }

            fn AttachChildren(
                &mut self,
                children: &[astersql_planner_memo::ImplementationRef],
            ) -> &mut dyn astersql_planner_memo::Implementation {
                self.attach_children(children);
                self
            }

            fn GetCostLimit(
                &self,
                cost_limit: f64,
                children: &[astersql_planner_memo::ImplementationRef],
            ) -> f64 {
                self.cost_limit(cost_limit, children)
            }
        }
    };
}

pub(crate) use impl_implementation;
