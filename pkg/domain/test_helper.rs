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

// Domain 单元测试辅助工具。
//
// 提供 mock infoCache、按库表名取 table ID、枚举全部 schema/表 等仅测试用 API，
// 对齐 Go 的 Domain test helper 拆分。
//
// 文件前半为贴近 Go 的机械翻译草稿（块注释内），后半为可编译的 `DomainTestHelper`。

// This file contains utilities for easier testing.
//
// Domain 只列出本文件测试 helper 访问的字段/方法占位。
// pub struct Domain {
//     pub infoCache: InfoCache,
//     pub isSyncer: SchemaSyncer,
// }
//
// pub struct InfoCache;
// pub struct SchemaSyncer;
// pub struct InfoSchema;
// pub struct TestingT;
// pub struct MetaReader;
// pub struct Table;
// pub struct TableInfo {
//     pub ID: i64,
//     pub Partition: PartitionInfo,
// }
// pub struct PartitionInfo {
//     pub Definitions: Vec<PartitionDefinition>,
// }
// pub struct PartitionDefinition {
//     pub ID: i64,
// }
// pub struct DBInfo;
//
// impl Domain {
// MockInfoCacheAndLoadInfoSchema only used in unit tests.
// 对应 Go 方法：重置 infoCache 容量并插入传入的 infoschema。
//     pub fn MockInfoCacheAndLoadInfoSchema(&mut self, is: InfoSchema) {
//         self.infoCache.Reset(16);
//         self.infoCache.Insert(is, 0);
//     }
//
// MustGetTableInfo returns the table info. Only used in unit tests.
// Go 里 require.Nil(t, err) 会在测试失败时终止；用 Result 再 unwrap_or_default 模拟必须成功的语义。
//     pub fn MustGetTableInfo(&self, t: &TestingT, dbName: &str, tableName: &str) -> TableInfo {
//         let tbl = self
//             .InfoSchema()
//             .TableByName(Context, NewCIStr(dbName), NewCIStr(tableName));
//         require_nil(t, &tbl.err);
//         tbl.table.Meta()
//     }
//
// MustGetTableID returns the table ID. Only used in unit tests.
// 对应 Go 方法：复用 MustGetTableInfo 后返回 TableInfo.ID。
//     pub fn MustGetTableID(&self, t: &TestingT, dbName: &str, tableName: &str) -> i64 {
//         let ti = self.MustGetTableInfo(t, dbName, tableName);
//         ti.ID
//     }
//
// MustGetPartitionAt returns the partition ID. Only used in unit tests.
// 对应 Go 方法：按 idx 访问 Partition.Definitions[idx].ID，未额外处理越界。
//     pub fn MustGetPartitionAt(&self, t: &TestingT, dbName: &str, tableName: &str, idx: usize) -> i64 {
//         let ti = self.MustGetTableInfo(t, dbName, tableName);
//         ti.Partition.Definitions[idx].ID
//     }
//
// FetchAllSchemasWithTables calls the internal function. Only used in unit tests.
// 对应 Go 方法：直接转发到 do.isSyncer.FetchAllSchemasWithTables。
//     pub fn FetchAllSchemasWithTables(&self, m: MetaReader) -> Result<Vec<DBInfo>, String> {
//         self.isSyncer.FetchAllSchemasWithTables(m)
//     }
//
//     pub fn InfoSchema(&self) -> InfoSchema {
//         InfoSchema
//     }
// }
//
// impl InfoCache {
//     pub fn Reset(&mut self, _capacity: usize) {}
//
//     pub fn Insert(&mut self, _is: InfoSchema, _schema_version: i64) {}
// }
//
// pub struct Context;
// pub struct CIStr(String);
//
// pub fn NewCIStr(value: &str) -> CIStr {
//     CIStr(value.to_string())
// }
//
// pub struct TableByNameResult {
//     pub table: Table,
//     pub err: Option<String>,
// }
//
// impl InfoSchema {
//     pub fn TableByName(&self, _ctx: Context, _dbName: CIStr, _tableName: CIStr) -> TableByNameResult {
//         TableByNameResult {
//             table: Table,
//             err: None,
//         }
//     }
// }
//
// impl Table {
//     pub fn Meta(&self) -> TableInfo {
//         TableInfo {
//             ID: 0,
//             Partition: PartitionInfo {
//                 Definitions: Vec::new(),
//             },
//         }
//     }
// }
//
// impl SchemaSyncer {
//     pub fn FetchAllSchemasWithTables(&self, _m: MetaReader) -> Result<Vec<DBInfo>, String> {
//         Ok(Vec::new())
//     }
// }
//
// fn require_nil(_t: &TestingT, err: &Option<String>) {
// Go 的 require.Nil 会写入 testing.T；不触发测试框架，只保留错误检查位置。
//     if err.is_some() {
//         panic!("require.Nil failed");
//     }
// }
// */
use std::sync::Arc;

use astersql_infoschema::{CiString, SchemaRef, TableInfo};

use crate::domain::Domain;

/// Domain 测试扩展 trait：与生产路径隔离的便捷方法。
/// Test support remains in a separate module, matching Go's test helper split.
pub trait DomainTestHelper {
    /// 重置 infoCache 容量并插入给定 infoschema（InfoSchema：内存中的库表元数据快照）。
    fn mock_info_cache_and_load_info_schema(&self, schema: SchemaRef);
    /// 按库表名取表元信息；不存在则 panic（对齐 Go require 必须成功语义）。
    fn must_get_table_info(&self, database: &str, table: &str) -> Arc<TableInfo>;
    /// 按库表名取 table ID；不存在则 panic（对齐 Go require 必须成功语义）。
    fn must_get_table_id(&self, database: &str, table: &str) -> i64;
    /// 按定义顺序取分区 ID；缺少分区或索引越界时 panic，与 Go 直接索引一致。
    fn must_get_partition_at(&self, database: &str, table: &str, index: usize) -> i64;
    /// 列出全部 schema 及其表名与 ID（测试用轻量视图）。
    fn fetch_all_schemas_with_tables(&self) -> Vec<(String, Vec<(String, i64)>)>;
}

impl DomainTestHelper for Domain {
    /// 实现：Reset(16) 后 Insert(schema, schema_version=0)。
    fn mock_info_cache_and_load_info_schema(&self, schema: SchemaRef) {
        let cache = self.info_cache();
        cache.Reset(16);
        cache.Insert(schema, 0);
    }

    fn must_get_table_info(&self, database: &str, table: &str) -> Arc<TableInfo> {
        self.info_schema()
            .TableByName(&CiString::new(database), &CiString::new(table))
            .unwrap_or_else(|_| panic!("table {database}.{table} does not exist"))
            .0
    }

    /// 实现：InfoSchema.TableByName → Meta().id。
    fn must_get_table_id(&self, database: &str, table: &str) -> i64 {
        self.must_get_table_info(database, table).id
    }

    fn must_get_partition_at(&self, database: &str, table: &str, index: usize) -> i64 {
        self.must_get_table_info(database, table)
            .partition
            .as_ref()
            .expect("table is not partitioned")
            .definitions[index]
            .id
    }

    /// 实现：遍历 AllSchemas，收集 (库名, [(表名, id), ...])。
    fn fetch_all_schemas_with_tables(&self) -> Vec<(String, Vec<(String, i64)>)> {
        self.info_schema()
            .AllSchemas()
            .into_iter()
            .map(|database| {
                (
                    database.name.original.clone(),
                    database
                        .tables
                        .iter()
                        .map(|table| (table.name.original.clone(), table.id))
                        .collect(),
                )
            })
            .collect()
    }
}
