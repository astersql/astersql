// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0

// `pattern` 模块的单元测试：覆盖 Operand 映射、匹配语义与 Pattern 构造。

use crate::*;

/// 验证常见逻辑计划类型经 `GetOperand` 映射到正确 Operand。
#[test]
fn TestGetOperand() {
    let cases: Vec<(Box<dyn logicalop::LogicalPlan>, Operand)> = vec![
        (Box::new(logicalop::LogicalJoin::default()), OperandJoin),
        (
            Box::new(logicalop::LogicalAggregation::default()),
            OperandAggregation,
        ),
        (
            Box::new(logicalop::LogicalProjection::default()),
            OperandProjection,
        ),
        (
            Box::new(logicalop::LogicalSelection::default()),
            OperandSelection,
        ),
        (Box::new(logicalop::LogicalApply::default()), OperandApply),
        (
            Box::new(logicalop::LogicalMaxOneRow::default()),
            OperandMaxOneRow,
        ),
        (
            Box::new(logicalop::LogicalTableDual::default()),
            OperandTableDual,
        ),
        (
            Box::new(logicalop::DataSource::default()),
            OperandDataSource,
        ),
        (
            Box::new(logicalop::LogicalUnionScan::default()),
            OperandUnionScan,
        ),
        (
            Box::new(logicalop::LogicalUnionAll::default()),
            OperandUnionAll,
        ),
        (Box::new(logicalop::LogicalSort::default()), OperandSort),
        (Box::new(logicalop::LogicalTopN::default()), OperandTopN),
        (Box::new(logicalop::LogicalLock::default()), OperandLock),
        (Box::new(logicalop::LogicalLimit::default()), OperandLimit),
    ];
    for (plan, expected) in cases {
        assert_eq!(GetOperand(plan.as_ref()), expected);
    }
}

/// 验证 Operand::Match：Any 与任意算子互配，同类型自配，异类型不配。
#[test]
fn TestOperandMatch() {
    for operand in [
        OperandLimit,
        OperandSelection,
        OperandJoin,
        OperandMaxOneRow,
        OperandAny,
    ] {
        assert!(OperandAny.Match(operand));
        assert!(operand.Match(OperandAny));
        assert!(operand.Match(operand));
    }
    assert!(!OperandLimit.Match(OperandSelection));
    assert!(!OperandLimit.Match(OperandJoin));
    assert!(!OperandLimit.Match(OperandMaxOneRow));
}

/// 验证 `NewPattern` 正确设置 Operand 且初始无子节点。
#[test]
fn TestNewPattern() {
    let pattern = NewPattern(OperandAny, EngineAll);
    assert_eq!(pattern.Operand, OperandAny);
    assert!(pattern.Children.is_empty());
    let pattern = NewPattern(OperandJoin, EngineAll);
    assert_eq!(pattern.Operand, OperandJoin);
    assert!(pattern.Children.is_empty());
}

/// 验证 `SetChildren` 可挂载单子节点或多子节点模式树。
#[test]
fn TestPatternSetChildren() {
    let mut pattern = NewPattern(OperandAny, EngineAll);
    pattern.SetChildren(vec![NewPattern(OperandLimit, EngineAll)]);
    assert_eq!(pattern.Children.len(), 1);
    assert_eq!(pattern.Children[0].Operand, OperandLimit);
    assert!(pattern.Children[0].Children.is_empty());

    let mut pattern = NewPattern(OperandJoin, EngineAll);
    pattern.SetChildren(vec![
        NewPattern(OperandProjection, EngineAll),
        NewPattern(OperandSelection, EngineAll),
    ]);
    assert_eq!(pattern.Children.len(), 2);
    assert_eq!(pattern.Children[0].Operand, OperandProjection);
    assert_eq!(pattern.Children[1].Operand, OperandSelection);
}
