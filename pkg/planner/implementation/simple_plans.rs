// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 简单物理计划的 Implementation：Show/Limit、Projection、Selection、
// HashAgg、TopN、UnionAll、Apply，以及 MaxOneRow / Window 透传。
//
// TiDB/TiKV 前缀区分根侧（TiDB）与下推到存储（TiKV/Coprocessor）的同类算子；
// Apply 对应相关子查询的侧向连接执行模型。

use astersql_planner_core_base::PhysicalPlan;
use astersql_planner_core_cost::factors_thresholds::SelectionFactor;
use astersql_planner_memo::ImplementationRef;

use crate::{
    ApplyCostPlan, AttachChildren, BaseImpl, ChildCost, ChildRows, HashAggCostPlan, PlanAccess,
    ProjectionCostPlan, SelectionCostPlan, TopNCostPlan, UnionAllCostPlan, impl_implementation,
};

/// 代价直接委托 `BaseImpl::CalcCost`（子代价求和）的 Implementation 生成宏。
macro_rules! base_cost_implementation {
    ($name:ident, $constructor:ident) => {
        /// 使用默认子代价求和的物理实现包装。
        pub struct $name {
            pub(crate) base: BaseImpl,
            plan_node: Box<dyn PlanAccess>,
        }

        /// 构造默认代价 Implementation。
        pub fn $constructor(plan: Box<dyn PlanAccess>) -> $name {
            $name {
                base: BaseImpl::default(),
                plan_node: plan,
            }
        }

        impl $name {
            fn calc_cost(&self, out_count: f64, children: &[ImplementationRef]) -> f64 {
                self.base.CalcCost(out_count, children)
            }
            fn plan(&self) -> &dyn PhysicalPlan {
                self.plan_node.Plan()
            }
            fn attach_children(&mut self, children: &[ImplementationRef]) {
                AttachChildren(self.plan_node.PlanMut(), children);
            }
            fn cost_limit(&self, cost_limit: f64, children: &[ImplementationRef]) -> f64 {
                self.base.GetCostLimit(cost_limit, children)
            }
        }

        impl_implementation!($name);
    };
}

// SHOW 语句物理实现（代价取子树和）。
base_cost_implementation!(ShowImpl, NewShowImpl);
// LIMIT 物理实现（代价取子树和）。
base_cost_implementation!(LimitImpl, NewLimitImpl);

/// Projection：计算投影表达式后输出；代价 = 自身 + 子代价。
pub struct ProjectionImpl {
    pub(crate) base: BaseImpl,
    plan_node: Box<dyn ProjectionCostPlan>,
}

/// 构造 Projection Implementation。
pub fn NewProjectionImpl(plan: Box<dyn ProjectionCostPlan>) -> ProjectionImpl {
    ProjectionImpl {
        base: BaseImpl::default(),
        plan_node: plan,
    }
}

impl ProjectionImpl {
    /// 自身代价按子节点行数估算，再加上子 Implementation 代价。
    fn calc_cost(&self, _out_count: f64, children: &[ImplementationRef]) -> f64 {
        let cost = self.plan_node.SelfCost(ChildRows(children, 0)) + ChildCost(children, 0);
        self.base.SetCost(cost);
        cost
    }
    fn plan(&self) -> &dyn PhysicalPlan {
        self.plan_node.Plan()
    }
    fn attach_children(&mut self, children: &[ImplementationRef]) {
        AttachChildren(self.plan_node.PlanMut(), children);
    }
    fn cost_limit(&self, cost_limit: f64, children: &[ImplementationRef]) -> f64 {
        self.base.GetCostLimit(cost_limit, children)
    }
}

impl_implementation!(ProjectionImpl);

/// Selection（Filter）：按谓词过滤行；`coprocessor` 区分 TiDB/TiKV CPU 因子。
pub struct SelectionImpl {
    pub(crate) base: BaseImpl,
    plan_node: Box<dyn SelectionCostPlan>,
    /// true 表示下推到 Coprocessor（TiKV）侧执行的 Selection。
    coprocessor: bool,
}

/// 内部构造：指定是否为 Coprocessor 侧 Selection。
fn newSelectionImpl(plan: Box<dyn SelectionCostPlan>, coprocessor: bool) -> SelectionImpl {
    SelectionImpl {
        base: BaseImpl::default(),
        plan_node: plan,
        coprocessor,
    }
}

/// TiDB（根侧）Selection Implementation。
pub fn NewTiDBSelectionImpl(plan: Box<dyn SelectionCostPlan>) -> SelectionImpl {
    newSelectionImpl(plan, false)
}

/// TiKV（Coprocessor）Selection Implementation。
pub fn NewTiKVSelectionImpl(plan: Box<dyn SelectionCostPlan>) -> SelectionImpl {
    newSelectionImpl(plan, true)
}

impl SelectionImpl {
    /// 代价 = 子行数 × CPU 因子 + 子代价。
    fn calc_cost(&self, _out_count: f64, children: &[ImplementationRef]) -> f64 {
        let cost = ChildRows(children, 0) * self.plan_node.CPUFactor(self.coprocessor)
            + ChildCost(children, 0);
        self.base.SetCost(cost);
        cost
    }
    fn plan(&self) -> &dyn PhysicalPlan {
        self.plan_node.Plan()
    }
    fn attach_children(&mut self, children: &[ImplementationRef]) {
        AttachChildren(self.plan_node.PlanMut(), children);
    }
    fn cost_limit(&self, cost_limit: f64, children: &[ImplementationRef]) -> f64 {
        self.base.GetCostLimit(cost_limit, children)
    }
}

impl_implementation!(SelectionImpl);
/// TiDB 侧 Selection 的类型别名。
pub type TiDBSelectionImpl = SelectionImpl;
/// TiKV 侧 Selection 的类型别名。
pub type TiKVSelectionImpl = SelectionImpl;

/// HashAgg：哈希聚合；`root` 为 true 表示 TiDB 根侧聚合。
pub struct HashAggImpl {
    pub(crate) base: BaseImpl,
    plan_node: Box<dyn HashAggCostPlan>,
    /// true = TiDB 根侧，false = TiKV 下推侧。
    root: bool,
}

fn newHashAggImpl(plan: Box<dyn HashAggCostPlan>, root: bool) -> HashAggImpl {
    HashAggImpl {
        base: BaseImpl::default(),
        plan_node: plan,
        root,
    }
}

/// TiDB 根侧 HashAgg。
pub fn NewTiDBHashAggImpl(plan: Box<dyn HashAggCostPlan>) -> HashAggImpl {
    newHashAggImpl(plan, true)
}

/// TiKV 下推侧 HashAgg。
pub fn NewTiKVHashAggImpl(plan: Box<dyn HashAggCostPlan>) -> HashAggImpl {
    newHashAggImpl(plan, false)
}

impl HashAggImpl {
    /// 代价 = SelfCost(子行数, root) + 子代价。
    fn calc_cost(&self, _out_count: f64, children: &[ImplementationRef]) -> f64 {
        let cost =
            self.plan_node.SelfCost(ChildRows(children, 0), self.root) + ChildCost(children, 0);
        self.base.SetCost(cost);
        cost
    }
    fn plan(&self) -> &dyn PhysicalPlan {
        self.plan_node.Plan()
    }
    fn attach_children(&mut self, children: &[ImplementationRef]) {
        AttachChildren(self.plan_node.PlanMut(), children);
    }
    fn cost_limit(&self, cost_limit: f64, children: &[ImplementationRef]) -> f64 {
        self.base.GetCostLimit(cost_limit, children)
    }
}

impl_implementation!(HashAggImpl);
/// TiDB 侧 HashAgg 别名。
pub type TiDBHashAggImpl = HashAggImpl;
/// TiKV 侧 HashAgg 别名。
pub type TiKVHashAggImpl = HashAggImpl;

/// TopN：排序取前 N；`root` 区分 TiDB/TiKV。
pub struct TopNImpl {
    pub(crate) base: BaseImpl,
    plan_node: Box<dyn TopNCostPlan>,
    root: bool,
}

fn newTopNImpl(plan: Box<dyn TopNCostPlan>, root: bool) -> TopNImpl {
    TopNImpl {
        base: BaseImpl::default(),
        plan_node: plan,
        root,
    }
}

/// TiDB 根侧 TopN。
pub fn NewTiDBTopNImpl(plan: Box<dyn TopNCostPlan>) -> TopNImpl {
    newTopNImpl(plan, true)
}

/// TiKV 下推侧 TopN。
pub fn NewTiKVTopNImpl(plan: Box<dyn TopNCostPlan>) -> TopNImpl {
    newTopNImpl(plan, false)
}

impl TopNImpl {
    /// 代价 = SelfCost(子行数, root) + 子代价。
    fn calc_cost(&self, _out_count: f64, children: &[ImplementationRef]) -> f64 {
        let cost =
            self.plan_node.SelfCost(ChildRows(children, 0), self.root) + ChildCost(children, 0);
        self.base.SetCost(cost);
        cost
    }
    fn plan(&self) -> &dyn PhysicalPlan {
        self.plan_node.Plan()
    }
    fn attach_children(&mut self, children: &[ImplementationRef]) {
        AttachChildren(self.plan_node.PlanMut(), children);
    }
    fn cost_limit(&self, cost_limit: f64, children: &[ImplementationRef]) -> f64 {
        self.base.GetCostLimit(cost_limit, children)
    }
}

impl_implementation!(TopNImpl);
/// TiDB 侧 TopN 别名。
pub type TiDBTopNImpl = TopNImpl;
/// TiKV 侧 TopN 别名。
pub type TiKVTopNImpl = TopNImpl;

/// UnionAll：并行合并多个分支；代价取子树最大代价 + 并发因子项。
pub struct UnionAllImpl {
    pub(crate) base: BaseImpl,
    plan_node: Box<dyn UnionAllCostPlan>,
}

/// 构造 UnionAll Implementation。
pub fn NewUnionAllImpl(plan: Box<dyn UnionAllCostPlan>) -> UnionAllImpl {
    UnionAllImpl {
        base: BaseImpl::default(),
        plan_node: plan,
    }
}

impl UnionAllImpl {
    /// 自身代价 ≈ (1 + 分支数) × 并发因子，再加上最慢子分支代价。
    fn calc_cost(&self, _out_count: f64, children: &[ImplementationRef]) -> f64 {
        let child_max = children
            .iter()
            .map(|child| child.borrow().GetCost())
            .fold(0.0, f64::max);
        let self_cost = (1 + children.len()) as f64 * self.plan_node.ConcurrencyFactor();
        let cost = self_cost + child_max;
        self.base.SetCost(cost);
        cost
    }
    fn plan(&self) -> &dyn PhysicalPlan {
        self.plan_node.Plan()
    }
    fn attach_children(&mut self, children: &[ImplementationRef]) {
        AttachChildren(self.plan_node.PlanMut(), children);
    }
    /// UnionAll 不向下缩减代价上限，直接透传。
    fn cost_limit(&self, cost_limit: f64, _children: &[ImplementationRef]) -> f64 {
        cost_limit
    }
}

impl_implementation!(UnionAllImpl);

/// Apply：相关子查询的侧向连接；对左表每行驱动右子计划。
pub struct ApplyImpl {
    pub(crate) base: BaseImpl,
    plan_node: Box<dyn ApplyCostPlan>,
}

/// 构造 Apply Implementation。
pub fn NewApplyImpl(plan: Box<dyn ApplyCostPlan>) -> ApplyImpl {
    ApplyImpl {
        base: BaseImpl::default(),
        plan_node: plan,
    }
}

impl ApplyImpl {
    /// 委托计划节点按左右行数与子代价计算总代价。
    fn calc_cost(&self, _out_count: f64, children: &[ImplementationRef]) -> f64 {
        let cost = self.plan_node.SelfCost(
            ChildRows(children, 0),
            ChildRows(children, 1),
            ChildCost(children, 0),
            ChildCost(children, 1),
        );
        self.base.SetCost(cost);
        cost
    }
    fn plan(&self) -> &dyn PhysicalPlan {
        self.plan_node.Plan()
    }
    fn attach_children(&mut self, children: &[ImplementationRef]) {
        AttachChildren(self.plan_node.PlanMut(), children);
    }
    /// 右子计划代价上限 = (总上限 - 左代价) / 有效左行数；
    /// 若有左过滤条件，左行数再乘 SelectionFactor 缩放。
    fn cost_limit(&self, cost_limit: f64, children: &[ImplementationRef]) -> f64 {
        if children.is_empty() {
            return cost_limit;
        }
        let mut left_count = ChildRows(children, 0);
        if self.plan_node.HasLeftConditions() {
            left_count *= SelectionFactor;
        }
        (cost_limit - ChildCost(children, 0)) / left_count
    }
}

impl_implementation!(ApplyImpl);

/// 透传代价（等于唯一子节点代价）的 Implementation 生成宏。
macro_rules! passthrough_implementation {
    ($name:ident, $constructor:ident) => {
        /// 自身无额外代价、直接透传子代价的物理实现。
        pub struct $name {
            pub(crate) base: BaseImpl,
            plan_node: Box<dyn PlanAccess>,
        }

        /// 构造透传代价 Implementation。
        pub fn $constructor(plan: Box<dyn PlanAccess>) -> $name {
            $name {
                base: BaseImpl::default(),
                plan_node: plan,
            }
        }

        impl $name {
            /// 代价等于第一个子节点代价。
            fn calc_cost(&self, _out_count: f64, children: &[ImplementationRef]) -> f64 {
                let cost = ChildCost(children, 0);
                self.base.SetCost(cost);
                cost
            }
            fn plan(&self) -> &dyn PhysicalPlan {
                self.plan_node.Plan()
            }
            fn attach_children(&mut self, children: &[ImplementationRef]) {
                AttachChildren(self.plan_node.PlanMut(), children);
            }
            fn cost_limit(&self, cost_limit: f64, children: &[ImplementationRef]) -> f64 {
                self.base.GetCostLimit(cost_limit, children)
            }
        }

        impl_implementation!($name);
    };
}

// MaxOneRow：保证至多一行输出的算子，代价透传。
passthrough_implementation!(MaxOneRowImpl, NewMaxOneRowImpl);
// Window：窗口函数算子，本实现代价透传。
passthrough_implementation!(WindowImpl, NewWindowImpl);
