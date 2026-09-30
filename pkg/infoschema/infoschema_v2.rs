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

// InfoSchema V2：基于版本化（MVCC 风格）元数据与表缓存的实现。
//
// 相对 V1，V2 将库/表/分区/外键历史按 schema 版本追加记录（含 tomb 删除标记），
// 查询时取 `schema_version <= 快照版本` 的最新可见记录；表实体可经 SIEVE 缓存加速。

#![allow(non_camel_case_types, non_snake_case)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use crate::infoschema::{
    CiString, DBInfo, InfoSchema, InfoSchemaError, MaskingPolicyInfo, MaskingPolicyLoader,
    PartitionDefinition, PlacementBundle, PolicyInfo, ReferredFKInfo, Table, TableInfo, TableItem,
};
use crate::sieve::{Sieve, newSieve};
use astersql_infoschema_context as context_dependency;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
/// 表缓存键：表 ID + schema 版本。
struct TableCacheKey {
    table_id: i64,
    schema_version: i64,
}

#[derive(Clone)]
/// 某一 schema 版本下的表记录（可含 tomb 表示删除）。
struct TableRecord {
    db_name: CiString,
    db_id: i64,
    table_name: CiString,
    table_id: i64,
    schema_version: i64,
    table: Option<Table>,
    tomb: bool,
}
#[derive(Clone)]
/// 某一 schema 版本下的库记录。
struct SchemaRecord {
    schema_version: i64,
    db: Arc<DBInfo>,
    tomb: bool,
}
#[derive(Clone)]
/// 分区 ID → 表 ID 的版本化映射记录。
struct PartitionRecord {
    schema_version: i64,
    table_id: i64,
    tomb: bool,
}
#[derive(Clone)]
/// 某一版本下父表的反向外键列表。
struct ForeignKeyRecord {
    schema_version: i64,
    refs: Vec<ReferredFKInfo>,
    tomb: bool,
}

#[derive(Default)]
/// 全部版本化索引的内存容器。
struct VersionedData {
    by_id: HashMap<i64, Vec<TableRecord>>,
    by_name: HashMap<(String, String), Vec<TableRecord>>,
    schema_by_name: HashMap<String, Vec<SchemaRecord>>,
    schema_id_to_name: HashMap<i64, Vec<(i64, CiString, bool)>>,
    partitions: HashMap<i64, Vec<PartitionRecord>>,
    referred_foreign_keys: HashMap<(String, String), Vec<ForeignKeyRecord>>,
    specials: HashMap<String, (Arc<DBInfo>, Vec<Table>)>,
}

/// 所有 InfoSchema V2 快照共享的 MVCC（多版本并发控制风格）元数据后端。
/// Shared MVCC metadata backing every InfoSchema v2 snapshot.
pub struct Data {
    inner: RwLock<VersionedData>,
    table_cache: Sieve<TableCacheKey, Table>,
    recent_min_ts: AtomicU64,
    temporary_table_ids: RwLock<HashSet<i64>>,
}

impl Default for Data {
    fn default() -> Self {
        Self::new()
    }
}

impl Data {
    /// 创建默认容量的共享 Data。
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(VersionedData::default()),
            table_cache: newSieve(1024 * 1024 * 1024),
            recent_min_ts: AtomicU64::new(0),
            temporary_table_ids: RwLock::new(HashSet::new()),
        }
    }
    /// 表 SIEVE 缓存容量。
    pub fn CacheCapacity(&self) -> u64 {
        self.table_cache.Capacity()
    }
    /// 设置缓存容量并等待淘汰完成。
    pub fn SetCacheCapacity(&self, capacity: u64) {
        self.table_cache.SetCapacityAndWaitEvict(capacity);
    }

    /// 在底层表 SIEVE 缓存上安装状态钩子（对应 Go 测试路径）。
    /// Install a status hook on the underlying table SIEVE cache (Go test path).
    pub fn SetStatusHook(&self, hook: Arc<dyn crate::sieve::SieveStatusHook>) {
        self.table_cache.SetStatusHook(hook);
    }

    pub fn addTemporaryTable(&self, table_id: i64) {
        self.temporary_table_ids
            .write()
            .expect("infoschema v2 temporary-table lock poisoned")
            .insert(table_id);
    }

    pub fn removeTemporaryTable(&self, table_id: i64) {
        self.temporary_table_ids
            .write()
            .expect("infoschema v2 temporary-table lock poisoned")
            .remove(&table_id);
    }

    pub fn hasTemporaryTable(&self) -> bool {
        !self
            .temporary_table_ids
            .read()
            .expect("infoschema v2 temporary-table lock poisoned")
            .is_empty()
    }

    /// 同 `CacheCapacity` 的别名访问。
    pub fn table_cache_capacity(&self) -> u64 {
        self.table_cache.Capacity()
    }

    /// 在指定 schema 版本登记一张表（含分区与外键反向索引），并写入缓存。
    pub fn add(&self, db: &DBInfo, table: Table, schema_version: i64) {
        let record = TableRecord {
            db_name: db.name.clone(),
            db_id: db.id,
            table_name: table.Meta().name.clone(),
            table_id: table.Meta().id,
            schema_version,
            table: Some(table.clone()),
            tomb: false,
        };
        let mut data = self.inner.write().expect("infoschema v2 lock poisoned");
        insert_table(
            data.by_id.entry(record.table_id).or_default(),
            record.clone(),
        );
        insert_table(
            data.by_name
                .entry((
                    record.db_name.lower.clone(),
                    record.table_name.lower.clone(),
                ))
                .or_default(),
            record.clone(),
        );
        if let Some(partitions) = &table.Meta().partition {
            for partition in &partitions.definitions {
                insert_partition(
                    data.partitions.entry(partition.id).or_default(),
                    PartitionRecord {
                        schema_version,
                        table_id: table.Meta().id,
                        tomb: false,
                    },
                );
            }
        }
        for foreign_key in &table.Meta().foreign_keys {
            let key = (
                foreign_key.ref_schema.lower.clone(),
                foreign_key.ref_table.lower.clone(),
            );
            let mut refs = visible_fk(data.referred_foreign_keys.get(&key), schema_version)
                .unwrap_or_default();
            let reference = ReferredFKInfo {
                child_schema: db.name.clone(),
                child_table: table.Meta().name.clone(),
                child_fk_name: foreign_key.name.clone(),
            };
            if !refs.contains(&reference) {
                refs.push(reference);
            }
            refs.sort_by(|a, b| {
                (
                    &a.child_schema.lower,
                    &a.child_table.lower,
                    &a.child_fk_name.lower,
                )
                    .cmp(&(
                        &b.child_schema.lower,
                        &b.child_table.lower,
                        &b.child_fk_name.lower,
                    ))
            });
            insert_fk(
                data.referred_foreign_keys.entry(key).or_default(),
                ForeignKeyRecord {
                    schema_version,
                    refs,
                    tomb: false,
                },
            );
        }
        drop(data);
        self.table_cache.Set(
            TableCacheKey {
                table_id: table.Meta().id,
                schema_version,
            },
            table,
        );
    }

    /// 登记特殊系统库（如 information_schema）及其内存表。
    pub fn addSpecialDB(&self, db: DBInfo, tables: Vec<Table>) {
        self.inner
            .write()
            .expect("infoschema v2 lock poisoned")
            .specials
            .entry(db.name.lower.clone())
            .or_insert((Arc::new(db), tables));
    }
    /// 在指定版本登记一个库（清空内嵌 tables，表走独立索引）。
    pub fn addDB(&self, schema_version: i64, mut db: DBInfo) {
        db.tables.clear();
        let db = Arc::new(db);
        let mut data = self.inner.write().expect("infoschema v2 lock poisoned");
        insert_schema(
            data.schema_by_name
                .entry(db.name.lower.clone())
                .or_default(),
            SchemaRecord {
                schema_version,
                db: db.clone(),
                tomb: false,
            },
        );
        let versions = data.schema_id_to_name.entry(db.id).or_default();
        versions.push((schema_version, db.name.clone(), false));
        versions.sort_by_key(|record| std::cmp::Reverse(record.0));
    }
    /// 在指定版本以 tomb 删除表，并清理其外键反向引用。
    pub fn remove(
        &self,
        db_name: CiString,
        db_id: i64,
        table_name: CiString,
        table_id: i64,
        schema_version: i64,
    ) {
        let previous = {
            let data = self.inner.read().expect("infoschema v2 lock poisoned");
            visible_table(data.by_id.get(&table_id), schema_version.saturating_sub(1))
        };
        let record = TableRecord {
            db_name,
            db_id,
            table_name,
            table_id,
            schema_version,
            table: None,
            tomb: true,
        };
        let mut data = self.inner.write().expect("infoschema v2 lock poisoned");
        insert_table(data.by_id.entry(table_id).or_default(), record.clone());
        insert_table(
            data.by_name
                .entry((
                    record.db_name.lower.clone(),
                    record.table_name.lower.clone(),
                ))
                .or_default(),
            record.clone(),
        );
        // Mirror Go deleteReferredForeignKeys: dropping a child table removes
        // its FK entries from each referenced parent at this schema version.
        if let Some(prev) = previous.and_then(|item| item.table) {
            for foreign_key in &prev.Meta().foreign_keys {
                let key = (
                    foreign_key.ref_schema.lower.clone(),
                    foreign_key.ref_table.lower.clone(),
                );
                let mut refs = visible_fk(data.referred_foreign_keys.get(&key), schema_version)
                    .unwrap_or_default();
                refs.retain(|reference| {
                    !(reference.child_schema.lower == record.db_name.lower
                        && reference.child_table.lower == record.table_name.lower
                        && reference.child_fk_name.lower == foreign_key.name.lower)
                });
                insert_fk(
                    data.referred_foreign_keys.entry(key).or_default(),
                    ForeignKeyRecord {
                        schema_version,
                        refs,
                        tomb: false,
                    },
                );
            }
        }
    }

    /// Remove the name entry recorded in the v2 index for this ID. The cached
    /// TableInfo may carry metadata from a different version during a cutover.
    pub fn remove_by_id(&self, table_id: i64, schema_version: i64) -> bool {
        let item = {
            let data = self.inner.read().expect("infoschema v2 lock poisoned");
            visible_table(data.by_id.get(&table_id), schema_version.saturating_sub(1))
        };
        let Some(item) = item else {
            return false;
        };
        self.remove(
            item.db_name,
            item.db_id,
            item.table_name,
            table_id,
            schema_version,
        );
        true
    }
    /// 在指定版本以 tomb 删除库。
    pub fn deleteDB(&self, db: DBInfo, schema_version: i64) {
        let db = Arc::new(db);
        let mut data = self.inner.write().expect("infoschema v2 lock poisoned");
        insert_schema(
            data.schema_by_name
                .entry(db.name.lower.clone())
                .or_default(),
            SchemaRecord {
                schema_version,
                db: db.clone(),
                tomb: true,
            },
        );
        let versions = data.schema_id_to_name.entry(db.id).or_default();
        versions.push((schema_version, db.name.clone(), true));
        versions.sort_by_key(|record| std::cmp::Reverse(record.0));
    }
    /// 在给定版本查询引用某父表的外键列表。
    pub fn getTableReferredForeignKeys(
        &self,
        schema: &str,
        table: &str,
        version: i64,
    ) -> Vec<ReferredFKInfo> {
        visible_fk(
            self.inner
                .read()
                .expect("infoschema v2 lock poisoned")
                .referred_foreign_keys
                .get(&(schema.to_lowercase(), table.to_lowercase())),
            version,
        )
        .unwrap_or_default()
    }
    /// 垃圾回收低于 cut_version 的旧表历史；返回删除条数与剩余名索引规模。
    pub fn GCOldVersion(&self, cut_version: i64) -> (usize, i64) {
        let mut data = self.inner.write().expect("infoschema v2 lock poisoned");
        let mut removed = Vec::new();
        for history in data.by_name.values_mut() {
            let remaining = 1024usize.saturating_sub(removed.len());
            if remaining == 0 {
                break;
            }
            let Some(pivot) = history
                .iter()
                .position(|item| item.schema_version < cut_version)
            else {
                continue;
            };
            let remove_count = history.len().saturating_sub(pivot + 1).min(remaining);
            let split_at = history.len() - remove_count;
            removed.extend(
                history
                    .drain(split_at..)
                    .map(|item| (item.table_id, item.schema_version)),
            );
        }
        let removed: HashSet<_> = removed.into_iter().collect();
        for history in data.by_id.values_mut() {
            history.retain(|item| !removed.contains(&(item.table_id, item.schema_version)));
        }
        for history in data.referred_foreign_keys.values_mut() {
            if let Some(pivot) = history
                .iter()
                .position(|item| item.schema_version < cut_version)
            {
                history.truncate(pivot + 1);
            }
        }
        (
            removed.len(),
            data.by_name.values().map(Vec::len).sum::<usize>() as i64,
        )
    }
    /// 全量加载前：对现有历史追加 tomb，避免旧版本残留可见。
    pub fn resetBeforeFullLoad(&self, schema_version: i64) {
        let mut data = self.inner.write().expect("infoschema v2 lock poisoned");
        for history in data.by_id.values_mut() {
            if let Some(latest) = history.first().cloned() {
                insert_table(
                    history,
                    TableRecord {
                        schema_version,
                        table: None,
                        tomb: true,
                        ..latest
                    },
                );
            }
        }
        for history in data.by_name.values_mut() {
            if let Some(latest) = history.first().cloned() {
                insert_table(
                    history,
                    TableRecord {
                        schema_version,
                        table: None,
                        tomb: true,
                        ..latest
                    },
                );
            }
        }
        for history in data.schema_by_name.values_mut() {
            if let Some(latest) = history.first().cloned() {
                insert_schema(
                    history,
                    SchemaRecord {
                        schema_version,
                        tomb: true,
                        ..latest
                    },
                );
            }
        }
        for history in data.schema_id_to_name.values_mut() {
            if let Some(latest) = history.first().cloned() {
                history.retain(|old| old.0 != schema_version);
                history.push((schema_version, latest.1, true));
                history.sort_by_key(|record| std::cmp::Reverse(record.0));
            }
        }
        for history in data.partitions.values_mut() {
            if let Some(latest) = history.first().cloned() {
                insert_partition(
                    history,
                    PartitionRecord {
                        schema_version,
                        tomb: true,
                        ..latest
                    },
                );
            }
        }
        for history in data.referred_foreign_keys.values_mut() {
            if let Some(latest) = history.first().cloned() {
                insert_fk(
                    history,
                    ForeignKeyRecord {
                        schema_version,
                        refs: Vec::new(),
                        tomb: true,
                        ..latest
                    },
                );
            }
        }
    }
    /// 记录近期最小时间戳，防止 GC 过早回收仍被引用的版本。
    fn keep_alive(&self, ts: u64) {
        let mut current = self.recent_min_ts.load(Ordering::Acquire);
        while (current == 0 || ts < current)
            && self
                .recent_min_ts
                .compare_exchange_weak(current, ts, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            current = self.recent_min_ts.load(Ordering::Acquire);
        }
    }
}

/// 按版本降序插入/替换表历史记录。
fn insert_table(history: &mut Vec<TableRecord>, record: TableRecord) {
    history.retain(|old| old.schema_version != record.schema_version);
    history.push(record);
    history.sort_by_key(|item| std::cmp::Reverse(item.schema_version));
}
/// 按版本降序插入/替换库历史记录。
fn insert_schema(history: &mut Vec<SchemaRecord>, record: SchemaRecord) {
    history.retain(|old| old.schema_version != record.schema_version);
    history.push(record);
    history.sort_by_key(|item| std::cmp::Reverse(item.schema_version));
}
/// 按版本降序插入/替换分区历史记录。
fn insert_partition(history: &mut Vec<PartitionRecord>, record: PartitionRecord) {
    history.retain(|old| old.schema_version != record.schema_version);
    history.push(record);
    history.sort_by_key(|item| std::cmp::Reverse(item.schema_version));
}
/// 按版本降序插入/替换外键历史记录。
fn insert_fk(history: &mut Vec<ForeignKeyRecord>, record: ForeignKeyRecord) {
    history.retain(|old| old.schema_version != record.schema_version);
    history.push(record);
    history.sort_by_key(|item| std::cmp::Reverse(item.schema_version));
}
/// 取 `schema_version <= version` 且非 tomb 的最新表记录。
fn visible_table(history: Option<&Vec<TableRecord>>, version: i64) -> Option<TableRecord> {
    history?
        .iter()
        .find(|item| item.schema_version <= version)
        .filter(|item| !item.tomb)
        .cloned()
}
/// 取可见的最新库元数据。
fn visible_schema(history: Option<&Vec<SchemaRecord>>, version: i64) -> Option<Arc<DBInfo>> {
    history?
        .iter()
        .find(|item| item.schema_version <= version)
        .filter(|item| !item.tomb)
        .map(|item| item.db.clone())
}
/// 取可见的分区→表 ID 映射。
fn visible_partition(history: Option<&Vec<PartitionRecord>>, version: i64) -> Option<i64> {
    history?
        .iter()
        .find(|item| item.schema_version <= version)
        .filter(|item| !item.tomb)
        .map(|item| item.table_id)
}
/// 取可见的反向外键列表。
fn visible_fk(
    history: Option<&Vec<ForeignKeyRecord>>,
    version: i64,
) -> Option<Vec<ReferredFKInfo>> {
    history?
        .iter()
        .find(|item| item.schema_version <= version)
        .filter(|item| !item.tomb)
        .map(|item| item.refs.clone())
}
/// InfoSchema V2 快照：持有共享 Data、当前 schema 元版本与读时间戳。
pub struct infoschemaV2 {
    pub Data: Arc<Data>,
    schema_meta_version: i64,
    start_ts: u64,
    bundles: HashMap<i64, Arc<PlacementBundle>>,
    policies: HashMap<i64, Arc<PolicyInfo>>,
    masking_cache: HashMap<i64, HashMap<i64, Arc<MaskingPolicyInfo>>>,
    masking_loaded: bool,
    masking_loader: Option<Arc<dyn MaskingPolicyLoader>>,
}

impl infoschemaV2 {
    /// 基于共享 Data 构造指定版本/时间戳的 V2 快照。
    pub fn new(data: Arc<Data>, schema_meta_version: i64, start_ts: u64) -> Self {
        Self {
            Data: data,
            schema_meta_version,
            start_ts,
            bundles: HashMap::new(),
            policies: HashMap::new(),
            masking_cache: HashMap::new(),
            masking_loaded: false,
            masking_loader: None,
        }
    }
    pub fn with_bundles_and_policies(
        mut self,
        bundles: HashMap<i64, Arc<PlacementBundle>>,
        policies: HashMap<i64, Arc<PolicyInfo>>,
    ) -> Self {
        self.bundles = bundles;
        self.policies = policies;
        self
    }
    pub fn with_masking_cache(
        mut self,
        cache: HashMap<i64, HashMap<i64, Arc<MaskingPolicyInfo>>>,
        loaded: bool,
        loader: Option<Arc<dyn MaskingPolicyLoader>>,
    ) -> Self {
        self.masking_cache = cache;
        self.masking_loaded = loaded;
        self.masking_loader = loader;
        self
    }
    /// 克隆快照并更新读时间戳。
    pub fn CloneAndUpdateTS(&self, start_ts: u64) -> Self {
        Self::new(self.Data.clone(), self.schema_meta_version, start_ts)
            .with_bundles_and_policies(self.bundles.clone(), self.policies.clone())
            .with_masking_cache(
                self.masking_cache.clone(),
                self.masking_loaded,
                self.masking_loader.clone(),
            )
    }
    /// 表是否已在 SIEVE 缓存中。
    pub fn TableIsCached(&self, id: i64) -> bool {
        self.Data.table_cache.Contains(&TableCacheKey {
            table_id: id,
            schema_version: self.schema_meta_version,
        })
    }
    /// 从缓存中淘汰指定表。
    pub fn EvictTable(&self, schema: &CiString, table: &CiString) {
        if let Some(record) = visible_table(
            self.Data
                .inner
                .read()
                .expect("infoschema v2 lock poisoned")
                .by_name
                .get(&(schema.lower.clone(), table.lower.clone())),
            self.schema_meta_version,
        ) {
            self.Data.table_cache.Remove(&TableCacheKey {
                table_id: record.table_id,
                schema_version: record.schema_version,
            });
        }
    }
    /// 按库表名取 `TableInfo`。
    pub fn TableInfoByName(
        &self,
        schema: &CiString,
        table: &CiString,
    ) -> Result<Arc<TableInfo>, InfoSchemaError> {
        self.TableByName(schema, table).map(|table| table.0)
    }
    /// 按表 ID 取 `TableInfo`。
    pub fn TableInfoByID(&self, id: i64) -> Option<Arc<TableInfo>> {
        self.TableByID(id).map(|table| table.0)
    }
    /// 列出指定 schema 在当前版本可见的全部表信息。
    pub fn SchemaTableInfos(
        &self,
        schema: &CiString,
    ) -> Result<Vec<Arc<TableInfo>>, InfoSchemaError> {
        let data = self.Data.inner.read().expect("infoschema v2 lock poisoned");
        Ok(data
            .by_name
            .iter()
            .filter(|((db, _), _)| db == &schema.lower)
            .filter_map(|(_, history)| {
                visible_table(Some(history), self.schema_meta_version)
                    .and_then(|record| record.table.map(|table| table.0))
            })
            .collect())
    }
    /// 当前版本可见的全部 schema 名。
    pub fn AllSchemaNames(&self) -> Vec<CiString> {
        self.AllSchemas()
            .into_iter()
            .map(|db| db.name.clone())
            .collect()
    }
    /// schema 是否在当前版本可见。
    pub fn SchemaExists(&self, schema: &CiString) -> bool {
        self.SchemaByName(schema).is_some()
    }
    /// 库表是否在当前版本可见。
    pub fn TableExists(&self, schema: &CiString, table: &CiString) -> bool {
        self.TableByName(schema, table).is_ok()
    }
    /// 由分区 ID 解析所属表 ID。
    pub fn TableIDByPartitionID(&self, partition_id: i64) -> Option<i64> {
        visible_partition(
            self.Data
                .inner
                .read()
                .expect("infoschema v2 lock poisoned")
                .partitions
                .get(&partition_id),
            self.schema_meta_version,
        )
    }
    /// 由分区 ID 得到 TableItem。
    pub fn TableItemByPartitionID(&self, partition_id: i64) -> Option<TableItem> {
        self.TableItemByID(self.TableIDByPartitionID(partition_id)?)
    }
    /// 遍历当前版本全部可见表的 TableItem；visit 返回 false 则停止。
    pub fn IterateAllTableItems(&self, mut visit: impl FnMut(TableItem) -> bool) {
        let data = self.Data.inner.read().expect("infoschema v2 lock poisoned");
        for history in data.by_id.values() {
            if let Some(record) = visible_table(Some(history), self.schema_meta_version) {
                if !visit(TableItem {
                    DBName: record.db_name,
                    TableName: record.table_name,
                }) {
                    break;
                }
            }
        }
    }
    /// 查询当前版本下引用指定父表的外键。
    pub fn GetTableReferredForeignKeys(&self, schema: &str, table: &str) -> Vec<ReferredFKInfo> {
        self.Data
            .getTableReferredForeignKeys(schema, table, self.schema_meta_version)
    }
}

impl InfoSchema for infoschemaV2 {
    fn SchemaMetaVersion(&self) -> i64 {
        self.schema_meta_version
    }
    fn SchemaByName(&self, schema: &CiString) -> Option<Arc<DBInfo>> {
        let data = self.Data.inner.read().expect("infoschema v2 lock poisoned");
        data.specials
            .get(&schema.lower)
            .map(|special| special.0.clone())
            .or_else(|| {
                visible_schema(
                    data.schema_by_name.get(&schema.lower),
                    self.schema_meta_version,
                )
            })
    }
    fn SchemaByID(&self, id: i64) -> Option<Arc<DBInfo>> {
        let data = self.Data.inner.read().expect("infoschema v2 lock poisoned");
        let name = data
            .schema_id_to_name
            .get(&id)?
            .iter()
            .find(|record| record.0 <= self.schema_meta_version)
            .filter(|record| !record.2)?
            .1
            .clone();
        visible_schema(
            data.schema_by_name.get(&name.lower),
            self.schema_meta_version,
        )
        .or_else(|| {
            data.specials
                .get(&name.lower)
                .map(|special| special.0.clone())
        })
    }
    fn TableByName(&self, schema: &CiString, table: &CiString) -> Result<Table, InfoSchemaError> {
        self.Data.keep_alive(self.start_ts);
        let data = self.Data.inner.read().expect("infoschema v2 lock poisoned");
        if let Some((_, tables)) = data.specials.get(&schema.lower) {
            if let Some(found) = tables
                .iter()
                .find(|candidate| candidate.Meta().name.lower == table.lower)
            {
                return Ok(found.clone());
            }
        }
        let record = visible_table(
            data.by_name
                .get(&(schema.lower.clone(), table.lower.clone())),
            self.schema_meta_version,
        )
        .ok_or_else(|| InfoSchemaError {
            code: "ErrNoSuchTable",
            message: format!("{}.{}", schema.original, table.original),
        })?;
        drop(data);
        let key = TableCacheKey {
            table_id: record.table_id,
            schema_version: record.schema_version,
        };
        if let Some(cached) = self.Data.table_cache.Get(&key) {
            return Ok(cached);
        }
        let loaded = record.table.ok_or_else(|| InfoSchemaError {
            code: "ErrNoSuchTable",
            message: table.original.clone(),
        })?;
        self.Data.table_cache.Set(key, loaded.clone());
        Ok(loaded)
    }
    fn TableByID(&self, id: i64) -> Option<Table> {
        self.Data.keep_alive(self.start_ts);
        let record = visible_table(
            self.Data
                .inner
                .read()
                .expect("infoschema v2 lock poisoned")
                .by_id
                .get(&id),
            self.schema_meta_version,
        )?;
        let key = TableCacheKey {
            table_id: id,
            schema_version: record.schema_version,
        };
        if let Some(cached) = self.Data.table_cache.Get(&key) {
            return Some(cached);
        }
        let loaded = record.table?;
        self.Data.table_cache.Set(key, loaded.clone());
        Some(loaded)
    }
    fn SchemaTableInfos(&self, schema: &CiString) -> Result<Vec<Arc<TableInfo>>, InfoSchemaError> {
        self.SchemaTableInfos(schema)
    }
    fn HasTemporaryTable(&self) -> bool {
        self.Data.hasTemporaryTable()
    }
    fn TableItemByID(&self, id: i64) -> Option<TableItem> {
        let record = visible_table(
            self.Data
                .inner
                .read()
                .expect("infoschema v2 lock poisoned")
                .by_id
                .get(&id),
            self.schema_meta_version,
        )?;
        Some(TableItem {
            DBName: record.db_name,
            TableName: record.table_name,
        })
    }
    fn FindTableByPartitionID(
        &self,
        partition_id: i64,
    ) -> Option<(Table, Arc<DBInfo>, PartitionDefinition)> {
        let table = self.TableByID(self.TableIDByPartitionID(partition_id)?)?;
        let db = self.SchemaByID(table.Meta().db_id)?;
        let partition = table
            .Meta()
            .partition
            .as_ref()?
            .definitions
            .iter()
            .find(|partition| partition.id == partition_id)?
            .clone();
        Some((table, db, partition))
    }
    fn AllSchemas(&self) -> Vec<Arc<DBInfo>> {
        let data = self.Data.inner.read().expect("infoschema v2 lock poisoned");
        let mut schemas: Vec<_> = data
            .schema_by_name
            .values()
            .filter_map(|history| visible_schema(Some(history), self.schema_meta_version))
            .collect();
        schemas.extend(data.specials.values().map(|special| special.0.clone()));
        schemas
    }
    fn AllPlacementPolicies(&self) -> Vec<Arc<PolicyInfo>> {
        self.policies.values().cloned().collect()
    }
    fn PlacementBundleByPhysicalTableID(&self, id: i64) -> Option<Arc<PlacementBundle>> {
        self.bundles.get(&id).cloned()
    }
    fn AllPlacementBundles(&self) -> Vec<Arc<PlacementBundle>> {
        self.bundles.values().cloned().collect()
    }
    fn MaskingCacheSnapshot(&self) -> (HashMap<i64, HashMap<i64, Arc<MaskingPolicyInfo>>>, bool) {
        (self.masking_cache.clone(), self.masking_loaded)
    }
    fn MaskingLoader(&self) -> Option<Arc<dyn MaskingPolicyLoader>> {
        self.masking_loader.clone()
    }
    fn ListTablesWithSpecialAttribute(
        &self,
        filter: context_dependency::SpecialAttributeFilter,
    ) -> Vec<context_dependency::TableInfoResult> {
        let data = self.Data.inner.read().expect("infoschema v2 lock poisoned");
        let mut matches = data
            .by_id
            .values()
            .filter_map(|history| {
                let record = visible_table(Some(history), self.schema_meta_version)?;
                let table_info = record.table?.Meta().model_meta.clone()?;
                filter(table_info.as_ref()).then_some((record.db_name, record.table_id, table_info))
            })
            .collect::<Vec<_>>();
        drop(data);

        // Go's tableInfoResident btree is traversed with Descend: database
        // name, table ID, and schema version are all visited in descending
        // order. `visible_table` has already selected one version per ID.
        matches.sort_by(|left, right| {
            right
                .0
                .lower
                .cmp(&left.0.lower)
                .then_with(|| right.1.cmp(&left.1))
        });

        let mut results: Vec<context_dependency::TableInfoResult> = Vec::new();
        for (db_name, _, table_info) in matches {
            if let Some(current) = results
                .last_mut()
                .filter(|current| current.DBName.L == db_name.lower)
            {
                current.TableInfos.push(table_info);
                continue;
            }
            results.push(context_dependency::TableInfoResult {
                DBName: astersql_parser_ast::NewCIStr(&db_name.original),
                TableInfos: vec![table_info],
            });
        }
        results
    }
    fn IsV2(&self) -> bool {
        true
    }
    fn GCOldVersion(&self, cut_version: i64) -> Option<(usize, i64)> {
        Some(self.Data.GCOldVersion(cut_version))
    }
}

/// 构造共享的空 Data。
pub fn NewData() -> Arc<Data> {
    Arc::new(Data::new())
}
/// 构造 InfoSchema V2 快照。
pub fn NewInfoSchemaV2(data: Arc<Data>, schema_meta_version: i64, start_ts: u64) -> infoschemaV2 {
    infoschemaV2::new(data, schema_meta_version, start_ts)
}
/// 判断给定 InfoSchema 是否为 V2 实现。
pub fn IsV2(schema: &dyn InfoSchema) -> bool {
    schema.IsV2()
}
/// 是否为系统特殊库（information_schema / performance_schema / metrics_schema 等）。
pub fn IsSpecialDB(db_name: &str) -> bool {
    matches!(
        db_name.to_ascii_lowercase().as_str(),
        "information_schema" | "performance_schema" | "metrics_schema" | "inspection_schema"
    )
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 控制查表时是否回填（refill）SIEVE 缓存的选项。
pub struct RefillOption(pub bool);
/// 构造 RefillOption。
pub fn WithRefillOption(refill: bool) -> RefillOption {
    RefillOption(refill)
}
