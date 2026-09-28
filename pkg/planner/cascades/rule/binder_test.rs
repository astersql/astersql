// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0

// `binder` 模块单元测试：覆盖成功/失败绑定、子树匹配、多组合与 OperandAny 语义。

use crate::*;
use cascades_memo::{GroupExpressionRef, GroupRef, Memo};
use cascades_pattern::*;
use logicalop::{LogicalJoin, LogicalPlanRef, LogicalTableDual};

/// 向 Memo 插入带给定输入 Group 的逻辑计划，返回所在 Group 与表达式。
fn insert(
    memo: &mut Memo,
    plan: LogicalPlanRef,
    inputs: Vec<GroupRef>,
) -> (GroupRef, GroupExpressionRef) {
    let group = memo.NewGroup();
    let expression = memo.NewGroupExpression(plan, inputs);
    let (expression, inserted) = memo.InsertGroupExpression(expression, Some(group.clone()));
    assert!(inserted);
    (group, expression)
}

/// 构造指定行数的 LogicalTableDual（常量行源）。
fn dual(rows: i32) -> LogicalPlanRef {
    Box::new(LogicalTableDual {
        RowCount: rows,
        ..LogicalTableDual::default()
    })
}

/// 构造左右均为 TableDual(1/2) 的 Join 树，返回根 GroupExpression。
fn join_tree(memo: &mut Memo) -> GroupExpressionRef {
    join_tree_with(memo, 1, 2)
}

/// 构造左右 TableDual 行数可配的 Join 树。
fn join_tree_with(memo: &mut Memo, left_rows: i32, right_rows: i32) -> GroupExpressionRef {
    let (left, _) = insert(memo, dual(left_rows), Vec::new());
    let (right, _) = insert(memo, dual(right_rows), Vec::new());
    insert(memo, Box::new(LogicalJoin::default()), vec![left, right]).1
}

/// 构造 Join(left, right) 的 Pattern，引擎为 EngineAll。
fn join_pattern(left: Operand, right: Operand) -> Pattern {
    BuildPattern(
        OperandJoin,
        EngineAll,
        vec![NewPattern(left, EngineAll), NewPattern(right, EngineAll)],
    )
}

/// 左右均为 TableDual 的 Pattern 应成功绑定且仅一个匹配。
#[test]
fn TestBinderSuccess() {
    let mut memo = Memo::NewMemo(&[]);
    let root = join_tree(&mut memo);
    let mut binder = NewBinder(join_pattern(OperandTableDual, OperandTableDual), root);
    assert!(
        binder.GetHolder().is_some(),
        "NewBinder keeps the supplied root as Go's initial holder"
    );
    let bound = binder.Next().expect("join should bind");
    bound.WithWrappedLogicalPlan(|plan| {
        assert!(plan.as_any().is::<LogicalJoin>());
    });
    assert_eq!(bound.Children().len(), 2);
    assert_eq!(child_rows(&bound), (1, 2));
    assert!(binder.GetHolder().is_some());
    assert!(binder.Next().is_none());
}

/// 右孩子 Pattern 为 Projection 时与实际 TableDual 不匹配，应绑定失败。
#[test]
fn TestBinderFail() {
    let mut memo = Memo::NewMemo(&[]);
    let root = join_tree(&mut memo);
    let mut binder = NewBinder(
        join_pattern(OperandTableDual, OperandProjection),
        root.clone(),
    );
    assert!(binder.Next().is_none());
    assert!(
        binder.GetHolder().is_some(),
        "a failed match must not clear Go's initial root holder"
    );

    // A pinned root must not be searched recursively for a matching subtree.
    let mut binder = NewBinder(
        BuildPattern(
            OperandProjection,
            EngineAll,
            vec![BuildPattern(
                OperandLimit,
                EngineAll,
                vec![NewPattern(OperandJoin, EngineAll)],
            )],
        ),
        root,
    );
    assert!(binder.Next().is_none());

    // A root that matches only the outer shape must still fail when its child
    // group contains an expression of the wrong operand.
    let mut nested_memo = Memo::NewMemo(&[]);
    let join_group = insert(
        &mut nested_memo,
        Box::new(LogicalJoin::default()),
        Vec::new(),
    )
    .0;
    let limit_group = insert(
        &mut nested_memo,
        Box::new(logicalop::LogicalLimit::default()),
        vec![join_group],
    )
    .0;
    let projection = insert(
        &mut nested_memo,
        Box::new(logicalop::LogicalProjection::default()),
        vec![limit_group],
    )
    .1;
    let mut binder = NewBinder(
        BuildPattern(
            OperandProjection,
            EngineAll,
            vec![BuildPattern(
                OperandLimit,
                EngineAll,
                vec![NewPattern(OperandJoin, EngineAll)],
            )],
        ),
        projection.clone(),
    );
    assert!(binder.Next().is_some());

    let mut binder = NewBinder(
        BuildPattern(
            OperandProjection,
            EngineAll,
            vec![BuildPattern(
                OperandLimit,
                EngineAll,
                vec![NewPattern(OperandProjection, EngineAll)],
            )],
        ),
        projection,
    );
    assert!(binder.Next().is_none());
}

/// 仅匹配根 Join、不约束孩子时也应成功。
#[test]
fn TestBinderTopNode() {
    let mut memo = Memo::NewMemo(&[]);
    let root = join_tree(&mut memo);
    let mut binder = NewBinder(NewPattern(OperandJoin, EngineAll), root);
    let bound = binder.Next().expect("root join should bind");
    bound.WithWrappedLogicalPlan(|plan| {
        assert!(plan.as_any().is::<LogicalJoin>());
    });
    assert!(bound.Children().is_empty());
}

/// 无输入孩子的孤立 Join 节点仍可被 OperandJoin Pattern 绑定。
#[test]
fn TestBinderOneNode() {
    let mut memo = Memo::NewMemo(&[]);
    let root = insert(&mut memo, Box::new(LogicalJoin::default()), Vec::new()).1;
    let mut binder = NewBinder(NewPattern(OperandJoin, EngineAll), root);
    let bound = binder.Next().expect("isolated join should bind");
    bound.WithWrappedLogicalPlan(|plan| {
        assert!(plan.as_any().is::<LogicalJoin>());
    });
    assert!(bound.Children().is_empty());
}

/// 子树为 Join 时可匹配 Join/Join；要求 TableDual/TableDual 时因根孩子是 Join 而失败。
#[test]
fn TestBinderSubTreeMatch() {
    let mut memo = Memo::NewMemo(&[]);
    let left = join_tree_with(&mut memo, 1, 2)
        .borrow()
        .GetGroup()
        .expect("left owner");
    let right = join_tree_with(&mut memo, 3, 4)
        .borrow()
        .GetGroup()
        .expect("right owner");
    let root = insert(
        &mut memo,
        Box::new(LogicalJoin::default()),
        vec![left, right],
    )
    .1;
    let mut binder = NewBinder(join_pattern(OperandJoin, OperandJoin), root.clone());
    let bound = binder.Next().expect("nested joins should bind");
    bound.WithWrappedLogicalPlan(|plan| {
        assert!(plan.as_any().is::<LogicalJoin>());
    });
    assert_eq!(bound.Children().len(), 2);
    for child in bound.Children() {
        child.WithWrappedLogicalPlan(|plan| {
            assert!(plan.as_any().is::<LogicalJoin>());
        });
        assert!(child.Children().is_empty());
    }
    let mut binder = NewBinder(join_pattern(OperandTableDual, OperandTableDual), root);
    assert!(
        binder.Next().is_none(),
        "binder keeps the supplied root pinned"
    );
}

/// 从绑定的左右孩子读取 TableDual 行数，用于断言组合结果。
fn child_rows(bound: &BoundPlan) -> (i32, i32) {
    let row = |child: &BoundPlan| {
        child.WithWrappedLogicalPlan(|plan| {
            plan.as_any()
                .downcast_ref::<LogicalTableDual>()
                .expect("table dual")
                .RowCount
        })
    };
    (row(&bound.Children()[0]), row(&bound.Children()[1]))
}

/// 左右 Group 各有两条等价式时，应枚举全部 2×2 组合。
#[test]
fn TestBinderMultiNext() {
    let mut memo = Memo::NewMemo(&[]);
    let (left, _) = insert(&mut memo, dual(1), Vec::new());
    let extra_left = memo.NewGroupExpression(dual(3), Vec::new());
    assert!(memo.InsertGroupExpression(extra_left, Some(left.clone())).1);
    let (right, _) = insert(&mut memo, dual(2), Vec::new());
    let extra_right = memo.NewGroupExpression(dual(4), Vec::new());
    assert!(
        memo.InsertGroupExpression(extra_right, Some(right.clone()))
            .1
    );
    let root = insert(
        &mut memo,
        Box::new(LogicalJoin::default()),
        vec![left, right],
    )
    .1;
    let mut binder = NewBinder(join_pattern(OperandTableDual, OperandTableDual), root);
    let mut combinations = Vec::new();
    while let Some(bound) = binder.Next() {
        combinations.push(child_rows(&bound));
    }
    assert_eq!(combinations, vec![(1, 2), (1, 4), (3, 2), (3, 4)]);
}

/// 右孩子为 OperandAny 时只取该 Group 首个表达式，左孩子仍枚举全部。
#[test]
fn TestBinderAny() {
    let mut memo = Memo::NewMemo(&[]);
    let (left, _) = insert(&mut memo, dual(1), Vec::new());
    assert!(
        memo.InsertGroupExpression(
            memo.NewGroupExpression(dual(3), Vec::new()),
            Some(left.clone())
        )
        .1
    );
    let (right, _) = insert(&mut memo, dual(2), Vec::new());
    assert!(
        memo.InsertGroupExpression(
            memo.NewGroupExpression(dual(4), Vec::new()),
            Some(right.clone())
        )
        .1
    );
    let root = insert(
        &mut memo,
        Box::new(LogicalJoin::default()),
        vec![left, right],
    )
    .1;
    let mut binder = NewBinder(join_pattern(OperandTableDual, OperandAny), root);
    let rows = std::iter::from_fn(|| binder.Next())
        .map(|bound| child_rows(&bound))
        .collect::<Vec<_>>();
    assert_eq!(rows, vec![(1, 2), (3, 2)]);
}

/// 左右均为 OperandAny 时各取首个，故仅一个匹配。
#[test]
fn TestBinderMultiAny() {
    let mut memo = Memo::NewMemo(&[]);
    let root = join_tree(&mut memo);
    let mut binder = NewBinder(join_pattern(OperandAny, OperandAny), root);
    let bound = binder.Next().expect("ANY children should bind");
    assert_eq!(child_rows(&bound), (1, 2));
    assert!(binder.Next().is_none());
}

/// 回归：BaseRule / 规则类型名字符串保持与 Go 默认一致。
#[test]
fn rule_type_and_base_rule_keep_go_defaults() {
    let base = NewBaseRule(XFJoinToApply, NewPattern(OperandJoin, EngineTiDBOnly));
    assert_eq!(base.ID(), 0);
    assert_eq!(base.Pattern().Operand, OperandJoin);
    assert_eq!(XFJoinToApply.String(), "join_to_apply");
    assert_eq!(XFDeCorrelateSimpleApply.String(), "default_none");
}
