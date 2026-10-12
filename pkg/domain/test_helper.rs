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
