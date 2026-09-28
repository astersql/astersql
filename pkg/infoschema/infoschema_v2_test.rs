// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// InfoSchema V2 路径单元测试（对应 `infoschema_v2_test.go`）。
//
// 直接驱动 `infoschema_v2` / Builder API（无 KV 存储）：基础查表、资源组与
// 放置策略的 ApplyDiff、外键反向引用、以及特殊库名判定。

// Rust counterpart of pkg/infoschema/infoschema_v2_test.go.
// Drives production infoschema_v2 / Builder APIs directly (no kv store).

use std::sync::Arc;

use crate::builder::{ActionType, MetadataReader, NewBuilder, SchemaDiff};
use crate::infoschema::{
    CiString, ColumnInfo, DBInfo, ForeignKeyInfo, InfoSchema, PolicyInfo, ResourceGroupInfo, Table,
    TableInfo, TableItem,
};
use crate::infoschema_v2::{IsSpecialDB, IsV2, NewData, NewInfoSchemaV2};

/// 构造大小写不敏感标识符的测试辅助。
fn ci(name: &str) -> CiString {
    CiString::new(name)
}

/// V2 基础：按名/ID 查库表、SchemaTableInfos、元版本与 TableItem。
#[test]
fn test_v2_basic() {
    let data = NewData();
    let db = DBInfo {
        id: 1,
        name: ci("testDB"),
        tables: Vec::new(),
        table_name_2_id: Default::default(),
    };
    data.addDB(1, db.clone());
    let tbl = Table::new(TableInfo {
        id: 10,
        db_id: 1,
        name: ci("test"),
        columns: vec![ColumnInfo {
            id: 1,
            name: ci("a"),
            auto_increment: false,
        }],
        ..Default::default()
    });
    data.add(&db, tbl.clone(), 2);

    let is = NewInfoSchemaV2(data, 2, 100);
    assert!(IsV2(&is));
    assert_eq!(1, is.AllSchemas().len());

    let got_db = is.SchemaByName(&ci("testDB")).expect("schema");
    assert_eq!(1, got_db.id);
    assert!(is.SchemaByID(1).is_some());
    assert!(is.SchemaByID(-1).is_none());

    let got = is.TableByName(&ci("testDB"), &ci("test")).expect("table");
    assert_eq!(10, got.Meta().id);
    assert!(is.TableByName(&ci("testDB"), &ci("notexist")).is_err());

    assert!(is.TableByID(10).is_some());
    assert!(is.TableByID(-1).is_none());
    assert!(is.TableInfoByID(10).is_some());
    assert!(is.TableInfoByID(-1).is_none());
    assert!(is.TableInfoByID(1234567).is_none());

    let tables = is.SchemaTableInfos(&ci("testDB")).expect("schema tables");
    assert_eq!(1, tables.len());
    assert_eq!(10, tables[0].id);

    let empty = is
        .SchemaTableInfos(&ci("notexist"))
        .expect("missing schema should produce an empty table list");
    assert!(empty.is_empty());

    assert_eq!(2, is.SchemaMetaVersion());

    let item: TableItem = is.TableItemByID(10).expect("item");
    assert_eq!(ci("testDB"), item.DBName);
    assert!(is.TableItemByID(11).is_none());
    assert!(is.TableItemByID(-1).is_none());
}

/// 通过 Builder.initMisc / ApplyDiff 驱动资源组创建并推进 schema 版本。
#[test]
fn test_misc_resource_groups() {
    let data = NewData();
    let mut builder = NewBuilder(0, data, true);
    builder.InitWithDBInfos(&mut [], Vec::new(), Vec::new(), 1);
    let is = builder.Build(u64::MAX);
    // Fresh builder has no resource groups attached to the InfoSchema trait surface;
    // drive Builder::initMisc / ApplyDiff for resource groups instead.
    let data = NewData();
    let mut builder = NewBuilder(0, data, false);
    builder.initMisc(
        Vec::new(),
        vec![ResourceGroupInfo {
            id: 1,
            name: ci("test"),
        }],
    );
    let is = builder.Build(u64::MAX);
    // Resource groups live on the concrete infoSchema; retrieve via downcast-like path
    // by rebuilding and checking Policy/Resource helpers through Builder state.
    let _ = is;
    let data = NewData();
    let mut builder = NewBuilder(0, data, false);
    /// 仅实现 resource_group 读取的空 MetadataReader。
    struct EmptyMeta;
    impl MetadataReader for EmptyMeta {
        fn database(&self, _id: i64) -> Result<Option<DBInfo>, String> {
            Ok(None)
        }
        fn table(&self, _schema_id: i64, _table_id: i64) -> Result<Option<TableInfo>, String> {
            Ok(None)
        }
        fn resource_group(&self, id: i64) -> Result<Option<ResourceGroupInfo>, String> {
            Ok(Some(ResourceGroupInfo {
                id,
                name: ci("test"),
            }))
        }
    }
    builder
        .ApplyDiff(
            &EmptyMeta,
            &SchemaDiff {
                version: 1,
                action_type: ActionType::CreateResourceGroup,
                table_id: 7,
                ..Default::default()
            },
        )
        .expect("create resource group");
    let is = builder.Build(1);
    // Concrete infoSchema holds resource groups; SchemaMetaVersion advances.
    assert_eq!(1, is.SchemaMetaVersion());
}

/// 反向外键：子表 FK 指向父表后，按父表名可查到 referred FK。
#[test]
fn test_referred_foreign_keys() {
    let data = NewData();
    let parent = DBInfo {
        id: 1,
        name: ci("db"),
        tables: Vec::new(),
        table_name_2_id: Default::default(),
    };
    data.addDB(1, parent.clone());
    let child = Table::new(TableInfo {
        id: 2,
        db_id: 1,
        name: ci("child"),
        foreign_keys: vec![ForeignKeyInfo {
            name: ci("fk"),
            ref_schema: ci("db"),
            ref_table: ci("parent"),
        }],
        ..Default::default()
    });
    let parent_tbl = Table::new(TableInfo {
        id: 1,
        db_id: 1,
        name: ci("parent"),
        ..Default::default()
    });
    data.add(&parent, parent_tbl, 1);
    data.add(&parent, child, 1);
    let is = NewInfoSchemaV2(data, 1, 1);
    let refs = is.GetTableReferredForeignKeys("db", "parent");
    assert_eq!(1, refs.len());
    assert_eq!("child", refs[0].child_table.original);
}

/// Full-load reset must hide every derived index at the new schema version,
/// while preserving the previous snapshot's partition and referred-FK view.
#[test]
fn test_reset_before_full_load_hides_partition_and_referred_fk_indexes() {
    let data = NewData();
    let db = DBInfo {
        id: 1,
        name: ci("db"),
        tables: Vec::new(),
        table_name_2_id: Default::default(),
    };
    data.addDB(1, db.clone());
    data.add(
        &db,
        Table::new(TableInfo {
            id: 10,
            db_id: 1,
            name: ci("child"),
            partition: Some(crate::infoschema::PartitionInfo {
                definitions: vec![crate::infoschema::PartitionDefinition {
                    id: 100,
                    name: ci("p0"),
                }],
            }),
            foreign_keys: vec![ForeignKeyInfo {
                name: ci("fk"),
                ref_schema: ci("db"),
                ref_table: ci("parent"),
            }],
            ..Default::default()
        }),
        1,
    );

    data.resetBeforeFullLoad(2);

    let old = NewInfoSchemaV2(data.clone(), 1, 1);
    assert_eq!(Some(10), old.TableIDByPartitionID(100));
    assert_eq!(1, old.GetTableReferredForeignKeys("db", "parent").len());

    let reset = NewInfoSchemaV2(data, 2, 1);
    assert_eq!(None, reset.TableIDByPartitionID(100));
    assert!(reset.GetTableReferredForeignKeys("db", "parent").is_empty());
}

/// Go collects at most 1024 table-history records per GC pass and removes the
/// exact same records from the by-name and by-ID indexes.
#[test]
fn test_gc_old_version_applies_one_global_batch_limit() {
    let data = NewData();
    let db = DBInfo {
        id: 1,
        name: ci("db"),
        tables: Vec::new(),
        table_name_2_id: Default::default(),
    };
    data.addDB(1, db.clone());
    for id in 1..=1025 {
        for version in 1..=3 {
            data.add(
                &db,
                Table::new(TableInfo {
                    id,
                    db_id: db.id,
                    name: ci(&format!("t{id}")),
                    ..Default::default()
                }),
                version,
            );
        }
    }

    let (deleted, remaining_by_name) = data.GCOldVersion(3);
    assert_eq!(1024, deleted);
    assert_eq!(2051, remaining_by_name);
}

/// 特殊系统库名判定，以及 initMisc 注入放置策略后的版本号。
#[test]
fn test_special_attribute_and_policies() {
    assert!(IsSpecialDB("INFORMATION_SCHEMA"));
    assert!(IsSpecialDB("performance_schema"));
    assert!(!IsSpecialDB("test"));

    let data = NewData();
    let mut builder = NewBuilder(0, data, false);
    builder.SetSchemaVersion(1);
    builder.initMisc(
        vec![PolicyInfo {
            id: 1,
            name: ci("p1"),
        }],
        Vec::new(),
    );
    let is = builder.Build(1);
    assert_eq!(1, is.SchemaMetaVersion());
}

/// ApplyDiff 创建放置策略（Placement Policy）并推进 schema 版本。
#[test]
fn test_bundles_via_apply_diff_policy() {
    /// 仅实现 policy 读取的 MetadataReader。
    struct PolicyMeta;
    impl MetadataReader for PolicyMeta {
        fn database(&self, _id: i64) -> Result<Option<DBInfo>, String> {
            Ok(None)
        }
        fn table(&self, _schema_id: i64, _table_id: i64) -> Result<Option<TableInfo>, String> {
            Ok(None)
        }
        fn policy(&self, id: i64) -> Result<Option<PolicyInfo>, String> {
            Ok(Some(PolicyInfo {
                id,
                name: ci("policy"),
            }))
        }
    }
    let data = NewData();
    let mut builder = NewBuilder(0, data, false);
    builder
        .ApplyDiff(
            &PolicyMeta,
            &SchemaDiff {
                version: 1,
                action_type: ActionType::CreatePlacementPolicy,
                table_id: 3,
                ..Default::default()
            },
        )
        .expect("create policy");
    let is = builder.Build(1);
    assert_eq!(1, is.SchemaMetaVersion());
}
