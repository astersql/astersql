// Copyright 2026 AsterSQL.

// Memo 核心行为的 AsterSQL 单元测试：Group 管理、探索标记、实现缓存、指纹与 ExprIter。
//
// 覆盖 Cascades 优化器记忆化搜索空间（Memo）中等价类插入/删除、物理实现按
// PhysicalProperty 缓存、表达式指纹与 Pattern 匹配迭代等关键路径。

use crate::*;
use astersql_expression::{ExprBox, NewOne, NewSchema, NewZero};
use astersql_planner_cascades_pattern::{
    BuildPattern, EngineAll, EngineTiDBOnly, NewPattern, OperandAny, OperandProjection,
    OperandSelection,
};
use astersql_planner_core_operator_logicalop::{
    LogicalLimit, LogicalPlanRef, LogicalProjection, LogicalSelection,
};
use astersql_planner_property::PhysicalProperty;
use std::cell::RefCell;
use std::rc::Rc;

/// 测试用物理实现桩：只记录代价，不提供真实 PhysicalPlan。
struct TestImplementation {
    /// 该实现自身的固定代价分量。
    cost: f64,
}

impl Implementation for TestImplementation {
    /// 总代价 = 自身代价 + 输出行数估计（out_count）。
    fn CalcCost(&self, out_count: f64, _children: &[ImplementationRef]) -> f64 {
        self.cost + out_count
    }

    fn SetCost(&mut self, cost: f64) {
        self.cost = cost;
    }

    fn GetCost(&self) -> f64 {
        self.cost
    }

    fn GetPlan(&self) -> &dyn astersql_planner_core_base::PhysicalPlan {
        panic!("plan access is outside this cache-key test")
    }

    fn AttachChildren(&mut self, _children: &[ImplementationRef]) -> &mut dyn Implementation {
        self
    }

    /// 向子节点传递代价上限时减去自身代价。
    fn GetCostLimit(&self, cost_limit: f64, _children: &[ImplementationRef]) -> f64 {
        cost_limit - self.cost
    }
}

/// 构造带常量真/假条件的 LogicalSelection GroupExpr。
fn selection(value: bool) -> GroupExprRef {
    let condition: ExprBox = if value {
        Box::new(NewOne())
    } else {
        Box::new(NewZero())
    };
    NewGroupExpr(Box::new(LogicalSelection {
        Conditions: vec![condition],
        ..LogicalSelection::default()
    }))
}

/// 构造默认 LogicalProjection GroupExpr。
fn projection() -> GroupExprRef {
    NewGroupExpr(Box::new(LogicalProjection::default()))
}

/// 构造指定 Count 的 LogicalLimit GroupExpr。
fn limit(count: u64) -> GroupExprRef {
    NewGroupExpr(Box::new(LogicalLimit {
        Count: count,
        ..LogicalLimit::default()
    }))
}

/// 用空 Schema 将单个表达式包装成 Group。
fn group(expression: GroupExprRef) -> GroupRef {
    NewGroupWithSchema(expression, &NewSchema(Vec::new()))
}

/// 验证 Group 按 Operand 类型聚集等价表达式，且同指纹拒绝重复插入。
#[test]
fn group_owns_expressions_and_keeps_operand_ranges_contiguous() {
    let first = selection(false);
    let selections = group(first.clone());
    let middle = projection();
    let second = selection(true);
    assert!(Group::Insert(&selections, middle));
    assert!(Group::Insert(&selections, second.clone()));

    // 同类 Operand 应连续排列：两个 Selection 后跟 Projection。
    let operands = selections
        .borrow()
        .Equivalents
        .iter()
        .map(|expression| {
            astersql_planner_cascades_pattern::GetOperand(expression.borrow().ExprNode.as_ref())
        })
        .collect::<Vec<_>>();
    assert_eq!(
        operands,
        vec![OperandSelection, OperandSelection, OperandProjection]
    );
    assert!(Rc::ptr_eq(
        &second.borrow().Group.upgrade().unwrap(),
        &selections
    ));
    // 语义等价的 Selection 不得再次插入。
    assert!(!Group::Insert(&selections, selection(true)));

    Group::Delete(&selections, &first);
    assert_eq!(selections.borrow().GetFirstElem(OperandSelection), Some(0));
    assert!(first.borrow().Group.upgrade().is_none());
}

/// 验证 ExploreMark 可按轮次独立置位/清除已探索状态。
#[test]
fn explore_mark_sets_and_clears_independent_rounds() {
    let mut mark = ExploreMark::default();
    assert!(!mark.Explored(0));
    assert!(!mark.Explored(3));
    mark.SetExplored(0);
    mark.SetExplored(3);
    assert!(mark.Explored(0));
    assert!(mark.Explored(3));
    mark.SetUnexplored(3);
    assert!(mark.Explored(0));
    assert!(!mark.Explored(3));
}

/// 验证物理实现缓存键包含完整 PhysicalProperty 哈希（含 ExpectedCnt）。
#[test]
fn implementation_cache_keys_by_full_physical_property_hash() {
    let expressions = group(limit(1));
    let property = PhysicalProperty::default();
    let mut different = PhysicalProperty::default();
    different.ExpectedCnt = 7.0;
    let implementation: ImplementationRef = Rc::new(RefCell::new(TestImplementation { cost: 3.0 }));
    expressions
        .borrow_mut()
        .InsertImpl(&property, implementation.clone());

    let cached = expressions.borrow().GetImpl(&property).unwrap();
    assert!(Rc::ptr_eq(&cached, &implementation));
    // ExpectedCnt 不同则视为不同物理需求，缓存不应命中。
    assert!(expressions.borrow().GetImpl(&different).is_none());
}

/// 验证指纹依赖子 Group 身份，且 AppliedRule 集合可独立查询。
#[test]
fn fingerprint_uses_child_group_identity_and_rule_ids() {
    let left = group(limit(1));
    let right = group(limit(1));
    let first = projection();
    first.borrow_mut().SetChildren(vec![left]);
    let second = projection();
    second.borrow_mut().SetChildren(vec![right]);

    // 子 Group 不同 → 指纹不同，即使算子类型相同。
    assert_ne!(
        first.borrow_mut().FingerPrint(),
        second.borrow_mut().FingerPrint()
    );
    first.borrow_mut().AddAppliedRule(17);
    assert!(first.borrow().HasAppliedRule(17));
    assert!(!first.borrow().HasAppliedRule(18));
}

/// 验证 ExprIter 以最右子节点优先的笛卡尔序枚举，再推进根。
#[test]
fn iterator_enumerates_rightmost_child_first_then_root() {
    let left = group(selection(false));
    assert!(Group::Insert(&left, selection(true)));
    let right = group(selection(false));
    assert!(Group::Insert(&right, selection(true)));

    let root_expression = projection();
    root_expression
        .borrow_mut()
        .SetChildren(vec![left.clone(), right.clone()]);
    let root = group(root_expression);
    // Pattern：Projection 下挂两个 Selection（任意引擎）。
    let pattern = BuildPattern(
        OperandProjection,
        EngineTiDBOnly,
        vec![
            NewPattern(OperandSelection, EngineAll),
            NewPattern(OperandSelection, EngineAll),
        ],
    );
    let mut iterator = NewExprIterFromGroupElem(&root, 0, &pattern).unwrap();

    let mut positions = Vec::new();
    loop {
        positions.push((
            iterator.Children[0].Element.unwrap(),
            iterator.Children[1].Element.unwrap(),
        ));
        if !iterator.Next() {
            break;
        }
    }
    // 右子优先：(0,0)(0,1)(1,0)(1,1)。
    assert_eq!(positions, vec![(0, 0), (0, 1), (1, 0), (1, 1)]);
    assert!(!iterator.Matched());
}

/// OperandAny 匹配整个 Group 而不绑定具体表达式。
#[test]
fn any_matches_group_without_binding_an_expression() {
    let root = group(limit(1));
    let pattern = NewPattern(OperandAny, EngineTiDBOnly);
    let mut iterator = NewExprIterFromGroupElem(&root, 0, &pattern).unwrap();
    assert!(iterator.Matched());
    assert!(!iterator.Next());
}

/// Convert2Group 切断计划树子节点链接，BuildKeyInfo 幂等构建属性。
#[test]
fn convert_detaches_plan_children_and_builds_key_info_once() {
    let child: LogicalPlanRef = Box::new(LogicalLimit {
        Count: 1,
        ..LogicalLimit::default()
    });
    let mut parent = LogicalProjection::default();
    use astersql_planner_core_operator_logicalop::LogicalPlan as _;
    parent.SetChildren(vec![child]);
    let converted = Convert2Group(Box::new(parent));

    let expression = converted.borrow().Equivalents[0].clone();
    assert_eq!(expression.borrow().Children.len(), 1);
    // 连续两次 BuildKeyInfo 应安全（幂等）。
    BuildKeyInfo(&converted);
    BuildKeyInfo(&converted);
    assert_eq!(
        converted
            .borrow()
            .Prop
            .Schema
            .as_ref()
            .unwrap()
            .Columns
            .len(),
        0
    );
}
