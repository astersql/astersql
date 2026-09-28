// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// RENAME TABLE 相关单元测试（对齐 Go 错误顺序与跨 schema auto ID 语义）。
//
// 覆盖：
// - [`TableCatalog::rename_table_checked`] 在不同 RenameMode 下的错误优先级；
// - 跨 schema 重命名时 `auto_id_schema_id`（自增 ID 归属库）的跟踪与回迁；
// - 批量 rename 的原子性：任一项失败则整批回滚，表 ID 不变。

use std::collections::BTreeMap;

use crate::table::{RenameMode, TableCatalog, TableError, TableInfo, TableState};

/// 构造处于 Public 状态的测试表，并预置 auto_increment / auto_id_cache。
fn table(id: i64, schema_id: i64, name: &str) -> TableInfo {
    TableInfo {
        id,
        schema_id,
        name: name.into(),
        state: TableState::Public,
        partition_ids: Vec::new(),
        auto_increment_id: 5,
        auto_random_id: 0,
        auto_id_cache: 5,
        auto_id_schema_id: 0,
        shard_row_id_bits: 0,
        max_shard_row_id_bits: 0,
        comment: String::new(),
        charset: "utf8mb4".into(),
        collation: "utf8mb4_bin".into(),
        version: 0,
        foreign_keys: Vec::new(),
        tiflash_replica: None,
        placement_policy: None,
        attributes: BTreeMap::new(),
        cached: false,
        affinity: None,
        split_policy: None,
    }
}

/// 校验 rename 错误顺序：目标已存在、源不存在、schema 缺失、名过长、大小写冲突等。
#[test]
fn checked_rename_matches_schema_name_and_statement_error_order() {
    let mut catalog = TableCatalog::default();
    assert!(catalog.create_schema(1));
    assert!(catalog.create_schema(2));
    catalog.insert(table(1, 1, "t")).unwrap();
    catalog.insert(table(2, 2, "occupied")).unwrap();

    // RenameTable：目标名已占用优先于源不存在。
    assert_eq!(
        Err(TableError::AlreadyExists),
        catalog.rename_table_checked(RenameMode::RenameTable, 99, "missing", 2, "occupied")
    );
    assert_eq!(
        Err(TableError::NotFound),
        catalog.rename_table_checked(RenameMode::RenameTable, 99, "missing", 98, "missing")
    );
    assert_eq!(
        Err(TableError::SchemaNotFound),
        catalog.rename_table_checked(RenameMode::RenameTable, 1, "t", 98, "missing")
    );
    // AlterTable 路径：源不存在优先报告 NotFound。
    assert_eq!(
        Err(TableError::NotFound),
        catalog.rename_table_checked(RenameMode::AlterTable, 99, "missing", 2, "occupied")
    );
    assert_eq!(
        Err(TableError::SchemaNotFound),
        catalog.rename_table_checked(RenameMode::RenameTable, 1, "t", 99, "new")
    );
    assert_eq!(
        Err(TableError::NameTooLong),
        catalog.rename_table_checked(RenameMode::RenameTable, 1, "t", 2, &"x".repeat(65))
    );
    // RenameTable 将仅大小写变化视为冲突；AlterTable 允许同 schema 大小写调整。
    assert_eq!(
        Err(TableError::AlreadyExists),
        catalog.rename_table_checked(RenameMode::RenameTable, 1, "t", 1, "T")
    );
    catalog
        .rename_table_checked(RenameMode::AlterTable, 1, "t", 1, "T")
        .unwrap();
}

/// 跨 schema 重命名时记录原始 auto ID 归属库；迁回原 schema 后清零。
#[test]
fn cross_schema_rename_tracks_the_original_auto_id_schema() {
    let mut catalog = TableCatalog::default();
    for schema_id in [1, 2] {
        assert!(catalog.create_schema(schema_id));
    }
    catalog.insert(table(1, 1, "t1")).unwrap();

    // schema1→2：auto_id_schema_id 记为源库 1。
    catalog
        .rename_table_checked(RenameMode::RenameTable, 1, "t1", 2, "t2")
        .unwrap();
    assert_eq!(1, catalog.get(2, "t2").unwrap().auto_id_schema_id);
    catalog
        .rename_table_checked(RenameMode::RenameTable, 2, "t2", 2, "t1")
        .unwrap();
    assert_eq!(1, catalog.get(2, "t1").unwrap().auto_id_schema_id);
    // 迁回原 schema：auto_id_schema_id 清零。
    catalog
        .rename_table_checked(RenameMode::RenameTable, 2, "t1", 1, "t1")
        .unwrap();
    assert_eq!(0, catalog.get(1, "t1").unwrap().auto_id_schema_id);

    // 源 schema 被 drop 后，再跨库 rename 仍保留最初的 auto_id_schema_id。
    catalog
        .rename_table_checked(RenameMode::RenameTable, 1, "t1", 2, "t2")
        .unwrap();
    assert!(catalog.drop_schema(1).unwrap().is_empty());
    assert!(catalog.create_schema(3));
    catalog
        .rename_table_checked(RenameMode::RenameTable, 2, "t2", 3, "t3")
        .unwrap();
    let renamed = catalog.get(3, "t3").unwrap();
    assert_eq!(1, renamed.auto_id_schema_id);
    assert_eq!(5, renamed.auto_increment_id);
    assert_eq!(5, renamed.auto_id_cache);
}

/// 批量 rename 成功时保留表 ID；任一项失败整批不生效。
#[test]
fn checked_batch_rename_is_atomic_and_preserves_ids() {
    let mut catalog = TableCatalog::default();
    assert!(catalog.create_schema(1));
    assert!(catalog.create_schema(2));
    catalog.insert(table(1, 1, "t1")).unwrap();
    catalog.insert(table(2, 1, "t2")).unwrap();

    catalog
        .rename_tables_checked(&[
            (1, "t1".into(), 2, "t1".into()),
            (1, "t2".into(), 2, "t2".into()),
        ])
        .unwrap();
    assert_eq!(1, catalog.get(2, "t1").unwrap().id);
    assert_eq!(2, catalog.get(2, "t2").unwrap().id);
    assert_eq!(1, catalog.get(2, "t1").unwrap().auto_id_schema_id);

    // 第二项源表不存在：整批失败，已有表位置不变。
    let error = catalog.rename_tables_checked(&[
        (2, "t1".into(), 1, "new1".into()),
        (2, "missing".into(), 1, "new2".into()),
    ]);
    assert_eq!(Err(TableError::NotFound), error);
    assert_eq!(1, catalog.get(2, "t1").unwrap().id);
    assert_eq!(2, catalog.get(2, "t2").unwrap().id);

    assert_eq!(
        Err(TableError::NotFound),
        catalog.rename_tables_checked(&[
            (99, "missing".into(), 98, "missing".into()),
            (99, "missing2".into(), 98, "missing2".into()),
        ])
    );
}
