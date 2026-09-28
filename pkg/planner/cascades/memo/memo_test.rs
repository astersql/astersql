// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// Memo 图核心行为单元测试。
//
// 覆盖 Init/CopyIn 建图、InsertGroupExpression 去重与合并、
// mergeGroup（含递归合并父表达式）、以及 IteratorLP 笛卡尔积枚举。

use crate::{Group, Memo};
use logicalop::{LogicalLimit, LogicalPlan, LogicalTableDual};
use std::collections::HashSet;
use std::rc::Rc;

/// 构造默认测试上下文下的 LogicalLimit。
fn limit(offset: u64, count: u64) -> logicalop::LogicalPlanRef {
    Box::new(
        LogicalLimit {
            Offset: offset,
            Count: count,
            ..LogicalLimit::default()
        }
        .Init(crate::main_test::context(), 0),
    )
}

/// 构造默认测试上下文下的 LogicalTableDual（内存行集）。
fn dual(rows: i32) -> logicalop::LogicalPlanRef {
    Box::new(
        LogicalTableDual {
            RowCount: rows,
            ..LogicalTableDual::default()
        }
        .Init(crate::main_test::context(), 0),
    )
}

/// 使用指定 PlanContext 构造 LogicalLimit（保证同一上下文下 ID 分配一致）。
fn limit_with(
    context: &core_base::ContextRef,
    offset: u64,
    count: u64,
) -> logicalop::LogicalPlanRef {
    Box::new(
        LogicalLimit {
            Offset: offset,
            Count: count,
            ..LogicalLimit::default()
        }
        .Init(context.clone(), 0),
    )
}

/// 使用指定 PlanContext 构造 LogicalTableDual。
fn dual_with(context: &core_base::ContextRef, rows: i32) -> logicalop::LogicalPlanRef {
    Box::new(
        LogicalTableDual {
            RowCount: rows,
            ..LogicalTableDual::default()
        }
        .Init(context.clone(), 0),
    )
}

/// Init 一棵 Limit→Dual 树后应得到两个 Group，且根有一个输入。
#[test]
fn TestMemo() {
    let mut root = LogicalLimit {
        Count: 1,
        ..LogicalLimit::default()
    }
    .Init(crate::main_test::context(), 0);
    root.SetChildren(vec![dual(2)]);
    let mut memo = Memo::NewMemo(&[]);
    let root_expression = memo.Init(Box::new(root)).expect("initialize memo");
    assert_eq!(memo.GetGroups().len(), 2);
    assert_eq!(memo.GetGroupID2Group().len(), 2);
    assert_eq!(root_expression.borrow().InputsLen(), 1);
    assert!(memo.GetRootGroup().is_some());
    for (index, group) in memo.GetGroups().iter().enumerate() {
        assert_eq!(group.borrow().GetGroupID(), (index + 1) as u64);
        Group::Check(group);
    }
}

/// 插入重复表达式应去重并合并 Group；RemoveOut 后标记 abandoned。
#[test]
fn TestInsertGE() {
    let mut memo = Memo::NewMemo(&[]);
    let source = memo.NewGroup();
    let target = memo.NewGroup();
    let first = memo.NewGroupExpression(limit(1, 10), Vec::new());
    let duplicate = memo.NewGroupExpression(limit(1, 10), Vec::new());
    let (first, inserted) = memo.InsertGroupExpression(first, Some(source.clone()));
    assert!(inserted);
    // 相同 Limit 再次插入到另一 Group：应复用并合并，只剩 1 个 Group
    let (existing, inserted) = memo.InsertGroupExpression(duplicate, Some(target.clone()));
    assert!(!inserted);
    assert!(Rc::ptr_eq(&first, &existing));
    assert_eq!(memo.GetGroups().len(), 1);
    assert!(Rc::ptr_eq(
        &existing.borrow().GetGroup().expect("moved owner"),
        &target
    ));

    memo.RemoveOut(&target, &existing);
    assert!(existing.borrow().IsAbandoned());
    assert!(target.borrow().GetLogicalExpressions().is_empty());
    assert_eq!(memo.global_expression_count(), 0);
}

/// 合并 source→target 后，父表达式输入应改指向 target，表达式迁入 target。
#[test]
fn TestMergeGroup() {
    let mut memo = Memo::NewMemo(&[]);
    let source = memo.NewGroup();
    let target = memo.NewGroup();
    let parent_group = memo.NewGroup();
    let source_expression = memo.NewGroupExpression(limit(1, 1), Vec::new());
    let target_expression = memo.NewGroupExpression(limit(2, 1), Vec::new());
    memo.InsertGroupExpression(source_expression, Some(source.clone()));
    memo.InsertGroupExpression(target_expression, Some(target.clone()));
    let parent = memo.NewGroupExpression(limit(10, 1), vec![source.clone()]);
    memo.InsertGroupExpression(parent.clone(), Some(parent_group));

    memo.mergeGroup(&source, &target);
    assert_eq!(memo.GetGroups().len(), 2);
    assert!(Rc::ptr_eq(&parent.borrow().Inputs[0], &target));
    assert_eq!(source.borrow().GetLogicalExpressions().len(), 0);
    assert_eq!(target.borrow().GetLogicalExpressions().len(), 2);
    assert_eq!(target.borrow().parent_count(), 1);
    Group::Check(&target);
}

/// 两边父表达式在孩子合并后变得等价，应触发递归合并父 Group。
#[test]
fn TestRecursiveMergeGroup() {
    let mut memo = Memo::NewMemo(&[]);
    let source = memo.NewGroup();
    let target = memo.NewGroup();
    let source_parent = memo.NewGroup();
    let target_parent = memo.NewGroup();
    memo.InsertGroupExpression(
        memo.NewGroupExpression(limit(1, 1), Vec::new()),
        Some(source.clone()),
    );
    memo.InsertGroupExpression(
        memo.NewGroupExpression(limit(2, 1), Vec::new()),
        Some(target.clone()),
    );
    // 两侧父节点都是 Limit(10,1) 且分别以 source/target 为唯一孩子
    memo.InsertGroupExpression(
        memo.NewGroupExpression(limit(10, 1), vec![source.clone()]),
        Some(source_parent.clone()),
    );
    memo.InsertGroupExpression(
        memo.NewGroupExpression(limit(10, 1), vec![target.clone()]),
        Some(target_parent.clone()),
    );

    memo.mergeGroup(&source, &target);
    assert_eq!(memo.GetGroups().len(), 2);
    // source 与 source_parent 应从 ID 索引中消失
    assert!(
        !memo
            .GetGroupID2Group()
            .contains_key(&source.borrow().GetGroupID())
    );
    assert!(
        !memo
            .GetGroupID2Group()
            .contains_key(&source_parent.borrow().GetGroupID())
    );
    assert_eq!(target_parent.borrow().GetLogicalExpressions().len(), 1);
    assert_eq!(target.borrow().GetLogicalExpressions().len(), 2);
}

/// 根 2 个 Limit × 孩子 2 个 Dual = 4 条备选，且 PlanIDsHash 两两不同。
#[test]
fn TestIteratorLogicalPlan() {
    let mut memo = Memo::NewMemo(&[]);
    let context = crate::main_test::context();
    let child = memo.NewGroup();
    let root = memo.NewGroup();
    memo.InsertGroupExpression(
        memo.NewGroupExpression(dual_with(&context, 1), Vec::new()),
        Some(child.clone()),
    );
    memo.InsertGroupExpression(
        memo.NewGroupExpression(dual_with(&context, 0), Vec::new()),
        Some(child.clone()),
    );
    memo.InsertGroupExpression(
        memo.NewGroupExpression(limit_with(&context, 0, 1), vec![child.clone()]),
        Some(root.clone()),
    );
    memo.InsertGroupExpression(
        memo.NewGroupExpression(limit_with(&context, 1, 1), vec![child]),
        Some(root.clone()),
    );
    assert_eq!(root.borrow().GetLogicalExpressions().len(), 2);
    assert_eq!(
        root.borrow().GetLogicalExpressions()[0].borrow().Inputs[0]
            .borrow()
            .GetLogicalExpressions()
            .len(),
        2
    );
    memo.set_root_group(root);

    let mut iterator = memo.NewIterator();
    let mut alternatives = Vec::new();
    iterator.Each(|alternative| {
        alternatives.push(alternative.clone());
        true
    });
    assert_eq!(alternatives.len(), 4);
    assert!(
        alternatives
            .iter()
            .all(|alternative| alternative.Children.len() == 1)
    );
    assert_eq!(
        alternatives
            .iter()
            .map(|alternative| alternative.PlanIDsHash)
            .collect::<HashSet<_>>()
            .len(),
        4
    );
}
