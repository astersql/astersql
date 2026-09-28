// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// Group 与 GroupExpression 的 Memo 行为单元测试。
//
// 覆盖等价表达式去重插入、哈希碰撞下仍按 Equals 区分、删除、
// Group/表达式 Hash64·Equals，以及父子引用（parent GE refs）完整性。

use crate::{Group, Memo};
use logicalop::{LogicalLimit, LogicalPlan, LogicalTableDual};
use std::rc::Rc;

/// 构造带指定 offset/count 的 LogicalLimit（逻辑 Limit 算子）计划引用。
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

fn dual(rows: i32) -> logicalop::LogicalPlanRef {
    Box::new(
        LogicalTableDual {
            RowCount: rows,
            ..LogicalTableDual::default()
        }
        .Init(crate::main_test::context(), 0),
    )
}

/// 相同 Limit 表达式第二次 Insert 应失败；不同参数可并存。
#[test]
fn TestRawHashMap() {
    let mut memo = Memo::NewMemo(&[4]);
    let group = memo.NewGroup();
    let first = memo.NewGroupExpression(limit(1, 10), Vec::new());
    let duplicate = memo.NewGroupExpression(limit(1, 10), Vec::new());
    let distinct = memo.NewGroupExpression(limit(2, 10), Vec::new());
    assert!(Group::Insert(&group, first));
    assert!(!Group::Insert(&group, duplicate));
    assert!(Group::Insert(&group, distinct));
    assert_eq!(group.borrow().GetLogicalExpressions().len(), 2);
}

/// 人为制造 hash64 碰撞：子 Group 顺序不同则仍视为两个不同表达式。
#[test]
fn TestGroupExpressionHashCollision() {
    let mut memo = Memo::NewMemo(&[]);
    let child1 = memo.NewGroup();
    let child2 = memo.NewGroup();
    let root = memo.NewGroup();
    let first = memo.NewGroupExpression(limit(0, 1), vec![child1.clone(), child2.clone()]);
    let second = memo.NewGroupExpression(limit(0, 1), vec![child2.clone(), child1.clone()]);
    // 强制相同哈希，验证 Insert 仍按 Equals 判定而非仅靠哈希。
    first.borrow_mut().hash64 = 1;
    second.borrow_mut().hash64 = 1;
    assert!(Group::Insert(&root, first.clone()));
    assert!(Group::Insert(&root, second.clone()));
    assert_eq!(root.borrow().GetLogicalExpressions().len(), 2);
    for (expression, expected_inputs) in [
        (first, [child1.clone(), child2.clone()]),
        (second, [child2, child1]),
    ] {
        let expression = expression.borrow();
        assert_eq!(expression.GetHash64(), 1);
        assert!(Rc::ptr_eq(
            &expression.GetGroup().expect("inserted expression owner"),
            &root
        ));
        assert!(Rc::ptr_eq(&expression.Inputs[0], &expected_inputs[0]));
        assert!(Rc::ptr_eq(&expression.Inputs[1], &expected_inputs[1]));
    }
}

/// 逐个 Delete 后表达式列表与指针归属正确，最终为空。
#[test]
fn TestGroupExpressionDelete() {
    let mut memo = Memo::NewMemo(&[]);
    let root = memo.NewGroup();
    let first = memo.NewGroupExpression(limit(0, 1), Vec::new());
    let second = memo.NewGroupExpression(limit(1, 1), Vec::new());
    assert!(Group::Insert(&root, first.clone()));
    assert!(Group::Insert(&root, second.clone()));
    let absent = memo.NewGroupExpression(limit(2, 1), Vec::new());
    Group::Delete(&root, &absent);
    assert_eq!(root.borrow().GetLogicalExpressions().len(), 2);
    Group::Delete(&root, &first);
    assert_eq!(root.borrow().GetLogicalExpressions().len(), 1);
    assert!(Rc::ptr_eq(
        &root.borrow().GetLogicalExpressions()[0],
        &second
    ));
    Group::Delete(&root, &second);
    assert!(root.borrow().GetLogicalExpressions().is_empty());
}

/// Group 相等性与 Hash64 仅由 groupID 决定。
#[test]
fn TestGroupHashEquals() {
    let mut memo = Memo::NewMemo(&[]);
    let first = memo.NewGroup();
    let second = memo.NewGroup();
    assert!(!first.borrow().Equals(&second.borrow()));
    second.borrow_mut().groupID = first.borrow().groupID;
    assert!(first.borrow().Equals(&second.borrow()));
    let mut left = cascades_base::NewHashEqualer();
    let mut right = cascades_base::NewHashEqualer();
    first.borrow().Hash64(left.as_mut());
    second.borrow().Hash64(right.as_mut());
    assert_eq!(left.Sum64(), right.Sum64());
    second.borrow_mut().groupID += 1;
    right.Reset();
    second.borrow().Hash64(right.as_mut());
    assert_ne!(left.Sum64(), right.Sum64());
    assert!(!first.borrow().Equals(&second.borrow()));
}

/// 相同算子与相同子 Group 顺序则相等；交换子顺序则哈希与相等性均不同。
#[test]
fn TestGroupExpressionHashEquals() {
    let mut memo = Memo::NewMemo(&[]);
    let child1 = memo.NewGroup();
    let child2 = memo.NewGroup();
    let first = memo.NewGroupExpression(limit(0, 1), vec![child1.clone(), child2.clone()]);
    let equal = memo.NewGroupExpression(limit(0, 1), vec![child1.clone(), child2.clone()]);
    let reordered = memo.NewGroupExpression(limit(0, 1), vec![child2, child1]);
    assert!(first.borrow().Equals(&equal.borrow()));
    assert_eq!(first.borrow().GetHash64(), equal.borrow().GetHash64());
    assert!(!first.borrow().Equals(&reordered.borrow()));
    assert_ne!(first.borrow().GetHash64(), reordered.borrow().GetHash64());
}

/// InsertGroupExpression 后子 Group 应登记父表达式弱引用，且 Check 通过。
#[test]
fn TestGroupParentGERefs() {
    let context = crate::main_test::context();
    let mut middle = LogicalLimit {
        Count: 10,
        ..LogicalLimit::default()
    }
    .Init(context.clone(), 0);
    middle.SetChildren(vec![dual(1)]);
    let mut root_plan = LogicalLimit {
        Offset: 1,
        Count: 5,
        ..LogicalLimit::default()
    }
    .Init(context, 0);
    root_plan.SetChildren(vec![Box::new(middle)]);

    let mut memo = Memo::NewMemo(&[]);
    let root_expression = memo
        .Init(Box::new(root_plan))
        .expect("initialize three-node memo");
    assert_eq!(memo.GetGroups().len(), 3);
    assert_eq!(memo.GetGroupID2Group().len(), 3);
    assert_eq!(memo.global_expression_count(), 3);

    let root = memo.GetRootGroup().expect("root group");
    assert_eq!(root.borrow().parent_count(), 0);
    assert_eq!(root.borrow().GetLogicalExpressions().len(), 1);
    assert!(Rc::ptr_eq(
        &root.borrow().GetLogicalExpressions()[0],
        &root_expression
    ));
    assert_eq!(root_expression.borrow().LogicalPlan.TP(), "Limit");

    let middle = root_expression.borrow().Inputs[0].clone();
    let middle_expression = middle.borrow().GetLogicalExpressions()[0].clone();
    assert_eq!(middle.borrow().parent_count(), 1);
    assert!(
        middle
            .borrow()
            .parentExpressions
            .contains_key(&crate::GroupExpression::addr(&root_expression))
    );
    assert_eq!(middle_expression.borrow().LogicalPlan.TP(), "Limit");

    let leaf = middle_expression.borrow().Inputs[0].clone();
    let leaf_expression = leaf.borrow().GetLogicalExpressions()[0].clone();
    assert_eq!(leaf.borrow().parent_count(), 1);
    assert!(
        leaf.borrow()
            .parentExpressions
            .contains_key(&crate::GroupExpression::addr(&middle_expression))
    );
    assert_eq!(leaf_expression.borrow().LogicalPlan.TP(), "TableDual");

    for group in memo.GetGroups() {
        Group::Check(&group);
    }
}

/// Memo 初始化必须把计划树已提取的 FD 保留到每个 Group 的逻辑属性中。
#[test]
fn TestDeriveLogicalPropPreservesFD() {
    let context = crate::main_test::context();
    let child = Box::new(
        LogicalTableDual {
            RowCount: 1,
            ..LogicalTableDual::default()
        }
        .Init(context.clone(), 0),
    );
    let mut root = LogicalLimit {
        Count: 1,
        ..LogicalLimit::default()
    }
    .Init(context, 0);
    root.SetChildren(vec![child]);
    root.base_mut().ExtractFD();

    let mut memo = Memo::NewMemo(&[]);
    memo.Init(Box::new(root)).expect("initialize memo");

    for group in memo.GetGroups() {
        let group = group.borrow();
        let property = group
            .GetLogicalProperty()
            .expect("new memo group must have logical properties");
        assert!(property.Stats.is_some(), "group stats must be derived");
        assert!(property.Schema.is_some(), "group schema must be derived");
        assert!(property.FD.is_some(), "group FD must be derived");
    }
}
