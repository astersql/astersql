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

// Cascades 旧版强制规则（Enforcer）：在物理属性无法由算子自然满足时注入额外算子。
//
// 典型场景：父节点要求有序输出，而孩子 Group 无法提供排序时，
// `OrderEnforcer` 插入 `PhysicalSort` 并重新计算孩子所需物理属性。

use std::cell::RefCell;
use std::rc::Rc;

use astersql_planner_cascades_pattern::EngineTiDB;
use astersql_planner_core_base::{PhysicalPlan, Plan};
use astersql_planner_core_operator_physicalop::PhysicalSort;
use astersql_planner_implementation::{NewSortImpl, PlanAccess, SortCostPlan};
use astersql_planner_memo::{Group, Implementation, ImplementationRef};
use astersql_planner_property::PhysicalProperty;
use astersql_planner_util::ByItems;

/// 强制规则接口：生成孩子属性、挂上强制算子、估算强制代价。
pub trait Enforcer {
    /// 返回强制后对孩子要求的物理属性（通常清空已满足的排序项）。
    fn NewProperty(&self, property: &PhysicalProperty) -> PhysicalProperty;
    /// 把强制算子包在孩子实现之上，返回新的 Implementation。
    fn OnEnforce(&self, required: &PhysicalProperty, child: ImplementationRef)
    -> ImplementationRef;
    /// 估算在该 Group 上施加强制的代价。
    fn GetEnforceCost(&self, group: &Group) -> f64;
}

/// 按引擎与所需物理属性挑选可用的强制规则列表。
///
/// 当前仅在 TiDB 引擎且存在排序需求时返回 `OrderEnforcer`。
pub fn GetEnforcerRules(group: &Group, property: &PhysicalProperty) -> Vec<&'static dyn Enforcer> {
    if group.EngineType != EngineTiDB || property.IsSortItemEmpty() {
        Vec::new()
    } else {
        vec![&ORDER_ENFORCER]
    }
}

/// 通过插入 PhysicalSort 满足父节点排序需求的强制规则。
pub struct OrderEnforcer;
static ORDER_ENFORCER: OrderEnforcer = OrderEnforcer;

/// 包装 PhysicalSort，供代价模型与实现层访问。
struct SortPlan {
    plan: PhysicalSort,
}

impl PlanAccess for SortPlan {
    fn Plan(&self) -> &dyn PhysicalPlan {
        &self.plan
    }
    fn PlanMut(&mut self) -> &mut dyn PhysicalPlan {
        &mut self.plan
    }
}

impl SortCostPlan for SortPlan {
    fn ExpectedCount(&self) -> f64 {
        self.plan
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetChildReqProps(0)
            .ExpectedCnt
    }

    fn SelfCost(&self, input_rows: f64, schema: &astersql_expression::Schema) -> f64 {
        self.plan.GetCost(input_rows, schema)
    }

    fn InjectProjectionBelowSort(&mut self, child: Box<dyn PhysicalPlan>) -> Box<dyn PhysicalPlan> {
        // 克隆 Sort 并把投影后的孩子挂回去
        let context = self.plan.s_ctx().clone();
        let mut attached = self
            .plan
            .Clone(context)
            .expect("enforcer sort must remain cloneable");
        attached.set_children(vec![child]);
        Box::new(attached)
    }
}

impl Enforcer for OrderEnforcer {
    fn NewProperty(&self, _property: &PhysicalProperty) -> PhysicalProperty {
        // 强制排序后孩子不再需要继承父排序项，只保留最大期望行数
        let mut property = PhysicalProperty::default();
        property.ExpectedCnt = f64::MAX;
        property
    }

    fn OnEnforce(
        &self,
        required: &PhysicalProperty,
        child: ImplementationRef,
    ) -> ImplementationRef {
        let child_plan = child.borrow();
        let context = child_plan.GetPlan().s_ctx().clone();
        let stats = child_plan.GetPlan().stats_info().clone();
        let offset = child_plan.GetPlan().query_block_offset();
        let schema = child_plan.GetPlan().schema().Clone();
        drop(child_plan);

        // 构造 PhysicalSort，ByItems 来自父所需 SortItems
        let mut child_property = PhysicalProperty::default();
        child_property.ExpectedCnt = f64::MAX;
        let mut sort = PhysicalSort::New(context.clone());
        sort.ByItems = required
            .SortItems
            .iter()
            .map(|item| ByItems {
                Expr: Box::new(item.Col.Clone()),
                Desc: item.Desc,
            })
            .collect();
        sort.PhysicalSchemaProducer.SetSchema(schema);
        let sort = sort.Init(context, stats, offset, vec![Box::new(child_property)]);
        let mut implementation = NewSortImpl(Box::new(SortPlan { plan: sort }));
        implementation.AttachChildren(&[child]);
        Rc::new(RefCell::new(implementation))
    }

    fn GetEnforceCost(&self, group: &Group) -> f64 {
        // 用 Group 统计与 Schema 估算一次 Sort 自代价
        let expression = group
            .Equivalents
            .first()
            .expect("a memo group must contain at least one expression")
            .borrow();
        let context = expression
            .ExprNode
            .SCtx()
            .expect("memo expression must retain planner context")
            .clone();
        let stats = group.Prop.Stats.as_deref().cloned().unwrap_or_default();
        let schema = group
            .Prop
            .Schema
            .as_deref()
            .expect("memo group must retain schema");
        let sort = PhysicalSort::New(context.clone()).Init(context, stats.clone(), 0, Vec::new());
        sort.GetCost(stats.RowCount, schema)
    }
}
