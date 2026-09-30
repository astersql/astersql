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

// Builder 单元测试：分配器保留策略与 TableName2ID 大小写处理。
//
// 覆盖 `getKeptAllocators` 在各类 DDL 动作下的过滤规则，以及
// `InitWithDBInfos` 对已加载/未加载表名映射的清理语义。

// 对应 pkg/infoschema/builder_test.go。

use std::sync::Arc;

use astersql_meta_autoid::{Allocator, AllocatorType, Allocators, AutoIdError, Context};

use crate::builder::{
    ActionType, AffectedOption, MetadataReader, NewBuilder, SchemaDiff, getKeptAllocators,
};
use crate::infoschema::{CiString, DBInfo, TableInfo};
use crate::infoschema_v2::NewData;

/// 仅实现 `get_type` 的假 Allocator，其余方法返回固定错误。
struct MockAlloc {
    tp: AllocatorType,
}

impl Allocator for MockAlloc {
    fn alloc(
        &self,
        _ctx: &Context,
        _n: u64,
        _increment: i64,
        _offset: i64,
    ) -> Result<(i64, i64), AutoIdError> {
        Err(AutoIdError::InvalidAutoRandom("mock".into()))
    }
    fn alloc_seq_cache(&self) -> Result<(i64, i64, i64), AutoIdError> {
        Err(AutoIdError::InvalidAutoRandom("mock".into()))
    }
    fn rebase(&self, _ctx: &Context, _new_base: i64, _alloc_ids: bool) -> Result<(), AutoIdError> {
        Err(AutoIdError::InvalidAutoRandom("mock".into()))
    }
    fn force_rebase(&self, _new_base: i64) -> Result<(), AutoIdError> {
        Err(AutoIdError::InvalidAutoRandom("mock".into()))
    }
    fn rebase_seq(&self, _new_base: i64) -> Result<(i64, bool), AutoIdError> {
        Err(AutoIdError::InvalidAutoRandom("mock".into()))
    }
    fn transfer(&self, _database_id: i64, _table_id: i64) -> Result<(), AutoIdError> {
        Err(AutoIdError::InvalidAutoRandom("mock".into()))
    }
    fn base(&self) -> i64 {
        0
    }
    fn end(&self) -> i64 {
        0
    }
    fn next_global_auto_id(&self) -> Result<i64, AutoIdError> {
        Err(AutoIdError::InvalidAutoRandom("mock".into()))
    }
    fn get_type(&self) -> AllocatorType {
        self.tp
    }
}

/// 断言分配器列表长度与各元素类型与期望一致。
fn check_allocators(allocators: &Allocators, expected: &[AllocatorType]) {
    assert_eq!(allocators.len(), expected.len());
    for (i, tp) in expected.iter().enumerate() {
        assert_eq!(*tp, allocators.allocators[i].get_type());
    }
}

#[test]
/// 表驱动验证 Truncate/Rebase/MultiSchemaChange 等场景下保留的分配器类型。
fn test_get_kept_allocators() {
    let allocators = Allocators::new(
        true,
        vec![
            Arc::new(MockAlloc {
                tp: AllocatorType::RowId,
            }),
            Arc::new(MockAlloc {
                tp: AllocatorType::AutoIncrement,
            }),
            Arc::new(MockAlloc {
                tp: AllocatorType::AutoRandom,
            }),
        ],
    );
    // 每项：(SchemaDiff, 期望保留的 AllocatorType 序列)。
    let cases: Vec<(SchemaDiff, Vec<AllocatorType>)> = vec![
        (
            SchemaDiff {
                action_type: ActionType::TruncateTable,
                ..Default::default()
            },
            vec![
                AllocatorType::RowId,
                AllocatorType::AutoIncrement,
                AllocatorType::AutoRandom,
            ],
        ),
        (
            SchemaDiff {
                action_type: ActionType::RebaseAutoID,
                ..Default::default()
            },
            vec![AllocatorType::AutoRandom],
        ),
        (
            SchemaDiff {
                action_type: ActionType::ModifyTableAutoIDCache,
                ..Default::default()
            },
            vec![AllocatorType::AutoRandom],
        ),
        (
            SchemaDiff {
                action_type: ActionType::RebaseAutoRandomBase,
                ..Default::default()
            },
            vec![AllocatorType::RowId, AllocatorType::AutoIncrement],
        ),
        (
            SchemaDiff {
                action_type: ActionType::MultiSchemaChange,
                sub_action_types: vec![ActionType::AddColumn, ActionType::RebaseAutoID],
                ..Default::default()
            },
            vec![AllocatorType::AutoRandom],
        ),
        (
            SchemaDiff {
                action_type: ActionType::MultiSchemaChange,
                sub_action_types: vec![ActionType::ModifyTableAutoIDCache],
                ..Default::default()
            },
            vec![AllocatorType::AutoRandom],
        ),
        (
            SchemaDiff {
                action_type: ActionType::MultiSchemaChange,
                sub_action_types: vec![ActionType::RebaseAutoRandomBase],
                ..Default::default()
            },
            vec![AllocatorType::RowId, AllocatorType::AutoIncrement],
        ),
        (
            SchemaDiff {
                action_type: ActionType::MultiSchemaChange,
                sub_action_types: vec![ActionType::AddColumn],
                ..Default::default()
            },
            vec![
                AllocatorType::RowId,
                AllocatorType::AutoIncrement,
                AllocatorType::AutoRandom,
            ],
        ),
    ];
    for (i, (diff, expected)) in cases.into_iter().enumerate() {
        let res = getKeptAllocators(&diff, &allocators);
        check_allocators(&res, &expected);
        let _ = i;
    }
}

/// 构造仅含 id/db_id/name 的最小 TableInfo。
fn mock_table(id: i64, db_id: i64, name: &str) -> Arc<TableInfo> {
    Arc::new(TableInfo {
        id,
        db_id,
        name: CiString::new(name),
        ..Default::default()
    })
}

/// 用给定库信息跑一遍 `InitWithDBInfos`（v2 Data）。
fn run_init_with_db(db: &mut DBInfo) {
    let data = NewData();
    let mut builder = NewBuilder(0, data, false);
    builder.InitWithDBInfos(std::slice::from_mut(db), Vec::new(), Vec::new(), 1);
}

#[derive(Clone)]
struct MockMetadata {
    db: Option<DBInfo>,
    tables: std::collections::HashMap<(i64, i64), TableInfo>,
}

impl MetadataReader for MockMetadata {
    fn database(&self, id: i64) -> Result<Option<DBInfo>, String> {
        Ok(self.db.clone().filter(|db| db.id == id))
    }

    fn table(&self, schema_id: i64, table_id: i64) -> Result<Option<TableInfo>, String> {
        Ok(self.tables.get(&(schema_id, table_id)).cloned())
    }
}

#[test]
fn test_refresh_meta_drops_table_missing_from_metadata() {
    let table = mock_table(10, 1, "obsolete");
    let mut db = DBInfo {
        id: 1,
        name: CiString::new("testdb"),
        tables: vec![table],
        ..Default::default()
    };
    let data = NewData();
    let mut builder = NewBuilder(0, data, false);
    builder.InitWithDBInfos(std::slice::from_mut(&mut db), Vec::new(), Vec::new(), 1);

    let metadata = MockMetadata {
        db: Some(db),
        tables: Default::default(),
    };
    let affected = builder
        .ApplyDiff(
            &metadata,
            &SchemaDiff {
                version: 2,
                action_type: ActionType::RefreshMeta,
                schema_id: 1,
                table_id: 10,
                ..Default::default()
            },
        )
        .expect("refresh missing table");
    assert_eq!(vec![10], affected);
    assert!(builder.Build(2).TableByID(10).is_none());
}

#[test]
fn test_create_tables_uses_only_affected_options() {
    let table = TableInfo {
        id: 10,
        db_id: 1,
        name: CiString::new("created"),
        ..Default::default()
    };
    let mut db = DBInfo {
        id: 1,
        name: CiString::new("testdb"),
        ..Default::default()
    };
    let metadata = MockMetadata {
        db: Some(db.clone()),
        tables: [((1, 10), table)].into_iter().collect(),
    };
    let data = NewData();
    let mut builder = NewBuilder(0, data, false);
    builder.InitWithDBInfos(std::slice::from_mut(&mut db), Vec::new(), Vec::new(), 1);

    let affected = builder
        .ApplyDiff(
            &metadata,
            &SchemaDiff {
                version: 2,
                action_type: ActionType::CreateTables,
                affected_options: vec![AffectedOption {
                    schema_id: 1,
                    table_id: 10,
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .expect("create tables from affected options");
    assert_eq!(vec![10], affected);
    assert!(builder.Build(2).TableByID(10).is_some());
}

#[test]
/// 已加载表在 Init 后应从 TableName2ID 按原始大小写键移除。
fn test_table_name_2_id_case_sensitive() {
    let mixed = "MyTable";
    let tbl = mock_table(10, 1, mixed);
    let mut db = DBInfo {
        id: 1,
        name: CiString::new("testdb"),
        tables: vec![tbl.clone()],
        table_name_2_id: [(mixed.to_string(), tbl.id)].into_iter().collect(),
    };
    run_init_with_db(&mut db);
    assert!(
        db.table_name_2_id.is_empty(),
        "TableName2ID should be empty after processing, but it still contains: {:?}",
        db.table_name_2_id
    );
}

#[test]
/// 多表、多种大小写命名均应在加载后从映射中清除。
fn test_table_name_2_id_case_sensitive_multiple_tables() {
    let names = ["lowercase", "UPPERCASE", "MixedCase", "CamelCase"];
    let mut tables = Vec::new();
    let mut table_name_2_id = std::collections::HashMap::new();
    for (i, name) in names.iter().enumerate() {
        let tbl = mock_table(100 + i as i64, 1, name);
        table_name_2_id.insert(name.to_string(), tbl.id);
        tables.push(tbl);
    }
    let mut db = DBInfo {
        id: 1,
        name: CiString::new("testdb"),
        tables,
        table_name_2_id,
    };
    run_init_with_db(&mut db);
    assert!(
        db.table_name_2_id.is_empty(),
        "all original-case keys should be removed: {:?}",
        db.table_name_2_id
    );
}

#[test]
/// 已加载键被移除，未加载表的映射应保留以支持惰性加载。
fn test_table_name_2_id_with_unloaded_tables() {
    let loaded = mock_table(10, 1, "LoadedTable");
    let unloaded_name = "UnloadedTable";
    let unloaded_id = 99999_i64;
    let mut db = DBInfo {
        id: 1,
        name: CiString::new("testdb"),
        tables: vec![loaded.clone()],
        table_name_2_id: [
            ("LoadedTable".to_string(), loaded.id),
            (unloaded_name.to_string(), unloaded_id),
        ]
        .into_iter()
        .collect(),
    };
    run_init_with_db(&mut db);
    assert!(
        !db.table_name_2_id.contains_key("LoadedTable"),
        "LoadedTable should be removed from TableName2ID"
    );
    assert_eq!(
        db.table_name_2_id.get(unloaded_name),
        Some(&unloaded_id),
        "UnloadedTable should remain for lazy loading"
    );
}

#[test]
fn crossks_align_meta_loader_builder_updates_column_metadata() {
    let old = TableInfo {
        id: 100,
        db_id: 1,
        name: CiString::new("system_table"),
        ..Default::default()
    };
    let mut db = DBInfo {
        id: 1,
        name: CiString::new("mysql"),
        tables: vec![Arc::new(old.clone())],
        ..Default::default()
    };
    let mut builder = NewBuilder(0, NewData(), false).WithCrossKS(true);
    builder.InitWithDBInfos(std::slice::from_mut(&mut db), vec![], vec![], 1);
    let mut updated = old;
    updated.columns.push(crate::infoschema::ColumnInfo {
        id: 1,
        name: CiString::new("job_id"),
        ..Default::default()
    });
    let metadata = MockMetadata {
        db: Some(db),
        tables: [((1, 100), updated)].into_iter().collect(),
    };
    let affected = builder
        .ApplyDiff(
            &metadata,
            &SchemaDiff {
                version: 2,
                action_type: ActionType::AddColumn,
                schema_id: 1,
                table_id: 100,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(affected, vec![100]);
    let schema = builder.Build(2);
    assert_eq!(schema.SchemaMetaVersion(), 2);
    assert_eq!(schema.TableByID(100).unwrap().Meta().columns.len(), 1);
}

#[test]
fn crossks_align_meta_loader_drop_waits_for_state_none() {
    let model = astersql_meta_model::TableInfo {
        ID: 100,
        DBID: 1,
        Name: astersql_meta_model::ast::NewCIStr("system_table"),
        State: astersql_meta_model::StatePublic,
        ..Default::default()
    };
    let old = crate::infoschema::Table::from_model(model.clone())
        .Meta()
        .clone();
    let mut db = DBInfo {
        id: 1,
        name: CiString::new("mysql"),
        tables: vec![Arc::new(old)],
        ..Default::default()
    };
    let mut builder = NewBuilder(0, NewData(), false).WithCrossKS(true);
    builder.InitWithDBInfos(std::slice::from_mut(&mut db), vec![], vec![], 1);
    let mut dropping = model;
    dropping.State = astersql_meta_model::StateWriteOnly;
    let metadata = MockMetadata {
        db: Some(db.clone()),
        tables: [(
            (1, 100),
            crate::infoschema::Table::from_model(dropping.clone())
                .Meta()
                .clone(),
        )]
        .into_iter()
        .collect(),
    };
    let mut diff = SchemaDiff {
        version: 2,
        action_type: ActionType::DropTable,
        schema_id: 1,
        table_id: 100,
        ..Default::default()
    };
    assert_eq!(builder.ApplyDiff(&metadata, &diff).unwrap(), vec![100]);
    let schema = builder.Build(2);
    assert_eq!(
        schema
            .TableByID(100)
            .unwrap()
            .Meta()
            .model_meta
            .as_ref()
            .unwrap()
            .State,
        astersql_meta_model::StateWriteOnly
    );
    let mut builder = NewBuilder(0, NewData(), false).WithCrossKS(true);
    builder.InitWithOldInfoSchema(schema.as_ref());
    dropping.State = astersql_meta_model::StateNone;
    let metadata = MockMetadata {
        db: Some(db),
        tables: [(
            (1, 100),
            crate::infoschema::Table::from_model(dropping)
                .Meta()
                .clone(),
        )]
        .into_iter()
        .collect(),
    };
    diff.version = 3;
    assert_eq!(builder.ApplyDiff(&metadata, &diff).unwrap(), vec![100]);
    assert!(builder.Build(3).TableByID(100).is_none());
}
