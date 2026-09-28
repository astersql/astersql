// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

// DataSource 与相关逻辑算子的单元测试。
//
// 覆盖主键/唯一键构建、列裁剪（PruneColumns）、索引后缀有序性枚举、
// SHOW 列查找假统计，以及 CTE 共享种子统计（SeedStat）推导。

use crate::*;
use std::sync::Arc;

/// 构造 BIGINT 类型的规划器测试列。
fn planner_column(id: i64, unique_id: i64) -> Column {
    Column::new(
        *expression::types::NewFieldType(mysql::r#type::TypeLonglong),
        id,
        unique_id,
        0,
    )
}

#[test]
/// 验证 BuildKeyInfo 将句柄主键与唯一索引同时记入 Schema.PKOrUK。
fn data_source_builds_primary_and_unique_keys() {
    let mut primary = model::ColumnInfo::New(1, parser_ast::NewCIStr("id"));
    primary.AddFlag(mysql::r#type::PriKeyFlag | mysql::r#type::NotNullFlag);
    let mut unique = model::ColumnInfo::New(2, parser_ast::NewCIStr("email"));
    unique.AddFlag(mysql::r#type::NotNullFlag);
    unique.Offset = 1;
    let index = model::IndexInfo {
        Unique: true,
        State: model::StatePublic,
        Columns: vec![model::IndexColumn {
            Name: parser_ast::NewCIStr("email"),
            Offset: 1,
            Length: expression::types::UnspecifiedLength as isize,
            UseChangingType: false,
        }],
        ..Default::default()
    };
    let mut source = DataSource::default();
    source.TableInfo.PKIsHandle = true;
    source.TableInfo.Indices.push(index);
    source.Columns = vec![primary, unique];
    source
        .LogicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![
            planner_column(1, 101),
            planner_column(2, 102),
        ]));

    source.BuildKeyInfo();

    assert_eq!(source.Schema().PKOrUK.len(), 2);
    assert!(source.Schema().PKOrUK.iter().any(|key| key[0].ID == 1));
    assert!(source.Schema().PKOrUK.iter().any(|key| key[0].ID == 2));
}

#[test]
/// 验证父侧无用列时 PruneColumns 仍保留至少一列物理列。
fn data_source_pruning_keeps_one_physical_column() {
    let mut source = DataSource::default();
    source.Columns = vec![
        model::ColumnInfo::New(1, parser_ast::NewCIStr("a")),
        model::ColumnInfo::New(2, parser_ast::NewCIStr("b")),
    ];
    source
        .LogicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![
            planner_column(1, 101),
            planner_column(2, 102),
        ]));

    source.PruneColumns(&[]).unwrap();

    assert_eq!(source.Schema().Len(), 1);
    assert_eq!(source.Columns.len(), 1);
}

#[test]
/// 验证 PreparePossibleProperties 按等式前缀枚举索引后缀有序性。
fn data_source_enumerates_index_suffix_orders() {
    let first = planner_column(1, 101);
    let second = planner_column(2, 102);
    let mut source = DataSource::default();
    source
        .AllPossibleAccessPaths
        .push(planner_util::AccessPath {
            IdxCols: vec![first.clone(), second.clone()],
            EqCondCount: 1,
            ..Default::default()
        });

    let properties = source.PreparePossibleProperties();

    assert_eq!(properties.Orders.len(), 2);
    assert_eq!(properties.Orders[0][0].UniqueID, first.UniqueID);
    assert_eq!(properties.Orders[1][0].UniqueID, second.UniqueID);
}

#[test]
/// 验证 findShowColumnIDs 与 getFakeStats 按 Schema/列名工作。
fn show_statistics_and_column_lookup_follow_schema() {
    let schema = expression::NewSchema(vec![planner_column(1, 101)]);
    let names = NameSlice(vec![Some(Arc::new(FieldName {
        ColName: parser_ast::NewCIStr("db_name"),
        ..Default::default()
    }))]);

    let ids = findShowColumnIDs(&schema, &names, "db_name");
    let stats = getFakeStats(&schema);

    assert_eq!(ids, [101].into_iter().collect());
    assert_eq!(stats.RowCount, 1.0);
    assert_eq!(stats.ColNDVs[&101], 1.0);
}

#[test]
/// 验证 LogicalCTE::DeriveStats 复用共享 SeedStat 并标记已重载。
fn cte_uses_shared_seed_statistics() {
    let mut seed = StatsInfo {
        RowCount: 42.0,
        ..Default::default()
    };
    seed.ColNDVs.insert(101, 7.0);
    let mut cte = LogicalCTE::default();
    *cte.SeedStat.write().unwrap() = seed;

    let (stats, reloaded) = cte.DeriveStats(true).unwrap();

    assert!(reloaded);
    assert_eq!(stats.RowCount, 42.0);
    assert_eq!(stats.ColNDVs[&101], 7.0);
}
