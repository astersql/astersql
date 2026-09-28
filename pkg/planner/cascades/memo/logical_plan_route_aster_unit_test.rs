// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// GroupExpression 与逻辑计划（LogicalPlan）契约路由测试。
//
// 确认 Memo 包装后的组表达式仍可通过 `LogicalPlan` trait 路由：
// 类型、TP、子节点为空（输入是 Group 而非重复逻辑树），
// 且能取回内部包裹的具体逻辑算子。

use crate::{GroupExpression, Memo};
use logicalop::{LogicalLimit, LogicalPlan};

/// 验证 GroupExpression 作为 LogicalPlan 路由时保留包装与具体算子字段。
#[test]
fn group_expression_uses_the_canonical_logical_plan_contract() {
    let memo = Memo::NewMemo(&[]);
    // 构造 Limit 逻辑算子并装入新的组表达式
    let expression = memo.NewGroupExpression(
        Box::new(
            LogicalLimit {
                Offset: 7,
                Count: 11,
                ..LogicalLimit::default()
            }
            .Init(crate::main_test::context(), 0),
        ),
        Vec::new(),
    );
    let mut expression = expression.borrow_mut();

    // 通过 LogicalPlan trait 对象路由，应仍是 GroupExpression 包装
    let routed: &mut dyn LogicalPlan = &mut *expression;
    assert!(routed.as_any().downcast_ref::<GroupExpression>().is_some());
    assert_eq!(routed.TP(), logicalop::TypeLimit);
    // Memo 中子输入是 Group，不会再挂一份重复的逻辑子树
    assert!(
        routed.Children().is_empty(),
        "memo inputs are groups, not a duplicate logical tree"
    );

    // 取回包装内的具体 LogicalLimit，核对 Offset/Count
    let wrapped = routed
        .as_any()
        .downcast_ref::<GroupExpression>()
        .expect("the physical router must retain the memo wrapper")
        .GetWrappedLogicalPlan();
    let limit = wrapped
        .as_any()
        .downcast_ref::<LogicalLimit>()
        .expect("the wrapper must retain the concrete logical operator");
    assert_eq!(wrapped.ID(), limit.ID());
    assert_eq!((limit.Offset, limit.Count), (7, 11));
}
