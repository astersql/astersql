// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// InfoSchema / Builder 核心路径单元测试（对应 `infoschema_test.go`）。
//
// 用等价元数据形状驱动生产 Builder、MockInfoSchema、会话临时表与 V2 开关，
// 不依赖完整 mockstore / testkit。

// Rust counterpart of pkg/infoschema/infoschema_test.go.
// Drives production Builder / InfoSchema APIs with equivalent metadata shapes.

use std::sync::Arc;

use crate::builder::{ActionType, MetadataReader, NewBuilder, SchemaDiff};
use crate::infoschema::{
    AllSchemaNames, CiString, ColumnInfo, DBInfo, HasAutoIncrementColumn, IndexInfo, InfoSchema,
    InfoSchemaError, MaskingPolicyInfo, MaskingPolicyLoader, MaskingPolicyRestrictOps,
    MaskingPolicyType, MockInfoSchema, NewSessionTables, SchemaByTable, Table, TableInfo,
    TableIsSequence, TableIsView, buildLoadMaskingPoliciesQuery, infoSchema,
    loadMaskingPoliciesWithTableIDs, maskingPolicyRestrictOpsFromString,
    maskingPolicyStatusFromString, maskingPolicyTypeFromString,
};
use crate::infoschema_v2::{IsV2, NewData, NewInfoSchemaV2};

/// 构造大小写不敏感标识符（CIStr）的测试辅助。
fn ci(name: &str) -> CiString {
    CiString::new(name)
}

/// 基础查库/查表：按名、按 ID、SchemaByTable、视图/序列判定与 TableItem。
#[test]
fn test_basic() {
    let db_name = ci("Test");
    let tb_name = ci("T");
    let tbl = TableInfo {
        id: 100,
        db_id: 10,
        name: tb_name.clone(),
        columns: vec![ColumnInfo {
            id: 1,
            name: ci("A"),
            auto_increment: false,
        }],
        indices: vec![IndexInfo {
            id: 1,
            name: ci("idx"),
        }],
        ..Default::default()
    };
    let mut db = DBInfo {
        id: 10,
        name: db_name.clone(),
        tables: vec![Arc::new(tbl.clone())],
        table_name_2_id: Default::default(),
    };
    let data = NewData();
    let mut builder = NewBuilder(0, data, false);
    builder.InitWithDBInfos(std::slice::from_mut(&mut db), Vec::new(), Vec::new(), 1);
    let is = builder.Build(u64::MAX);
    let noexist = ci("noexist");

    let schema_names = AllSchemaNames(is.as_ref());
    assert!(schema_names.iter().any(|n| n == "Test"));
    assert!(is.SchemaByName(&db_name).is_some());
    assert!(is.SchemaByName(&noexist).is_none());
    assert!(is.SchemaByID(10).is_some());
    assert!(is.SchemaByID(100).is_none());
    assert!(is.SchemaByID(-1).is_none());

    assert!(SchemaByTable(is.as_ref(), &tbl).is_some());
    let missing = TableInfo {
        id: 12345,
        name: tbl.name.clone(),
        ..Default::default()
    };
    assert!(SchemaByTable(is.as_ref(), &missing).is_none());

    assert!(is.TableByName(&db_name, &tb_name).is_ok());
    assert!(is.TableByName(&db_name, &noexist).is_err());

    // Go SchemaTableInfos returns an empty slice for a missing schema.
    let missing_tables = is
        .SchemaTableInfos(&noexist)
        .expect("missing schema should produce an empty table list");
    assert!(missing_tables.is_empty());
    assert!(!TableIsView(is.as_ref(), &db_name, &tb_name));
    assert!(!TableIsSequence(is.as_ref(), &db_name, &tb_name));

    let tb = is.TableByID(100).expect("table by id");
    assert_eq!(tb.Meta().id, 100);
    assert!(is.TableByID(10).is_none());
    assert!(is.TableByID(-12345).is_none());
    assert!(is.TableByID(-1).is_none());

    let item = is.TableItemByID(100).expect("table item");
    assert_eq!(item.DBName, db_name);
    assert!(is.TableItemByID(101).is_none());
    assert!(is.TableItemByID(-1).is_none());
}

/// MockInfoSchema 构造与自增列探测。
#[test]
fn test_mock_info_schema() {
    let tbl = TableInfo {
        id: 1,
        name: ci("t"),
        columns: vec![ColumnInfo {
            id: 1,
            name: ci("a"),
            auto_increment: true,
        }],
        ..Default::default()
    };
    let is = MockInfoSchema(vec![tbl]);
    assert_eq!(0, is.SchemaMetaVersion());
    let table = is.TableByName(&ci("test"), &ci("t")).expect("mock table");
    assert_eq!(Some("a".to_string()), HasAutoIncrementColumn(table.Meta()));
}

/// Builder 注册全局临时表后 Build 仍可按 ID 查到。
#[test]
fn test_build_schema_with_global_temporary_table() {
    let mut db = DBInfo {
        id: 1,
        name: ci("test"),
        tables: vec![Arc::new(TableInfo {
            id: 10,
            db_id: 1,
            name: ci("tmp"),
            ..Default::default()
        })],
        table_name_2_id: Default::default(),
    };
    let data = NewData();
    let mut builder = NewBuilder(0, data, false);
    builder.addTemporaryTable(10);
    builder.InitWithDBInfos(std::slice::from_mut(&mut db), Vec::new(), Vec::new(), 1);
    let is = builder.Build(1);
    assert!(is.TableByID(10).is_some());
    assert!(is.HasTemporaryTable());
}

/// ApplyDiff 创建表：通过内存 MetadataReader 注入表元数据并校验受影响 ID。
#[test]
fn test_apply_diff_create_table() {
    /// 内存元数据读取器：按 schema/table ID 返回固定库表。
    struct MemMeta {
        db: DBInfo,
        table: TableInfo,
    }
    impl MetadataReader for MemMeta {
        fn database(&self, id: i64) -> Result<Option<DBInfo>, String> {
            if id == self.db.id {
                Ok(Some(self.db.clone()))
            } else {
                Ok(None)
            }
        }
        fn table(&self, schema_id: i64, table_id: i64) -> Result<Option<TableInfo>, String> {
            if schema_id == self.db.id && table_id == self.table.id {
                Ok(Some(self.table.clone()))
            } else {
                Ok(None)
            }
        }
    }

    let mut db = DBInfo {
        id: 1,
        name: ci("test"),
        tables: Vec::new(),
        table_name_2_id: Default::default(),
    };
    let data = NewData();
    let mut builder = NewBuilder(0, data, false);
    builder.InitWithDBInfos(std::slice::from_mut(&mut db), Vec::new(), Vec::new(), 1);

    let table = TableInfo {
        id: 20,
        db_id: 1,
        name: ci("t1"),
        ..Default::default()
    };
    let meta = MemMeta {
        db: DBInfo {
            id: 1,
            name: ci("test"),
            tables: vec![Arc::new(table.clone())],
            table_name_2_id: Default::default(),
        },
        table: table.clone(),
    };
    let affected = builder
        .ApplyDiff(
            &meta,
            &SchemaDiff {
                version: 2,
                action_type: ActionType::CreateTable,
                schema_id: 1,
                table_id: 20,
                ..Default::default()
            },
        )
        .expect("apply create table");
    assert!(affected.contains(&20));
    let is = builder.Build(2);
    assert!(is.TableByName(&ci("test"), &ci("t1")).is_ok());
}

/// 会话级临时表：Add / Exists / Remove / Count。
#[test]
fn test_local_temporary_tables() {
    let mut st = NewSessionTables();
    assert_eq!(0, st.Count());
    let db = DBInfo {
        id: 1,
        name: ci("tmpdb"),
        tables: Vec::new(),
        table_name_2_id: Default::default(),
    };
    let tbl = Table::new(TableInfo {
        id: 7,
        db_id: 1,
        name: ci("t"),
        ..Default::default()
    });
    st.AddTable(db, tbl).expect("add temp table");
    assert_eq!(1, st.Count());
    assert!(st.TableExists(&ci("tmpdb"), &ci("t")));
    assert!(st.RemoveTable(&ci("tmpdb"), &ci("t")));
    assert_eq!(0, st.Count());
    assert!(st.SchemaByID(1).is_none());
}

#[test]
fn test_schema_by_table_falls_back_to_table_id() {
    let table = TableInfo {
        id: 7,
        db_id: 0,
        name: ci("t"),
        ..Default::default()
    };
    let schema = MockInfoSchema(vec![table.clone()]);
    assert_eq!(
        Some(1),
        SchemaByTable(schema.as_ref(), &table).map(|db| db.id)
    );
}

#[test]
fn test_masking_policy_parsers_match_go_persisted_contract() {
    use crate::infoschema::MaskingPolicyStatus;

    assert_eq!(
        MaskingPolicyStatus::Enabled,
        maskingPolicyStatusFromString(" ENABLE ").unwrap()
    );
    assert_eq!(
        MaskingPolicyType::Full,
        maskingPolicyTypeFromString(" mask_full ").unwrap()
    );
    assert_eq!(
        MaskingPolicyType::Partial,
        maskingPolicyTypeFromString("MASK_PARTIAL").unwrap()
    );
    assert_eq!(
        MaskingPolicyType::Null,
        maskingPolicyTypeFromString("MASK_NULL").unwrap()
    );
    assert_eq!(
        MaskingPolicyType::Date,
        maskingPolicyTypeFromString("MASK_DATE").unwrap()
    );
    assert_eq!(
        MaskingPolicyType::Custom,
        maskingPolicyTypeFromString("CUSTOM").unwrap()
    );

    let ops = maskingPolicyRestrictOpsFromString(
        " insert_into_select, UPDATE_SELECT,delete_select,CTAS ",
    )
    .unwrap();
    assert_eq!(
        MaskingPolicyRestrictOps::INSERT_INTO_SELECT.0
            | MaskingPolicyRestrictOps::UPDATE_SELECT.0
            | MaskingPolicyRestrictOps::DELETE_SELECT.0
            | MaskingPolicyRestrictOps::CTAS.0,
        ops.0
    );
    assert_eq!(
        MaskingPolicyRestrictOps::NONE,
        maskingPolicyRestrictOpsFromString(" NONE ").unwrap()
    );
}

#[test]
fn test_masking_policy_query_and_batches_match_go() {
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingLoader(Mutex<Vec<Vec<i64>>>);
    impl MaskingPolicyLoader for RecordingLoader {
        fn load(&self, ids: &[i64], _: u64) -> Result<Vec<MaskingPolicyInfo>, InfoSchemaError> {
            self.0.lock().unwrap().push(ids.to_vec());
            Ok(ids
                .iter()
                .rev()
                .map(|id| MaskingPolicyInfo {
                    id: *id,
                    table_id: *id,
                    column_id: 1,
                    ..Default::default()
                })
                .collect())
        }
    }

    let (query, args) = buildLoadMaskingPoliciesQuery(&[2, 1]);
    assert!(query.starts_with("SELECT policy_id, policy_name, db_name, table_name"));
    assert!(query.contains("FROM mysql.tidb_masking_policy WHERE table_id IN (%?, %?)"));
    assert!(query.ends_with("ORDER BY table_id, column_id, policy_id"));
    assert_eq!(vec![2, 1], args);

    let loader = RecordingLoader::default();
    let ids: Vec<i64> = (1..=1025).rev().collect();
    let policies = loadMaskingPoliciesWithTableIDs(&loader, &ids, 42).unwrap();
    let calls = loader.0.lock().unwrap();
    assert_eq!(2, calls.len());
    assert_eq!(1024, calls[0].len());
    assert_eq!(vec![1025], calls[1]);
    assert!(
        policies
            .windows(2)
            .all(|pair| pair[0].table_id <= pair[1].table_id)
    );

    let schema = infoSchema::new(0);
    for (id, name) in [(3, "b"), (2, "a"), (1, "a")] {
        schema.put_masking_policy(MaskingPolicyInfo {
            id,
            name: ci(name),
            table_id: id,
            column_id: 1,
            ..Default::default()
        });
    }
    schema.set_masking_policies_loaded(true);
    assert_eq!(
        vec![("a", 1), ("a", 2), ("b", 3)],
        schema
            .AllMaskingPolicies()
            .iter()
            .map(|policy| (policy.name.lower.as_str(), policy.id))
            .collect::<Vec<_>>()
    );
}

/// 启用 InfoSchema V2：设置缓存容量并确认 `IsV2`。
#[test]
fn test_enable_info_schema_v2() {
    let data = NewData();
    data.SetCacheCapacity(1024 * 1024);
    assert!(data.CacheCapacity() > 0);
    let is = NewInfoSchemaV2(data, 1, 1);
    assert!(IsV2(&is));
}
