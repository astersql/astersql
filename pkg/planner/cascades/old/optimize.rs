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

// 旧版 Cascades 优化器：预处理 → 探索变换 → 物理实现三阶段。
//
// Cascades 把逻辑计划转成 memo（等价 Group 集合），用变换规则扩展候选，
// 再按物理属性与代价选出最优物理计划（PhysicalPlan）。

use std::cell::RefCell;
use std::collections::HashMap;

use astersql_expression::Column;
use astersql_planner_cascades_pattern::{self as pattern, Operand};
use astersql_planner_core_base::{PhysicalPlan, PlanContext};
use astersql_planner_core_operator_logicalop::{
    self as logicalop, DataSource, LogicalAggregation, LogicalCTE, LogicalIndexScan, LogicalPlan,
    LogicalPlanRef, LogicalProjection, LogicalSelection, LogicalSort, LogicalTableScan,
    LogicalTopN, MockDataSource, PossiblePropertiesInfo, SortProperties, TiKVSingleGather,
};
use astersql_planner_memo::{
    self as memo, Group, GroupExpr, GroupExprRef, GroupRef, ImplementationRef,
};
use astersql_planner_property::{PhysicalProperty, SortItemsFromCols};

use crate::enforcer_rules::GetEnforcerRules;
use crate::implementation_rules::{ImplementationRule, defaultImplementationMap};
use crate::transformation_rules::{TransformationRuleBatch, default_rule_batches};

/// 优化阶段统一错误类型别名。
type OptimizeResult<T> = Result<T, Box<dyn std::error::Error>>;

thread_local! {
    /// 持有默认规则集的全局 Optimizer（线程局部）。
    /// DefaultOptimizer is the optimizer containing the default rule sets.
    pub static DefaultOptimizer: RefCell<Optimizer> = RefCell::new(Optimizer::NewOptimizer());
}

/// 旧 Cascades 优化器：持有变换规则批次与实现规则映射。
/// Optimizer implements the three old-Cascades phases over the Rust memo.
pub struct Optimizer {
    transformation_rule_batches: Vec<TransformationRuleBatch>,
    implementation_rule_map: HashMap<Operand, Vec<Box<dyn ImplementationRule>>>,
}

impl Optimizer {
    #[allow(non_snake_case)]
    /// 用默认变换批次与实现规则映射构造优化器。
    pub fn NewOptimizer() -> Self {
        Self {
            transformation_rule_batches: default_rule_batches(),
            implementation_rule_map: defaultImplementationMap(),
        }
    }

    #[allow(non_snake_case)]
    /// 替换变换规则批次（测试可注入子集规则）。
    pub fn ResetTransformationRules(&mut self, batches: Vec<TransformationRuleBatch>) -> &mut Self {
        self.transformation_rule_batches = batches;
        self
    }

    #[allow(non_snake_case)]
    /// 替换按 Operand 索引的实现规则映射。
    pub fn ResetImplementationRules(
        &mut self,
        rules: HashMap<Operand, Vec<Box<dyn ImplementationRule>>>,
    ) -> &mut Self {
        self.implementation_rule_map = rules;
        self
    }

    #[allow(non_snake_case)]
    /// 按逻辑算子 Operand 取出候选实现规则。
    pub fn GetImplementationRules(&self, node: &dyn LogicalPlan) -> &[Box<dyn ImplementationRule>] {
        self.implementation_rule_map
            .get(&pattern::GetOperand(node))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    #[allow(non_snake_case)]
    /// 完整三阶段入口：剪枝列 → 探索 memo → 实现物理计划并解析索引。
    pub fn FindBestPlan(
        &self,
        sctx: &dyn PlanContext,
        logical: LogicalPlanRef,
    ) -> OptimizeResult<(Box<dyn PhysicalPlan>, f64)> {
        let logical = self.onPhasePreprocessing(sctx, logical)?;
        let root_group = memo::Convert2Group(logical);
        self.onPhaseExploration(sctx, &root_group)?;
        let (mut plan, cost) = self.onPhaseImplementation(sctx, &root_group)?;
        plan.resolve_indices()?;
        Ok((plan, cost))
    }

    #[allow(non_snake_case)]
    /// 预处理：按输出列做列剪枝（PruneColumns）。
    fn onPhasePreprocessing(
        &self,
        _sctx: &dyn PlanContext,
        mut plan: LogicalPlanRef,
    ) -> OptimizeResult<LogicalPlanRef> {
        let columns = plan.Schema().Columns.clone();
        plan.PruneColumns(&columns)?;
        Ok(plan)
    }

    #[allow(non_snake_case)]
    /// 探索阶段：按批次轮次反复 exploreGroup，直到 Group 标记为已探索。
    fn onPhaseExploration(&self, _sctx: &dyn PlanContext, group: &GroupRef) -> OptimizeResult<()> {
        // 每轮批次独立 ExploreMark；组内循环直到本轮全部表达式探索完毕。
        for (round, batch) in self.transformation_rule_batches.iter().enumerate() {
            while !group.borrow().Explored(round) {
                self.exploreGroup(group, round, batch)?;
            }
        }
        Ok(())
    }

    #[allow(non_snake_case)]
    /// 深度优先探索 Group：先探索子 Group，再对本表达式应用变换规则。
    fn exploreGroup(
        &self,
        group: &GroupRef,
        round: usize,
        batch: &TransformationRuleBatch,
    ) -> OptimizeResult<()> {
        if group.borrow().Explored(round) {
            return Ok(());
        }
        group.borrow_mut().SetExplored(round);

        // Insertion clears the group mark, so a snapshot is sufficient for one pass;
        // the outer loop will visit expressions inserted by this pass.
        let expressions = group.borrow().Equivalents.clone();
        for expression in expressions {
            if !group.borrow().Exists(&expression) || expression.borrow().Explored(round) {
                continue;
            }
            expression.borrow_mut().SetExplored(round);

            let children = expression.borrow().Children.clone();
            for child in children {
                while !child.borrow().Explored(round) {
                    self.exploreGroup(&child, round, batch)?;
                }
            }

            if self.findMoreEquiv(group, &expression, round, batch)? {
                Group::Delete(group, &expression);
            }
        }
        Ok(())
    }

    #[allow(non_snake_case)]
    /// 对当前表达式绑定规则 Pattern，调用 on_transform，写回新等价式。
    fn findMoreEquiv(
        &self,
        group: &GroupRef,
        expression: &GroupExprRef,
        round: usize,
        batch: &TransformationRuleBatch,
    ) -> OptimizeResult<bool> {
        let operand = pattern::GetOperand(expression.borrow().ExprNode.as_ref());
        let Some(rules) = batch.get(&operand) else {
            return Ok(false);
        };
        let mut erase_current = false;

        for rule in rules {
            let rule_pattern = rule.get_pattern();
            if !rule_pattern.Operand.Match(operand) {
                continue;
            }
            let Some(element) = group
                .borrow()
                .Equivalents
                .iter()
                .position(|candidate| std::rc::Rc::ptr_eq(candidate, expression))
            else {
                break;
            };
            let Some(mut binding) = memo::NewExprIterFromGroupElem(group, element, rule_pattern)
            else {
                continue;
            };
            let mut pending = Vec::new();
            while binding.Matched()
                && binding
                    .GetExpr()
                    .as_ref()
                    .is_some_and(|bound| std::rc::Rc::ptr_eq(bound, expression))
            {
                if !rule.matches(&binding) {
                    binding.Next();
                    continue;
                }
                let (new_expressions, erase_old, erase_all) = rule.on_transform(&binding)?;
                // eraseAll：清空 Group 全部等价式后插入新式，并提前结束本表达式。
                if erase_all {
                    Group::DeleteAll(group);
                    for new_expression in new_expressions {
                        Group::Insert(group, new_expression);
                    }
                    group.borrow_mut().SetExplored(round);
                    return Ok(false);
                }

                erase_current |= erase_old;
                pending.extend(new_expressions);
                binding.Next();
            }
            // Group stores expressions in a Vec. Defer inserts until all bindings
            // of the fixed root expression have been enumerated so indices remain stable.
            for new_expression in pending {
                if Group::Insert(group, new_expression) {
                    group.borrow_mut().SetUnexplored(round);
                }
            }
        }
        Ok(erase_current)
    }

    #[allow(non_snake_case)]
    /// 自底向上为 Group 推导统计信息（Stats），供代价估算使用。
    fn fillGroupStats(&self, group: &GroupRef) -> OptimizeResult<()> {
        if group.borrow().Prop.Stats.is_some() {
            return Ok(());
        }
        let expression =
            group.borrow().Equivalents.first().cloned().ok_or_else(|| {
                logicalop::PlannerError("memo group has no expression".to_owned())
            })?;
        let children = expression.borrow().Children.clone();
        let mut mock_children = Vec::with_capacity(children.len());
        for child in children {
            self.fillGroupStats(&child)?;
            let child = child.borrow();
            let stats = child.Prop.Stats.as_deref().cloned().ok_or_else(|| {
                logicalop::PlannerError("child group has no statistics".to_owned())
            })?;
            let schema = child
                .Prop
                .Schema
                .as_deref()
                .map(astersql_expression::Schema::Clone)
                .ok_or_else(|| logicalop::PlannerError("child group has no schema".to_owned()))?;
            let mut mock = MockDataSource::default();
            mock.SetSchema(schema);
            mock.SetStats(stats);
            mock_children.push(Box::new(mock) as LogicalPlanRef);
        }

        let stats_result = {
            let mut expression = expression.borrow_mut();
            expression.ExprNode.SetChildren(mock_children);
            let result = expression.ExprNode.DeriveStats(false);
            expression.ExprNode.TakeChildren();
            result
        };
        let (stats, _) = stats_result?;
        group.borrow_mut().Prop.Stats = Some(Box::new(stats));
        Ok(())
    }

    #[allow(non_snake_case)]
    /// 实现阶段：准备可能物理属性后，在无限代价上限下选取最优物理计划。
    fn onPhaseImplementation(
        &self,
        _sctx: &dyn PlanContext,
        group: &GroupRef,
    ) -> OptimizeResult<(Box<dyn PhysicalPlan>, f64)> {
        let mut property = PhysicalProperty::default();
        property.ExpectedCnt = f64::MAX;
        preparePossibleProperties(group, &mut HashMap::new());
        let implementation = self.implGroup(group, &property, f64::MAX)?.ok_or_else(|| {
            logicalop::PlannerError("Can't find a proper physical plan for this query".to_owned())
        })?;
        let implementation = implementation.borrow();
        let cost = implementation.GetCost();
        let physical = implementation.GetPlan();
        let plan = physical.clone_physical(physical.s_ctx().clone())?;
        Ok((plan, cost))
    }

    #[allow(non_snake_case)]
    /// 在给定物理属性与代价上限下，为 Group 搜索最优 Implementation。
    fn implGroup(
        &self,
        group: &GroupRef,
        required: &PhysicalProperty,
        mut cost_limit: f64,
    ) -> OptimizeResult<Option<ImplementationRef>> {
        if let Some(cached) = group.borrow().GetImpl(required) {
            let within_limit = cached.borrow().GetCost() <= cost_limit;
            return Ok(within_limit.then_some(cached));
        }

        self.fillGroupStats(group)?;
        let out_count = group
            .borrow()
            .Prop
            .Stats
            .as_deref()
            .expect("statistics were filled above")
            .RowCount
            .min(required.ExpectedCnt);
        let mut best: Option<ImplementationRef> = None;
        let expressions = group.borrow().Equivalents.clone();

        for expression in expressions {
            let candidates = {
                let expression = expression.borrow();
                self.implGroupExpr(&expression, required)?
            };
            let child_groups = expression.borrow().Children.clone();
            for candidate in candidates {
                let mut child_implementations = Vec::with_capacity(child_groups.len());
                for (index, child_group) in child_groups.iter().enumerate() {
                    let (child_property, child_limit) = {
                        let candidate = candidate.borrow();
                        (
                            candidate.GetPlan().get_child_req_props(index).clone(),
                            candidate.GetCostLimit(cost_limit, &child_implementations),
                        )
                    };
                    let Some(child) = self.implGroup(child_group, &child_property, child_limit)?
                    else {
                        candidate.borrow_mut().SetCost(f64::MAX);
                        break;
                    };
                    child_implementations.push(child);
                }
                if candidate.borrow().GetCost() == f64::MAX {
                    continue;
                }
                let candidate_cost = candidate
                    .borrow()
                    .CalcCost(out_count, &child_implementations);
                if candidate_cost > cost_limit {
                    continue;
                }
                if best
                    .as_ref()
                    .is_none_or(|current| current.borrow().GetCost() > candidate_cost)
                {
                    candidate
                        .borrow_mut()
                        .AttachChildren(&child_implementations);
                    best = Some(candidate);
                    cost_limit = candidate_cost;
                }
            }
        }

        // Enforcer：在无法自然满足排序等属性时，强制插入 PhysicalSort 等算子。
        for enforcer in GetEnforcerRules(&group.borrow(), required) {
            let relaxed = enforcer.NewProperty(required);
            let enforce_cost = enforcer.GetEnforceCost(&group.borrow());
            let Some(child) = self.implGroup(group, &relaxed, cost_limit - enforce_cost)? else {
                continue;
            };
            let child_cost = child.borrow().GetCost();
            let enforced = enforcer.OnEnforce(required, child);
            let enforced_cost = enforce_cost + child_cost;
            enforced.borrow_mut().SetCost(enforced_cost);
            if best
                .as_ref()
                .is_none_or(|current| current.borrow().GetCost() > enforced_cost)
            {
                best = Some(enforced);
                cost_limit = enforced_cost;
            }
        }

        let Some(best) = best.filter(|implementation| implementation.borrow().GetCost() < f64::MAX)
        else {
            return Ok(None);
        };
        group.borrow_mut().InsertImpl(required, best.clone());
        Ok(Some(best))
    }

    #[allow(non_snake_case)]
    /// 对单条 GroupExpr 匹配实现规则，收集候选 Implementation。
    fn implGroupExpr(
        &self,
        expression: &GroupExpr,
        required: &PhysicalProperty,
    ) -> logicalop::Result<Vec<ImplementationRef>> {
        let mut implementations = Vec::new();
        for rule in self.GetImplementationRules(expression.ExprNode.as_ref()) {
            if rule.Match(expression, required) {
                implementations.extend(rule.OnImplement(expression, required)?);
            }
        }
        Ok(implementations)
    }
}

/// 递归准备并缓存各 Group 可能的排序属性与 TiFlash 可达性。
/// Recursively prepares and memoizes ordering and TiFlash properties.
#[allow(non_snake_case)]
pub fn preparePossibleProperties(
    group: &GroupRef,
    property_map: &mut HashMap<u64, PossiblePropertiesInfo>,
) -> PossiblePropertiesInfo {
    let group_id = group.borrow().ID();
    if let Some(properties) = property_map.get(&group_id) {
        return properties.clone();
    }

    let expressions = group.borrow().Equivalents.clone();
    let mut unique_orders: HashMap<Vec<u8>, Vec<Column>> = HashMap::new();
    let mut has_tiflash = false;
    for expression in expressions {
        let children = expression.borrow().Children.clone();
        let child_properties = children
            .iter()
            .map(|child| preparePossibleProperties(child, property_map))
            .collect::<Vec<_>>();
        let info = prepare_expression_properties(
            &mut *expression.borrow_mut().ExprNode,
            &child_properties,
        );
        has_tiflash |= info.HasTiFlash;
        for order in info.Orders {
            let mut property = PhysicalProperty::default();
            property.SortItems = SortItemsFromCols(&order, true);
            unique_orders.entry(property.HashCode()).or_insert(order);
        }
    }

    let result = PossiblePropertiesInfo {
        Orders: unique_orders.into_values().collect(),
        HasTiFlash: has_tiflash,
    };
    {
        let mut group = group.borrow_mut();
        group.Prop.PossibleProps = result.Orders.clone();
        group.Prop.HasTiFlash = result.HasTiFlash;
    }
    property_map.insert(group_id, result.clone());
    result
}

#[cfg(test)]
#[path = "optimize_test.rs"]
mod optimize_test;

#[cfg(test)]
#[path = "stringer_test.rs"]
mod stringer_test;

/// 按具体逻辑算子类型派发 PreparePossibleProperties。
fn prepare_expression_properties(
    plan: &mut dyn LogicalPlan,
    children: &[PossiblePropertiesInfo],
) -> PossiblePropertiesInfo {
    if let Some(plan) = plan.as_any_mut().downcast_mut::<DataSource>() {
        return plan.PreparePossibleProperties();
    }
    if let Some(plan) = plan.as_any_mut().downcast_mut::<LogicalTableScan>() {
        return plan.PreparePossibleProperties();
    }
    if let Some(plan) = plan.as_any_mut().downcast_mut::<LogicalIndexScan>() {
        return plan.PreparePossibleProperties();
    }
    if let Some(plan) = plan.as_any_mut().downcast_mut::<LogicalCTE>() {
        return plan.PreparePossibleProperties();
    }
    if let Some(plan) = plan.as_any_mut().downcast_mut::<TiKVSingleGather>() {
        return plan.PreparePossibleProperties(children);
    }
    if let Some(plan) = plan.as_any_mut().downcast_mut::<LogicalSelection>() {
        return plan.PreparePossibleProperties(children);
    }

    let sort_children = children
        .iter()
        .map(|child| SortProperties {
            Orders: child.Orders.clone(),
            HasTiFlash: child.HasTiFlash,
        })
        .collect::<Vec<_>>();
    if let Some(plan) = plan.as_any_mut().downcast_mut::<LogicalProjection>() {
        let info = plan
            .PreparePossibleProperties(sort_children.first().unwrap_or(&SortProperties::default()));
        return PossiblePropertiesInfo {
            Orders: info.Orders,
            HasTiFlash: info.HasTiFlash,
        };
    }
    if let Some(plan) = plan.as_any_mut().downcast_mut::<LogicalSort>() {
        let schema = plan.Schema().Clone();
        let info = plan.PreparePossibleProperties(&schema, &sort_children);
        return PossiblePropertiesInfo {
            Orders: info.Orders,
            HasTiFlash: info.HasTiFlash,
        };
    }
    if let Some(plan) = plan.as_any_mut().downcast_mut::<LogicalTopN>() {
        let schema = plan.Schema().Clone();
        let info = plan.PreparePossibleProperties(&schema, &sort_children);
        return PossiblePropertiesInfo {
            Orders: info.Orders,
            HasTiFlash: info.HasTiFlash,
        };
    }
    if let Some(plan) = plan.as_any_mut().downcast_mut::<LogicalAggregation>() {
        let orders = plan.PreparePossibleProperties(
            children
                .first()
                .map_or(&[], |child| child.Orders.as_slice()),
        );
        let has_tiflash = plan.base_mut().PreparePossibleProperties(
            &children
                .iter()
                .map(|child| child.HasTiFlash)
                .collect::<Vec<_>>(),
        );
        return PossiblePropertiesInfo {
            Orders: orders,
            HasTiFlash: has_tiflash,
        };
    }

    let child_tiflash = children
        .iter()
        .map(|child| child.HasTiFlash)
        .collect::<Vec<_>>();
    let has_tiflash = plan.base_mut().PreparePossibleProperties(&child_tiflash);
    PossiblePropertiesInfo {
        Orders: children
            .first()
            .map(|child| child.Orders.clone())
            .unwrap_or_default(),
        HasTiFlash: has_tiflash,
    }
}
