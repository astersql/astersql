// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

// 逻辑算子 Hash64 / Equals 一致性单测。
//
// 验证各逻辑计划节点在默认状态下哈希与相等一致，字段变更后既不相等也不再共享
// 同一哈希。用于保证计划指纹（Plan Fingerprint）与等价判断实现正确。

use logicalop::*;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;

/// 构造带给定 UniqueID 的测试用整型列。
fn column(id: i64) -> Column {
    Column::new(
        *expression::types::NewFieldType(mysql::r#type::TypeLonglong),
        id,
        id + 100,
        0,
    )
}

/// 通用校验：默认值相等且哈希相同；`mutate` 后应不相等。
fn verify_generated<T: Default>(
    hash: impl Fn(&T, &mut dyn Hasher),
    equals: impl Fn(&T, &T) -> bool,
    mutate: impl Fn(&mut T),
) {
    let left = T::default();
    let mut right = T::default();
    let mut left_hash = DefaultHasher::new();
    let mut right_hash = DefaultHasher::new();
    hash(&left, &mut left_hash);
    hash(&right, &mut right_hash);
    assert!(equals(&left, &right));
    assert_eq!(left_hash.finish(), right_hash.finish());
    mutate(&mut right);
    assert!(!equals(&left, &right));
    let mut mutated_hash = DefaultHasher::new();
    hash(&right, &mut mutated_hash);
    assert_ne!(left_hash.finish(), mutated_hash.finish());
}

fn hash<T>(value: &T, write: impl Fn(&T, &mut dyn Hasher)) -> u64 {
    let mut hasher = DefaultHasher::new();
    write(value, &mut hasher);
    hasher.finish()
}

fn top_n() -> LogicalTopN {
    LogicalTopN {
        ByItems: vec![ByItems {
            Expr: Box::new(column(1)),
            Desc: true,
        }],
        PartitionBy: vec![SortItem {
            Col: column(1),
            Desc: true,
        }],
        Offset: 1,
        Count: 1,
        ..Default::default()
    }
}

fn assert_top_n_mutation(mutate: fn(&mut LogicalTopN)) {
    let left = top_n();
    let mut right = top_n();
    mutate(&mut right);
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
}

fn mutate_top_n_expr(p: &mut LogicalTopN) {
    p.ByItems[0].Expr = Box::new(column(2));
}
fn mutate_top_n_direction(p: &mut LogicalTopN) {
    p.ByItems[0].Desc = false;
}
fn mutate_top_n_partition_col(p: &mut LogicalTopN) {
    p.PartitionBy[0].Col = column(2);
}
fn mutate_top_n_partition_direction(p: &mut LogicalTopN) {
    p.PartitionBy[0].Desc = false;
}
fn mutate_top_n_offset(p: &mut LogicalTopN) {
    p.Offset = 2;
}
fn mutate_top_n_count(p: &mut LogicalTopN) {
    p.Count = 2;
}
fn mutate_top_n_prefer_limit_to_cop(p: &mut LogicalTopN) {
    p.PreferLimitToCop = true;
}

/// TopN：修改 Count 后应不相等。
#[test]
fn TestLogicalTopNHash64Equals() {
    let left = top_n();
    let mut right = top_n();
    assert!(left.Equals(&right));
    assert_eq!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );

    assert_top_n_mutation(mutate_top_n_expr);
    assert_top_n_mutation(mutate_top_n_direction);
    assert_top_n_mutation(mutate_top_n_partition_col);
    assert_top_n_mutation(mutate_top_n_partition_direction);
    assert_top_n_mutation(mutate_top_n_offset);
    assert_top_n_mutation(mutate_top_n_count);
    assert_top_n_mutation(mutate_top_n_prefer_limit_to_cop);
}
/// TableDual：修改 RowCount 后应不相等。
#[test]
fn TestLogicalTableDualHash64Equals() {
    let mut left = LogicalTableDual::default();
    left.SetSchema(expression::NewSchema(vec![column(1)]));
    let mut right = LogicalTableDual::default();
    right.SetSchema(expression::NewSchema(vec![column(1)]));
    assert!(left.Equals(&right));
    assert_eq!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );

    right.SetSchema(expression::NewSchema(vec![column(2)]));
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );

    right.SetSchema(expression::NewSchema(vec![column(1)]));
    right.RowCount = 1;
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
}
/// Sort：追加 ByItems 后应不相等。
#[test]
fn TestLogicalSortHash64Equals() {
    let mut left = LogicalSort {
        BaseLogicalPlan: BaseLogicalPlan::default(),
        ByItems: Vec::new(),
    };
    let mut right = LogicalSort {
        BaseLogicalPlan: BaseLogicalPlan::default(),
        ByItems: Vec::new(),
    };
    assert!(left.Equals(&right));
    assert_eq!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
    right.ByItems.push(ByItems {
        Expr: Box::new(column(1)),
        Desc: true,
    });
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
    left.ByItems.push(ByItems {
        Expr: Box::new(column(1)),
        Desc: false,
    });
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
    right.ByItems[0].Desc = false;
    assert!(left.Equals(&right));
    assert_eq!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
}
/// ShowDDLJobs：修改 Schema 后应不相等。
#[test]
fn TestLogicalShowDDLJobs() {
    let left = LogicalShowDDLJobs::default();
    let mut right = LogicalShowDDLJobs::default();
    assert!(left.Equals(&right));
    assert_eq!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
    right.SetSchema(expression::NewSchema(vec![column(1)]));
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
}
/// Show：修改 Schema 后应不相等。
#[test]
fn TestLogicalShowHash64Equals() {
    let left = LogicalShow::default();
    let mut right = LogicalShow::default();
    assert!(left.Equals(&right));
    assert_eq!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
    right.SetSchema(expression::NewSchema(vec![column(1)]));
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
}
/// Selection：追加条件后应不相等。
#[test]
fn TestLogicalSelectionHash64Equals() {
    let mut left = LogicalSelection {
        Conditions: vec![Box::new(column(1))],
        ..Default::default()
    };
    let mut right = LogicalSelection {
        Conditions: vec![Box::new(column(1))],
        ..Default::default()
    };
    assert!(left.Equals(&right));
    assert_eq!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
    right.Conditions.clear();
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
    right.Conditions = vec![Box::new(column(2))];
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
}
/// Projection：追加投影表达式后应不相等。
#[test]
fn TestLogicalProjectionHash64Equals() {
    let mut left = LogicalProjection {
        Exprs: vec![Box::new(column(2))],
        ..Default::default()
    };
    left.SetSchema(expression::NewSchema(vec![column(1)]));
    let mut right = LogicalProjection {
        Exprs: vec![Box::new(column(2))],
        ..Default::default()
    };
    right.SetSchema(expression::NewSchema(vec![column(1)]));
    assert!(left.Equals(&right));
    assert_eq!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
    right.Exprs.clear();
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
    right.Exprs = vec![Box::new(column(2))];
    right.CalculateNoDelay = true;
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
    right.CalculateNoDelay = false;
    right.Proj4Expand = true;
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
}
/// MemTable：修改 TableInfo.ID 后应不相等。
#[test]
fn TestLogicalMemTableHash64Equals() {
    let mut left = LogicalMemTable::default();
    left.SetSchema(expression::NewSchema(vec![column(1)]));
    let mut right = LogicalMemTable::default();
    right.SetSchema(expression::NewSchema(vec![column(1)]));
    assert!(left.Equals(&right));
    assert_eq!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
    right.SetSchema(expression::NewSchema(vec![column(2)]));
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
    right.SetSchema(expression::NewSchema(vec![column(1)]));
    right.DBName = parser_ast::NewCIStr("test");
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
    right.DBName = parser_ast::NewCIStr("");
    right.TableInfo.ID = 1;
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
}
/// Limit：修改 Count 后应不相等。
#[test]
fn TestLogicalLimitHash64Equals() {
    let mut left = LogicalLimit {
        PartitionBy: vec![SortItem {
            Col: column(1),
            Desc: true,
        }],
        Offset: 1,
        Count: 1,
        ..Default::default()
    };
    let mut right = LogicalLimit {
        PartitionBy: vec![SortItem {
            Col: column(1),
            Desc: true,
        }],
        Offset: 1,
        Count: 1,
        ..Default::default()
    };
    assert!(left.Equals(&right));
    assert_eq!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
    right.PartitionBy[0].Col = column(2);
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
    right.PartitionBy[0].Col = column(1);
    right.Offset = 2;
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
    right.Offset = 1;
    right.Count = 2;
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
}
/// Expand：修改 DistinctSize 后应不相等。
#[test]
fn TestLogicalExpandHash64Equals() {
    for mutate in [
        (|p: &mut LogicalExpand| p.DistinctGroupByCol.push(column(1))) as fn(&mut LogicalExpand),
        |p| p.DistinctGbyExprs.push(Box::new(column(1))),
        |p| p.DistinctSize = 1,
        |p| {
            p.RollupGroupingSets.0.push(GroupingSet {
                ColumnIDs: std::collections::BTreeSet::from([1]),
            })
        },
        |p| p.LevelExprs.push(vec![Box::new(column(1))]),
        |p| p.GID = Some(column(1)),
        |p| p.GPos = Some(column(1)),
    ] {
        verify_generated(
            |p: &LogicalExpand, h| p.Hash64(h),
            LogicalExpand::Equals,
            mutate,
        );
    }
}
/// Apply：打开 NoDecorrelate 后应不相等。
#[test]
fn TestLogicalApplyHash64Equals() {
    verify_generated(
        |p: &LogicalApply, h| p.Hash64(h),
        LogicalApply::Equals,
        |p| {
            p.CorCols.push(CorrelatedColumn {
                column: column(1),
                data: None,
            });
        },
    );
    verify_generated(
        |p: &LogicalApply, h| p.Hash64(h),
        LogicalApply::Equals,
        |p| {
            p.NoDecorrelate = true;
        },
    );
}
/// Join：追加左条件后应不相等。
#[test]
fn TestLogicalJoinHash64Equals() {
    for mutate in [
        (|p: &mut LogicalJoin| p.EqualConditions.push(Box::new(column(1)))) as fn(&mut LogicalJoin),
        |p| p.LeftConditions.push(Box::new(column(1))),
        |p| p.OtherConditions.push(Box::new(column(1))),
    ] {
        verify_generated(
            |p: &LogicalJoin, h| p.Hash64(h),
            LogicalJoin::Equals,
            mutate,
        );
    }
}
/// Aggregation：追加 COUNT 聚合函数后应不相等。
#[test]
fn TestLogicalAggregationHash64Equals() {
    verify_generated(
        |p: &LogicalAggregation, h| p.Hash64(h),
        LogicalAggregation::Equals,
        |p| {
            let ctx = exprstatic::NewExprContext(Vec::new());
            p.AggFuncs.push(
                aggregation::NewAggFuncDesc(
                    &ctx,
                    aggregation::ast::AggFuncCount,
                    vec![Box::new(column(1))],
                    false,
                )
                .expect("build count descriptor"),
            );
        },
    );
    verify_generated(
        |p: &LogicalAggregation, h| p.Hash64(h),
        LogicalAggregation::Equals,
        |p| p.GroupByItems.push(Box::new(column(1))),
    );
    verify_generated(
        |p: &LogicalAggregation, h| p.Hash64(h),
        LogicalAggregation::Equals,
        |p| p.PossibleProperties.push(vec![column(1)]),
    );
}

/// UnionAll：修改 schema 后应不相等。
#[test]
fn TestLogicalUnionAllHash64Equals() {
    let left = LogicalUnionAll::default();
    let mut right = LogicalUnionAll::default();
    assert!(left.Equals(&right));
    assert_eq!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
    right
        .LogicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![column(1)]));
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );

    let left = LogicalPartitionUnionAll::default();
    let mut right = LogicalPartitionUnionAll::default();
    assert!(left.Equals(&right));
    assert_eq!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
    right.SetSchema(expression::NewSchema(vec![column(1)]));
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
}

/// LogicalSchemaProducer：修改 schema 后应不相等。
#[test]
fn TestLogicalSchemaProducerHash64Equals() {
    let left = LogicalSchemaProducer::default();
    let mut right = LogicalSchemaProducer::default();
    assert!(left.Equals(&right));
    assert_eq!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
    right.SetSchema(expression::NewSchema(vec![column(1)]));
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
}

/// MaxOneRow：修改 schema 后应不相等。
#[test]
fn TestLogicalMaxOneRowHash64Equals() {
    let left = LogicalMaxOneRow {
        BaseLogicalPlan: BaseLogicalPlan::default(),
    };
    let mut right = LogicalMaxOneRow {
        BaseLogicalPlan: BaseLogicalPlan::default(),
    };
    assert!(left.Equals(&right));
    assert_eq!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
    right.SetSchema(expression::NewSchema(vec![column(1)]));
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
}

/// Sequence：修改 schema 后应不相等。
#[test]
fn TestLogicalSequence() {
    let left = LogicalSequence::default();
    let mut right = LogicalSequence::default();
    assert!(left.Equals(&right));
    assert_eq!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
    right.SetSchema(expression::NewSchema(vec![column(1)]));
    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&left, |p, h| p.Hash64(h)),
        hash(&right, |p, h| p.Hash64(h))
    );
}

/// FrameBound：修改 Num 后 Equals 与 Hash64 均应变化。
#[test]
fn TestFrameBoundHash64Equals() {
    let left = FrameBound::default();
    let mut right = FrameBound::default();
    assert!(left.Equals(&right));
    assert_eq!(left.Hash64(), right.Hash64());
    right.Num = 1;
    assert!(!left.Equals(&right));
    assert_ne!(left.Hash64(), right.Hash64());
}

/// Clone 后 CompareCols 非空切片仍保持 Hash/Equals 一致。
#[test]
fn TestFrameBoundClonePreservesNilSlicesForHashEquals() {
    let left = FrameBound {
        CompareCols: vec![Box::new(column(1))],
        ..Default::default()
    };
    let right = left.Clone();
    assert!(left.Equals(&right));
    assert_eq!(left.Hash64(), right.Hash64());
}

/// WindowFrame：设置 End 边界后应不相等。
#[test]
fn TestWindowFrameHash64Equals() {
    let left = WindowFrame::default();
    let mut right = WindowFrame::default();
    assert!(left.Equals(&right));
    assert_eq!(left.Hash64(), right.Hash64());
    right.End = Some(FrameBound {
        Num: 1,
        ..Default::default()
    });
    assert!(!left.Equals(&right));
}

/// HandleCols：Clone 后列数与 UniqueID 序列一致。
#[test]
fn TestHandleColsHash64Equals() {
    let left = planner_util::NewIntHandleCols(column(1));
    let right = left.CloneHandleCols();
    assert_eq!(left.NumCols(), right.NumCols());
    assert!(
        left.IterColumns()
            .zip(right.IterColumns())
            .all(|(a, b)| a.UniqueID == b.UniqueID)
    );
}

/// 占位 mock，供其它测试引用。
#[allow(dead_code)]
fn MockFunc() {}
/// 占位 mock，供其它测试引用。
#[allow(dead_code)]
fn MockFunc2() {}
