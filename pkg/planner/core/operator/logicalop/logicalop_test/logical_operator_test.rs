// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

// 逻辑算子行为与拷贝语义单元测试。
// 覆盖 Schema/Apply/FrameBound 克隆、表达式列替换（写时复制）以及 TopN 下推与列裁剪。

use expression::Expression as _;
use logicalop::*;
use std::collections::HashMap;

/// 构造指定 ID 的整型列。
fn column(id: i64) -> Column {
    Column::new(
        *expression::types::NewFieldType(mysql::r#type::TypeLonglong),
        id,
        id + 100,
        0,
    )
}

/// Schema 深拷贝后追加列不影响原 Schema。
#[test]
fn TestLogicalSchemaClone() {
    let schema = expression::NewSchema(vec![column(1)]);
    let mut cloned = schema.Clone();
    cloned.Append([column(2)]);
    assert_eq!(schema.Len(), 1);
    assert_eq!(cloned.Len(), 2);
}

/// LogicalApply 内嵌 Join 的浅拷贝与原对象相等。
#[test]
fn TestLogicalApplyClone() {
    let mut original = LogicalApply::default();
    original
        .LogicalJoin
        .LeftConditions
        .push(Box::new(column(1)));
    let cloned_join = original.LogicalJoin.LogicalJoinShallowRef();
    assert!(original.LogicalJoin.Equals(&cloned_join));
}

/// FrameBound（窗口帧边界）克隆对 CompareCols 做深拷贝。
#[test]
fn TestFrameBoundCloneDeepCopiesCompareCols() {
    let calc = column(1);
    let compare = column(2);
    let original = FrameBound {
        CalcFuncs: vec![Box::new(calc.clone())],
        CompareCols: vec![Box::new(compare.clone())],
        ..Default::default()
    };
    let mut cloned = original.Clone();

    assert_eq!(cloned.CompareCols.len(), 1);
    assert_eq!(
        cloned.CompareCols[0].as_column().unwrap().UniqueID,
        compare.UniqueID
    );
    assert_ne!(
        cloned.CompareCols[0].as_ref() as *const dyn expression::Expression,
        original.CompareCols[0].as_ref() as *const dyn expression::Expression
    );

    cloned.CompareCols[0] = Box::new(column(3));
    assert_eq!(original.CompareCols.len(), 1);
    assert_eq!(
        original.CompareCols[0].as_column().unwrap().UniqueID,
        compare.UniqueID
    );
    assert_eq!(
        original.CalcFuncs[0].as_column().unwrap().UniqueID,
        calc.UniqueID
    );
}

/// ReplaceExprColumns 采用写时复制：替换后表达式哈希变化。
#[test]
fn TestReplaceColumnOfExprCopyOnWrite() {
    let source = column(1);
    let replacement = column(2);
    let mut projection = LogicalProjection {
        Exprs: vec![Box::new(source.clone())],
        ..Default::default()
    };
    let original_hash = projection.Exprs[0].CanonicalHashCode();
    projection.ReplaceExprColumns(&HashMap::from([(source.CanonicalHashCode(), replacement)]));
    assert_ne!(projection.Exprs[0].CanonicalHashCode(), original_hash);
}

/// 列替换只改投影内表达式，不改动外部 source 列对象。
#[test]
fn TestResolveExprAndReplaceCopyOnWrite() {
    let source = column(1);
    let mut projection = LogicalProjection {
        Exprs: vec![Box::new(source.clone())],
        ..Default::default()
    };
    projection.ReplaceExprColumns(&HashMap::from([(source.CanonicalHashCode(), column(2))]));
    assert_eq!(projection.Exprs[0].as_column().unwrap().ID, 2);
    assert_eq!(source.ID, 1);
}

/// LogicalCTE 无子节点时 PreparePossibleProperties 返回空序且无 TiFlash。
#[test]
fn TestLogicalCTEPreparePossiblePropertiesSkipNilChild() {
    let cte = LogicalCTE::default();
    let properties = cte.PreparePossibleProperties();
    assert!(properties.Orders.is_empty());
    assert!(!properties.HasTiFlash);
}

/// Projection 下推 TopN：排序列映射回投影输入列 UniqueID。
#[test]
fn TestLogicalProjectionPushDownTopN() {
    let output = column(10);
    let input = column(1);
    let mut projection = LogicalProjection {
        Exprs: vec![Box::new(input.clone())],
        ..Default::default()
    };
    projection.SetSchema(expression::NewSchema(vec![output.clone()]));
    projection.SetChildren(vec![Box::new(LogicalTableDual::default())]);
    let top_n = LogicalTopN {
        ByItems: vec![ByItems {
            Expr: Box::new(output),
            Desc: true,
        }],
        Count: 1,
        ..Default::default()
    };
    let pushed = projection
        .PushDownTopN(Some(top_n))
        .expect("TopN should remain in the plan");
    let pushed = pushed.as_any().downcast_ref::<LogicalTopN>().unwrap();
    assert_eq!(
        pushed.ByItems[0].Expr.as_column().unwrap().UniqueID,
        input.UniqueID
    );
}

/// TopN 列裁剪先刷新 Schema，再内联投影，最终只保留所需列。
#[test]
fn TestLogicalTopNPruneColumnsRefreshesSchemaBeforeInlineProjection() {
    let first = column(1);
    let second = column(2);
    let sort_column = column(3);
    let mut child = LogicalTableDual::default();
    child.SetSchema(expression::NewSchema(vec![
        first.clone(),
        second.clone(),
        sort_column.clone(),
    ]));
    let mut top_n = LogicalTopN {
        ByItems: vec![ByItems {
            Expr: Box::new(sort_column.clone()),
            Desc: false,
        }],
        ..Default::default()
    };
    top_n.SetSchema(expression::NewSchema(vec![
        first.clone(),
        second.clone(),
        sort_column.clone(),
        sort_column.clone(),
    ]));
    top_n.SetChildren(vec![Box::new(child)]);
    top_n
        .PruneColumns(&[first.clone(), second.clone(), sort_column.clone()])
        .unwrap();
    assert_eq!(top_n.Schema().Len(), 3);
    assert_eq!(top_n.Schema().Columns[0].UniqueID, first.UniqueID);
    assert_eq!(top_n.Schema().Columns[1].UniqueID, second.UniqueID);
    assert_eq!(top_n.Schema().Columns[2].UniqueID, sort_column.UniqueID);
}

/// LogicalExpand 重建键信息后清空 PKOrUK（展开会破坏唯一性）。
#[test]
fn TestLogicalExpandBuildKeyInfo() {
    let key = column(1);
    let mut child = LogicalTableDual::default();
    let mut schema = expression::NewSchema(vec![key.clone()]);
    schema.PKOrUK.push(vec![key.clone()]);
    child.SetSchema(schema.Clone());
    let mut expand = LogicalExpand::default();
    expand.SetSchema(schema);
    expand.SetChildren(vec![Box::new(child)]);
    expand.BuildKeyInfo();
    assert!(expand.Schema().PKOrUK.is_empty());
}
