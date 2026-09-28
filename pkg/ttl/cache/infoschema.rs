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

// InfoSchema（信息模式，描述库/表/列等元数据）上的 TTL 表缓存。
//
// 按物理表 ID（非分区表用 table id，分区表用 partition id）建立到 `PhysicalTable`
// 的映射；仅当 `SchemaMetaVersion` 变化时重建，避免重复扫描全部 TTL 表。

// InfoSchemaCache 如何按物理表 ID 缓存 TTL 表信息。

use std::collections::HashMap;
use std::time::Duration;

use crate::base::{baseCache, newBaseCache};
use crate::table::{NewPhysicalTable, PhysicalTable, TableInfo};

/// InfoSchema 侧提供的一条 TTL 候选表：库名、表元数据、是否开启 TTL。
#[derive(Clone, Debug)]
pub struct TTLTableEntry {
    /// 库（schema）名。
    pub schema: String,
    /// 表元数据（含分区与 TTL 配置）。
    pub table: TableInfo,
    /// 是否已启用 TTL（对应 Go 中 `TTLInfo.Enable`）。
    pub enabled: bool,
}
/// 从 InfoSchema 拉取 schema 版本与 TTL 表列表的抽象，便于测试注入 mock。
pub trait InfoSchemaProvider {
    /// 当前 InfoSchema 的元数据版本号；未变化时可跳过缓存重建。
    fn schema_meta_version(&self) -> i64;
    /// 返回带 TTL 属性的表条目（调用方再按 enable/public 过滤）。
    fn ttl_tables(&self) -> Vec<TTLTableEntry>;
}
/// 按物理表 ID 索引的 TTL 物理表缓存，内嵌 `baseCache` 控制刷新节奏。
pub struct InfoSchemaCache {
    /// 刷新间隔与最近更新时间。
    cache: baseCache,
    /// 上次成功同步时的 SchemaMetaVersion。
    schema_version: i64,
    /// 物理表 ID → `PhysicalTable`（分区表按 partition id）。
    pub Tables: HashMap<i64, PhysicalTable>,
}
/// 创建空的 InfoSchema TTL 缓存，指定基础刷新间隔。
pub fn NewInfoSchemaCache(update_interval: Duration) -> InfoSchemaCache {
    InfoSchemaCache {
        cache: newBaseCache(update_interval),
        schema_version: 0,
        Tables: HashMap::new(),
    }
}
impl InfoSchemaCache {
    /// 是否已超过刷新间隔，需要再次 `Update`。
    pub fn ShouldUpdate(&self) -> bool {
        self.cache.ShouldUpdate()
    }
    /// 调整底层 `baseCache` 的刷新间隔。
    pub fn SetInterval(&mut self, interval: Duration) {
        self.cache.SetInterval(interval);
    }
    /// 返回当前缓存对应的 schema 版本。
    pub fn SchemaVersion(&self) -> i64 {
        self.schema_version
    }
    /// 当 schema 版本变化时重建物理表映射；版本相同则直接返回。
    pub fn Update(&mut self, schema: &dyn InfoSchemaProvider) {
        let version = schema.schema_meta_version();
        // schema 版本未变：沿用旧缓存，避免重复遍历。
        if version == self.schema_version {
            return;
        }
        let mut tables = HashMap::with_capacity(self.Tables.len());
        for entry in schema.ttl_tables() {
            // 只保留开启 TTL、public、且带 TTL 配置的表。
            if !entry.enabled || !entry.table.public || entry.table.ttl.is_none() {
                continue;
            }
            if entry.table.partitions.is_empty() {
                // 非分区表：以 table id 为物理 ID。
                if let Ok(table) = self.newTable(&entry.schema, &entry.table, "") {
                    tables.insert(entry.table.id, table);
                }
            } else {
                // 分区表：每个分区一条 PhysicalTable，键为 partition id。
                for partition in &entry.table.partitions {
                    if let Ok(table) = self.newTable(&entry.schema, &entry.table, &partition.name) {
                        tables.insert(partition.id, table);
                    }
                }
            }
        }
        self.Tables = tables;
        self.schema_version = version;
        self.cache.MarkUpdated();
    }
    /// 构造或复用 `PhysicalTable`：TableInfo 未变时复用旧缓存项。
    fn newTable(
        &self,
        schema: &str,
        info: &TableInfo,
        partition: &str,
    ) -> Result<PhysicalTable, String> {
        // 无分区名则用表 ID；有分区名则解析为对应 partition id。
        let id = if partition.is_empty() {
            info.id
        } else {
            info.partitions
                .iter()
                .find(|item| item.name.eq_ignore_ascii_case(partition))
                .map(|item| item.id)
                .unwrap_or(info.id)
        };
        // TableInfo 内容相同则复用，避免重复分配。
        if let Some(cached) = self.Tables.get(&id) {
            if cached.TableInfo == *info {
                return Ok(cached.clone());
            }
        }
        NewPhysicalTable(schema, info, partition)
    }
}
