// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 按物理 ID（表或分区）查找表元信息。
//
// InfoSchema V1 对分区查找代价高，初始化统计时用带 schema 版本的分区→表缓存；
// V2 则直接走 InfoSchema 查询。physical_id 可能是表 ID 或分区 ID。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// 分区定义：分区 ID 与名称。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartitionDefinition {
    pub id: i64,
    pub name: String,
}

/// 表元数据：物理表 ID、库表名及分区列表。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableMeta {
    pub id: i64,
    pub schema_name: String,
    pub table_name: String,
    pub partitions: Vec<PartitionDefinition>,
}

/// 轻量表条目：仅 ID 与库表名，用于初始化统计等路径。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableItem {
    pub id: i64,
    pub schema_name: String,
    pub table_name: String,
}

/// 信息模式（InfoSchema）抽象：按版本与 ID 查询表/分区元数据。
pub trait InfoSchema: Send + Sync {
    fn schema_meta_version(&self) -> i64;
    fn is_v2(&self) -> bool;
    fn table_by_id(&self, physical_id: i64) -> Option<Arc<TableMeta>>;
    fn find_table_by_partition_id(&self, partition_id: i64) -> Option<Arc<TableMeta>>;
    fn table_item_by_id(&self, id: i64) -> Option<TableItem>;
    fn table_item_by_partition_id(&self, partition_id: i64) -> Option<TableItem>;
    fn partitioned_tables(&self) -> Vec<Arc<TableMeta>>;
}

/// 统计侧表信息查找接口，区分普通查询与初始化统计路径。
pub trait TableInfoGetter: Send + Sync {
    fn table_info_by_id(
        &self,
        info_schema: &dyn InfoSchema,
        physical_id: i64,
    ) -> Option<Arc<TableMeta>>;
    fn table_info_by_id_for_init_stats(
        &self,
        info_schema: &dyn InfoSchema,
        physical_id: i64,
    ) -> Option<Arc<TableMeta>>;
    fn table_item_by_id_for_init_stats(
        &self,
        info_schema: &dyn InfoSchema,
        physical_id: i64,
    ) -> Option<TableItem>;
    fn table_item_by_id(&self, info_schema: &dyn InfoSchema, physical_id: i64)
    -> Option<TableItem>;
}

/// InfoSchema V1 初始化统计用的分区→表 ID 缓存，随 schema 版本失效重建。
#[derive(Default)]
struct PartitionCache {
    partition_to_table: HashMap<i64, i64>,
    schema_version: i64,
}

/// 带 V1 分区缓存的 `TableInfoGetter` 实现。
#[derive(Default)]
pub struct CachedTableInfoGetter {
    init_stats_v1: Mutex<PartitionCache>,
}

impl CachedTableInfoGetter {
    /// 构造空缓存的 getter。
    pub fn new() -> Self {
        Self::default()
    }

    /// V1 初始化统计：按分区 ID 查所属表 ID，必要时按 schema 版本重建映射。
    fn partition_id_to_table_id_for_init_stats(
        &self,
        info_schema: &dyn InfoSchema,
        partition_id: i64,
    ) -> Option<i64> {
        // InfoSchema V1 performs a full table scan for every partition lookup.
        // Serialize cache rebuild and lookup exactly as Go does because callers
        // can initialize statistics concurrently.
        // V1 每次分区查找会全表扫描；与 Go 一样串行化缓存重建，避免并发 init 竞态。
        let mut cache = self.init_stats_v1.lock().unwrap();
        let version = info_schema.schema_meta_version();
        if version != cache.schema_version {
            cache.schema_version = version;
            cache.partition_to_table = build_partition_id_to_table_id(info_schema);
        }
        cache.partition_to_table.get(&partition_id).copied()
    }
}

impl TableInfoGetter for CachedTableInfoGetter {
    fn table_info_by_id(
        &self,
        info_schema: &dyn InfoSchema,
        physical_id: i64,
    ) -> Option<Arc<TableMeta>> {
        // 先按表 ID，再按分区 ID 反查所属表
        info_schema
            .table_by_id(physical_id)
            .or_else(|| info_schema.find_table_by_partition_id(physical_id))
    }

    fn table_info_by_id_for_init_stats(
        &self,
        info_schema: &dyn InfoSchema,
        physical_id: i64,
    ) -> Option<Arc<TableMeta>> {
        if info_schema.is_v2() {
            return self.table_info_by_id(info_schema, physical_id);
        }
        // V1：表 ID 直查失败时，经分区缓存解析出表 ID 再查
        info_schema.table_by_id(physical_id).or_else(|| {
            self.partition_id_to_table_id_for_init_stats(info_schema, physical_id)
                .and_then(|table_id| info_schema.table_by_id(table_id))
        })
    }

    fn table_item_by_id_for_init_stats(
        &self,
        info_schema: &dyn InfoSchema,
        physical_id: i64,
    ) -> Option<TableItem> {
        if info_schema.is_v2() {
            return self.table_item_by_id(info_schema, physical_id);
        }
        info_schema.table_item_by_id(physical_id).or_else(|| {
            self.partition_id_to_table_id_for_init_stats(info_schema, physical_id)
                .and_then(|table_id| info_schema.table_item_by_id(table_id))
        })
    }

    fn table_item_by_id(
        &self,
        info_schema: &dyn InfoSchema,
        physical_id: i64,
    ) -> Option<TableItem> {
        info_schema
            .table_item_by_id(physical_id)
            .or_else(|| info_schema.table_item_by_partition_id(physical_id))
    }
}

/// 遍历所有分区表，构建「分区 ID → 所属表 ID」映射。
pub fn build_partition_id_to_table_id(info_schema: &dyn InfoSchema) -> HashMap<i64, i64> {
    let mut mapping = HashMap::new();
    for table in info_schema.partitioned_tables() {
        for partition in &table.partitions {
            mapping.insert(partition.id, table.id);
        }
    }
    mapping
}

/// 构造默认的带缓存表信息 getter。
pub fn new_table_info_getter() -> Arc<dyn TableInfoGetter> {
    Arc::new(CachedTableInfoGetter::new())
}
