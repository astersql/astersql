// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0

// `rule_set` 的 Aster 单元测试。
//
// 验证 `ListRules::Filter` 在掩码过滤后仍保持注册顺序与规则 ID，
// 以及 `OperandRules::Filter` 仅对带解相关中间标志的 Apply 走短路子集。

use crate::*;
use cascades_memo::Memo;
use cascades_pattern::{EngineAll, NewPattern, OperandApply};
use cascades_rule::{BaseRule, BoundPlan, NewBaseRule, NewBinder, Rule, RuleError, Type};
use logicalop::{
    APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG, LogicalApply, LogicalPlan, LogicalPlanRef,
};
use std::collections::BTreeMap;

/// 测试用规则桩：固定 ID，Pattern 指向 Apply，XForm 空操作。
struct TestRule {
    id: usize,
    base: BaseRule,
}

impl TestRule {
    /// 构造指定 ID 的测试规则。
    fn new(id: usize) -> Self {
        Self {
            id,
            base: NewBaseRule(Type::DefaultNone, NewPattern(OperandApply, EngineAll)),
        }
    }
}

impl Rule for TestRule {
    fn ID(&self) -> usize {
        self.id
    }
    fn String(&self, writer: &mut dyn cascades_util::StrBufferWriter) {
        self.base.String(writer);
    }
    fn Pattern(&self) -> &cascades_pattern::Pattern {
        self.base.Pattern()
    }
    fn XForm(&self, _plan: &BoundPlan) -> Result<(Vec<LogicalPlanRef>, bool), RuleError> {
        Ok((Vec::new(), false))
    }
}

/// 掩码过滤应保留原列表相对顺序，且只留下掩码内的规则 ID。
#[test]
fn list_filter_preserves_order_and_rule_ids() {
    let rules = ListRules::New(vec![
        Box::new(TestRule::new(3)),
        Box::new(TestRule::new(1)),
        Box::new(TestRule::new(2)),
    ]);
    let selected = rules.Filter(&RuleMask::New([1, 3]));
    assert_eq!(
        selected.iter().map(|rule| rule.ID()).collect::<Vec<_>>(),
        vec![3, 1]
    );
}

/// 构造可选地带解相关中间标志的 Apply 组表达式。
fn expression(flagged: bool) -> cascades_memo::GroupExpressionRef {
    let mut plan = LogicalApply::default();
    if flagged {
        plan.base_mut()
            .SetFlag(APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG);
    }
    Memo::NewMemo(&[]).NewGroupExpression(Box::new(plan), Vec::new())
}

/// 仅中间 Apply 走解相关子集；普通路径用完整列表；默认规则集为空。
#[test]
fn operand_filter_uses_short_path_only_for_intermediate_apply() {
    let mut subsets = BTreeMap::new();
    subsets.insert(
        XFSetDeCorrelateApply,
        ListRules::New(vec![Box::new(TestRule::new(7))]),
    );
    let rules = NewOperandRules(subsets, ListRules::New(vec![Box::new(TestRule::new(8))]));
    assert_eq!(
        rules.Filter(&expression(false)).Iter().next().unwrap().ID(),
        8
    );
    assert_eq!(
        rules.Filter(&expression(true)).Iter().next().unwrap().ID(),
        7
    );
    assert!(DefaultRuleSets().is_empty());
}

/// Go 侧 Apply 规则集合应注册同一个解相关规则，并保持 map/list 一致。
#[test]
fn apply_rule_sets_register_the_decorrelation_rule() {
    let map = OperandApplyRulesMap();
    let list = OperandApplyRulesList();
    let rules = OperandApplyRules();

    assert_eq!(map.len(), 1);
    assert_eq!(list.Iter().count(), 1);
    assert_eq!(rules.Filter(&expression(false)).Iter().count(), 1);

    let map_rule_id = map
        .get(&XFSetDeCorrelateApply)
        .unwrap()
        .Iter()
        .next()
        .unwrap()
        .ID();
    let list_rule_id = list.Iter().next().unwrap().ID();
    let default_rule_id = rules.Filter(&expression(false)).Iter().next().unwrap().ID();
    assert_eq!(map_rule_id, list_rule_id);
    assert_eq!(list_rule_id, default_rule_id);
    assert_eq!(default_rule_id, 2);
}

/// 注册的解相关规则应对无相关列 Apply 产生 Join，而不是空替代结果。
#[test]
fn registered_decorrelation_rule_rewrites_uncorrelated_apply() {
    let mut memo = Memo::NewMemo(&[]);
    let mut apply = LogicalApply::default();
    apply.SetChildren(vec![
        Box::new(logicalop::LogicalTableDual {
            RowCount: 1,
            ..Default::default()
        }),
        Box::new(logicalop::LogicalTableDual {
            RowCount: 2,
            ..Default::default()
        }),
    ]);
    let expression = memo.CopyIn(None, Box::new(apply)).unwrap();

    let rules = OperandApplyRulesList();
    let rule = rules.Iter().next().unwrap();
    let bound = NewBinder(rule.Pattern().clone(), expression)
        .Next()
        .expect("Apply pattern should bind two child groups");
    let (alternatives, remove) = rule.XForm(&bound).unwrap();

    assert!(!remove);
    let [join] = alternatives.as_slice() else {
        panic!("expected one Join alternative");
    };
    assert_eq!(join.TP(), "Join");
    assert!(join.Children().is_empty());
}
