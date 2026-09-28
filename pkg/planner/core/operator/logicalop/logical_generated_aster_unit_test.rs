// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

// 代码生成逻辑算子方法的单元测试。
//
// 验证 Hash64/Equals、ShallowRef（浅引用/写时复制容器）等生成代码
// 在 DataSource、LogicalJoin、LogicalProjection 上的行为。

use crate::*;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;

/// 构造 UniqueID = id+100 的测试列。
fn column(id: i64) -> Column {
    Column::new(
        *expression::types::NewFieldType(mysql::r#type::TypeLonglong),
        id,
        id + 100,
        0,
    )
}

#[test]
/// 验证 DataSource 哈希与相等性覆盖表 ID 与条件。
fn generated_datasource_hash_and_equals_cover_table_and_conditions() {
    let mut left = DataSource::default();
    left.TableInfo.ID = 7;
    left.PhysicalTableID = 11;
    left.AllConds.push(Box::new(column(1)));
    let mut right = DataSource::default();
    right.TableInfo.ID = 7;
    right.PhysicalTableID = 11;
    right.AllConds.push(Box::new(column(1)));
    let mut left_hash = DefaultHasher::new();
    let mut right_hash = DefaultHasher::new();

    left.Hash64(&mut left_hash);
    right.Hash64(&mut right_hash);

    assert!(left.Equals(&right));
    assert_eq!(left_hash.finish(), right_hash.finish());
    right.PhysicalTableID = 12;
    assert!(!left.Equals(&right));
}

#[test]
/// 验证 Join 浅引用对 LeftConditions 独立可变，不污染原对象。
fn generated_join_shallow_ref_has_independent_condition_container() {
    let mut original = LogicalJoin::default();
    original.LeftConditions.push(Box::new(column(1)));

    let mut copied = original.LogicalJoinShallowRef();
    copied.LeftConditionsShallowRef().push(Box::new(column(2)));

    assert_eq!(original.LeftConditions.len(), 1);
    assert_eq!(copied.LeftConditions.len(), 2);
}

#[test]
/// 验证 Projection 相等性随表达式列表变化而失效。
fn generated_projection_equality_tracks_expression_identity() {
    let mut left = LogicalProjection::default();
    left.Exprs.push(Box::new(column(1)));
    let mut right = left.LogicalProjectionShallowRef();

    assert!(left.Equals(&right));
    right.ExprsShallowRef().push(Box::new(column(2)));
    assert!(!left.Equals(&right));
}
