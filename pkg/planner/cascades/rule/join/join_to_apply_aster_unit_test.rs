// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0

// `join_to_apply` 的 Aster 单元测试：核对 Pattern 形状与 TODO 式空 XForm 结果。

use crate::*;
use cascades_pattern::*;
use cascades_rule::{NewBinder, Rule};

/// 验证 NewJoinToApply 的 Pattern/引擎/ID 与 Go 一致，且 XForm 返回空替代。
#[test]
fn join_to_apply_keeps_go_pattern_and_todo_result() {
    let rule = NewJoinToApply();
    let pattern = rule.Pattern();
    assert_eq!(pattern.Operand, OperandJoin);
    assert_eq!(pattern.EngineTypeSet, EngineTiDBOnly);
    assert_eq!(pattern.Children.len(), 2);
    assert_eq!(pattern.Children[0].Operand, OperandAny);
    assert_eq!(pattern.Children[0].EngineTypeSet, EngineAll);
    assert_eq!(pattern.Children[1].Operand, OperandJoin);
    assert_eq!(pattern.Children[1].EngineTypeSet, EngineTiDBOnly);
    assert_eq!(rule.ID(), 0);

    let memo = cascades_memo::Memo::NewMemo(&[]);
    let expression =
        memo.NewGroupExpression(Box::new(logicalop::LogicalJoin::default()), Vec::new());
    let bound = NewBinder(NewPattern(OperandJoin, EngineAll), expression)
        .Next()
        .expect("single join binds");
    assert!(rule.Match(&bound));
    let (alternatives, remove) = rule.XForm(&bound).expect("Go TODO returns no error");
    assert!(alternatives.is_empty());
    assert!(!remove);
}
