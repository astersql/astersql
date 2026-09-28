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

// Sort 相关物理 Implementation：真实排序 [`SortImpl`] 与名义排序 [`NominalSortImpl`]。
//
// NominalSort 表示输入已满足排序性质、无需实际排序的“名义”排序算子，
// 代价透传子树；真实 Sort 会按期望行数估算排序代价，并可在下方注入 Projection。

use astersql_planner_core_base::PhysicalPlan;
use astersql_planner_memo::ImplementationRef;

use crate::{BaseImpl, ChildCost, ClonePlan, PlanAccess, SortCostPlan, impl_implementation};

/// 真实排序物理实现：挂接子节点时可注入 Projection，代价含排序自身开销。
pub struct SortImpl {
    pub(crate) base: BaseImpl,
    plan_node: Box<dyn SortCostPlan>,
    /// 挂接子节点后得到的最终物理计划（可能含注入的 Projection）。
    attached_plan: Option<Box<dyn PhysicalPlan>>,
}

/// 构造 Sort Implementation。
pub fn NewSortImpl(plan: Box<dyn SortCostPlan>) -> SortImpl {
    SortImpl {
        base: BaseImpl::default(),
        plan_node: plan,
        attached_plan: None,
    }
}

impl SortImpl {
    /// 排序行数取子统计行数与 ExpectedCount 的较小值，再加子代价。
    fn calc_cost(&self, _out_count: f64, children: &[ImplementationRef]) -> f64 {
        let child = children[0].borrow();
        let child_rows = child.GetPlan().stats_info().RowCount;
        let expected_count = self.plan_node.ExpectedCount();
        let count = if child_rows.is_nan() || expected_count.is_nan() {
            f64::NAN
        } else {
            child_rows.min(expected_count)
        };
        let cost = self.plan_node.SelfCost(count, child.GetPlan().schema()) + child.GetCost();
        self.base.SetCost(cost);
        cost
    }
    /// 优先返回挂接后的计划，否则返回原始 Sort 节点。
    fn plan(&self) -> &dyn PhysicalPlan {
        self.attached_plan
            .as_deref()
            .unwrap_or_else(|| self.plan_node.Plan())
    }
    /// 克隆子计划并注入 Sort 下方所需的 Projection。
    fn attach_children(&mut self, children: &[ImplementationRef]) {
        let child = ClonePlan(children[0].borrow().GetPlan());
        self.attached_plan = Some(self.plan_node.InjectProjectionBelowSort(child));
    }
    fn cost_limit(&self, cost_limit: f64, children: &[ImplementationRef]) -> f64 {
        self.base.GetCostLimit(cost_limit, children)
    }
}

impl_implementation!(SortImpl);

/// 名义排序：不执行真实排序，仅标记顺序属性；代价等于子代价。
pub struct NominalSortImpl {
    pub(crate) base: BaseImpl,
    plan_node: Box<dyn PlanAccess>,
    /// 挂接后缓存的子物理计划。
    attached_plan: Option<Box<dyn PhysicalPlan>>,
}

/// 构造 NominalSort Implementation。
pub fn NewNominalSortImpl(plan: Box<dyn PlanAccess>) -> NominalSortImpl {
    NominalSortImpl {
        base: BaseImpl::default(),
        plan_node: plan,
        attached_plan: None,
    }
}

impl NominalSortImpl {
    /// 委托 BaseImpl 对子代价求和。
    fn calc_cost(&self, out_count: f64, children: &[ImplementationRef]) -> f64 {
        self.base.CalcCost(out_count, children)
    }
    fn plan(&self) -> &dyn PhysicalPlan {
        self.attached_plan
            .as_deref()
            .unwrap_or_else(|| self.plan_node.Plan())
    }
    /// 直接挂接子计划克隆，并把代价设为子代价。
    fn attach_children(&mut self, children: &[ImplementationRef]) {
        self.attached_plan = Some(ClonePlan(children[0].borrow().GetPlan()));
        self.base.SetCost(ChildCost(children, 0));
    }
    fn cost_limit(&self, cost_limit: f64, children: &[ImplementationRef]) -> f64 {
        self.base.GetCostLimit(cost_limit, children)
    }
}

impl_implementation!(NominalSortImpl);
