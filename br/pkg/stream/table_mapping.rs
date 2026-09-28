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

//! PITR 上下游 ID 映射管理：对齐 Go `br/pkg/stream/table_mapping.go`。
//! 从 meta KV（DefaultCF/WriteCF）解析库/表/分区，维护临时下游 ID 与全局映射。
//! WriteCF 依赖 DefaultCF 暂存；缺失时记入 `noDefaultKVErrorMap`，由 ReportIfError 抛出。
//! `ReplaceTemporaryIDs` 把负临时 ID 换成 `genGlobalIDs` 分配的正式 ID；临时 ID 递减生成。
//! 注释说明占位语义与 Go 对齐点；不把桩依赖描述为完整 TiDB meta 栈。
//! MergeBaseDBReplace 三阶段合并保证全量备份 ID 优先于日志解析临时 ID。

//! DefaultCF/WriteCF 配对按 startTs 索引；count 处理同 ts 多版本写入。
//! 过滤器 FilteredOut 在 ToProto/FromDBMapProto 往返中保留。
//! globalIdMap 跨库复用同一上游表 ID 的下游分配，避免分区/表冲突。
//! ParseMetaKvAndUpdateIdMapping 对非 Put write 类型直接 Ok，不污染暂存。

use std::collections::HashMap;

use astersql_br_pkg_utils_consts::{DefaultCF, WriteCF};
use base64::Engine;

use crate::meta_kv::{ParseTxnMetaKeyFrom, RawWriteCFValue};
use crate::stubs::backuppb::{PitrDBMap, PitrTableMap};
use crate::stubs::errors::Error;
use crate::stubs::errors::berrors;
use crate::stubs::model;
use crate::stubs::{
    DBReplace, DownstreamID, NewDBReplace, NewTableReplace, TableReplace, UpstreamID, meta, utils,
};

// 再导出表简要信息，供外部与 Go 侧 TableSimpleInfo 对齐使用。
pub use crate::stubs::TableSimpleInfo;

// 临时下游 ID 计数起点；generateTempID 先减再返回，故首个为 -1。
pub const InitialTempId: i64 = 0;
// WriteCF 有 PUT 但找不到对应 DefaultCF 时的错误前缀，对齐 Go。
const errMsgDefaultCFKVLost: &str = "the default cf kv is lost when there is its write cf kv";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
// DefaultCF 表 meta 暂存键：(db, table, startTs)。
struct tableMetaKey {
    dbId: i64,
    tableId: i64,
    ts: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
// DefaultCF 库 meta 暂存键：(db, startTs)。
struct dbMetaKey {
    dbId: i64,
    ts: u64,
}

// 暂存库名与引用计数；WriteCF 消费时递减。
struct dbMetaValue {
    name: String,
    count: i32,
}

// 暂存表简要信息与引用计数。
struct tableMetaValue {
    info: TableSimpleInfo,
    count: i32,
}

/// 上下游库/表/分区 ID 映射与 meta 解析状态机。
pub struct TableMappingManager {
    // 上游库 ID → 替换结构（含表/分区映射）。
    pub DBReplaceMap: HashMap<UpstreamID, DBReplace>,
    // 是否来自持久化 PITR id map（影响后续合并策略）。
    pub fromPitrIdMap: bool,
    // 全局上游→下游 ID（库/表/分区共用命名空间）。
    pub globalIdMap: HashMap<UpstreamID, DownstreamID>,
    // 下一个临时 ID 的计数器（向负方向递减）。
    pub tempIDCounter: DownstreamID,
    // DefaultCF 表值暂存，供 WriteCF 配对。
    tempDefaultKVTableMap: HashMap<tableMetaKey, tableMetaValue>,
    // DefaultCF 库值暂存。
    tempDefaultKVDbMap: HashMap<dbMetaKey, dbMetaValue>,
    // commitTs → 缺 DefaultCF 错误；可 CleanError 或 ReportIfError。
    pub(crate) noDefaultKVErrorMap: HashMap<u64, Error>,
    // 预分配下游 ID 区间 [start,end)，供外部协调。
    pub PreallocatedRange: [i64; 2],
}

/// 空映射管理器；临时 ID 从 InitialTempId 开始。
pub fn NewTableMappingManager() -> TableMappingManager {
    TableMappingManager {
        DBReplaceMap: HashMap::new(),
        fromPitrIdMap: false,
        globalIdMap: HashMap::new(),
        tempIDCounter: InitialTempId,
        tempDefaultKVTableMap: HashMap::new(),
        tempDefaultKVDbMap: HashMap::new(),
        noDefaultKVErrorMap: HashMap::new(),
        PreallocatedRange: [0, 0],
    }
}

impl TableMappingManager {
    /// 标记映射来自 PITR id map 文件。
    pub fn SetFromPiTRIDMap(&mut self) {
        self.fromPitrIdMap = true;
    }

    /// 是否标记为来自 PITR id map。
    pub fn IsFromPiTRIDMap(&self) -> bool {
        self.fromPitrIdMap
    }

    /// 清空 DefaultCF 暂存（批次边界调用）。
    pub fn CleanTempKV(&mut self) {
        self.tempDefaultKVDbMap.clear();
        self.tempDefaultKVTableMap.clear();
    }

    /// 仅在空管理器上装载已有 DBReplaceMap；非空则报 ErrRestoreInvalidRewrite。
    pub fn FromDBReplaceMap(
        &mut self,
        dbReplaceMap: Option<HashMap<UpstreamID, DBReplace>>,
    ) -> Result<(), Error> {
        // 防止覆盖未清空的映射。
        if !self.IsEmpty() {
            return Err(
                Error::new("expect table mapping manager empty when need to load ID map")
                    .Annotate(berrors::ErrRestoreInvalidRewrite),
            );
        }
        // None 视为空 map。
        self.DBReplaceMap = dbReplaceMap.unwrap_or_default();
        Ok(())
    }

    /// 解析单条 meta KV 并更新 ID 映射；非 meta DB key 直接忽略。
    /// 按 Field 类型分派：DB / Table / AutoIncrement / AutoTable / Sequence / AutoRandom。
    pub fn ParseMetaKvAndUpdateIdMapping(
        &mut self,
        key: &[u8],
        value: &[u8],
        cf: &str,
        ts: u64,
        collector: &mut dyn MetaInfoCollector,
    ) -> Result<(), Error> {
        // 非 mDB 前缀：与流备份无关，跳过。
        if !utils::IsMetaDBKey(key) {
            return Ok(());
        }
        // 解开 txn meta 编码得到 Field/Key/ts。
        let rawKey = ParseTxnMetaKeyFrom(key)?;

        // Field 为 DB:id → 库级 meta。
        if meta::IsDBkey(&rawKey.Field) {
            let dbID = self.parseDBKeyAndUpdateIdMapping(&rawKey.Field)?;
            match cf {
                // 库 DefaultCF：暂存名称待 WriteCF 提交。
                DefaultCF => self.parseDBValueAndUpdateIdMappingForDefaultCf(dbID, value, ts),
                WriteCF => {
                    self.parseDBValueAndUpdateIdMappingForWriteCf(dbID, value, ts, collector)
                }
                // 仅支持 default/write，其它 CF 直接失败。
                _ => Err(Error::new(format!("unsupported column family: {cf}"))),
            }
        // Key 非 DB 前缀则忽略（非表级命名空间）。
        } else if !meta::IsDBkey(&rawKey.Key) {
            Ok(())
        // 表 meta：先确保 table replace，再按 CF 解析值。
        } else if meta::IsTableKey(&rawKey.Field) {
            let dbID = meta::ParseDBKey(&rawKey.Key).map_err(Error::new)?;
            self.parseTableIdAndUpdateIdMapping(&rawKey.Key, &rawKey.Field, meta::ParseTableKey)?;
            match cf {
                // 表 DefaultCF：暂存 TableSimpleInfo。
                DefaultCF => self.parseTableValueAndUpdateIdMappingForDefaultCf(dbID, value, ts),
                WriteCF => {
                    // WriteCF 路径需要显式 tableId 以便配对暂存键。
                    let tableId = meta::ParseTableKey(&rawKey.Field).map_err(Error::new)?;
                    self.parseTableValueAndUpdateIdMappingForWriteCf(
                        dbID, tableId, value, ts, collector,
                    )
                }
                _ => Err(Error::new(format!("unsupported column family: {cf}"))),
            }
        // 自增/序列等仅登记 table ID，不解析值体。
        } else if meta::IsAutoIncrementIDKey(&rawKey.Field) {
            self.parseTableIdAndUpdateIdMapping(
                &rawKey.Key,
                &rawKey.Field,
                meta::ParseAutoIncrementIDKey,
            )
        } else if meta::IsAutoTableIDKey(&rawKey.Field) {
            // AutoTableID：同样只登记 ID。
            self.parseTableIdAndUpdateIdMapping(
                &rawKey.Key,
                &rawKey.Field,
                meta::ParseAutoTableIDKey,
            )
        } else if meta::IsSequenceKey(&rawKey.Field) {
            // Sequence：登记序列所属表 ID。
            self.parseTableIdAndUpdateIdMapping(&rawKey.Key, &rawKey.Field, meta::ParseSequenceKey)
        } else if meta::IsAutoRandomTableIDKey(&rawKey.Field) {
            // AutoRandom：登记表 ID。
            self.parseTableIdAndUpdateIdMapping(
                &rawKey.Key,
                &rawKey.Field,
                meta::ParseAutoRandomTableIDKey,
            )
        // 未识别 field：静默忽略，与 Go 一致。
        } else {
            Ok(())
        }
    }

    // 解析 DB field 并确保存在 DBReplace。
    fn parseDBKeyAndUpdateIdMapping(&mut self, field: &[u8]) -> Result<i64, Error> {
        let dbID = meta::ParseDBKey(field).map_err(Error::new)?;
        self.getOrCreateDBReplace(dbID)?;
        Ok(dbID)
    }

    // DefaultCF：按 (dbId,startTs) 暂存库名；重复 key 增加 count。
    fn parseDBValueAndUpdateIdMappingForDefaultCf(
        &mut self,
        dbId: i64,
        value: &[u8],
        startTs: u64,
    ) -> Result<(), Error> {
        // JSON DBInfo → 原始库名。
        let dbName = model::db_name_from_value(value).map_err(Error::new)?;
        let key = dbMetaKey { dbId, ts: startTs };
        // 同 startTs 重复 DefaultCF：增加引用，避免过早清除。
        if let Some(existingValue) = self.tempDefaultKVDbMap.get_mut(&key) {
            existingValue.count += 1;
            return Ok(());
        }
        self.tempDefaultKVDbMap.insert(
            key,
            dbMetaValue {
                name: dbName,
                count: 1,
            },
        );
        Ok(())
    }

    // WriteCF：Delete/Rollback 清暂存；Put 走 short value 或配对 DefaultCF。
    fn parseDBValueAndUpdateIdMappingForWriteCf(
        &mut self,
        dbId: i64,
        value: &[u8],
        commitTs: u64,
        collector: &mut dyn MetaInfoCollector,
    ) -> Result<(), Error> {
        let mut rawWriteCFValue = RawWriteCFValue::default();
        rawWriteCFValue.ParseFrom(value)?;
        // 事务未提交或删除：丢弃对应 DefaultCF 暂存。
        if rawWriteCFValue.IsDelete() || rawWriteCFValue.IsRollback() {
            let idx = dbMetaKey {
                dbId,
                ts: rawWriteCFValue.GetStartTs(),
            };
            self.tempDefaultKVDbMap.remove(&idx);
            return Ok(());
        }
        // 非 Put（如 Lock）忽略。
        if !rawWriteCFValue.IsPut() {
            return Ok(());
        }
        let startTs = rawWriteCFValue.GetStartTs();
        // short value 内嵌库名，无需 DefaultCF。
        if rawWriteCFValue.HasShortValue() {
            let dbName =
                model::db_name_from_value(&rawWriteCFValue.GetShortValue()).map_err(Error::new)?;
            return self.parseDBValueAndUpdateIdMapping(dbId, dbName, commitTs, collector);
        }
        let idx = dbMetaKey { dbId, ts: startTs };
        // 配对成功：递减 count 并落地名称。
        if let Some(dbValue) = self.tempDefaultKVDbMap.get_mut(&idx) {
            dbValue.count -= 1;
            let name = dbValue.name.clone();
            return self.parseDBValueAndUpdateIdMapping(dbId, name, commitTs, collector);
        }
        // 有 Write 无 Default：延迟报错，允许后续 CleanError。
        self.noDefaultKVErrorMap.insert(
            commitTs,
            Error::new(format!(
                "{}(db id:{dbId}, value {})",
                errMsgDefaultCFKVLost,
                base64::engine::general_purpose::STANDARD.encode(value)
            )),
        );
        Ok(())
    }

    // 落地库名并通知 collector。
    fn parseDBValueAndUpdateIdMapping(
        &mut self,
        dbId: i64,
        dbName: String,
        commitTs: u64,
        collector: &mut dyn MetaInfoCollector,
    ) -> Result<(), Error> {
        let dbReplace = self.getOrCreateDBReplace(dbId)?;
        // 空名不覆盖已有 Name。
        if !dbName.is_empty() {
            dbReplace.Name = dbName.clone();
        }
        collector.OnDatabaseInfo(dbId, dbName, commitTs);
        Ok(())
    }

    // 懒创建 DBReplace，分配临时下游 DbID。
    fn getOrCreateDBReplace(&mut self, dbID: i64) -> Result<&mut DBReplace, Error> {
        if !self.DBReplaceMap.contains_key(&dbID) {
            let newID = self.generateTempID();
            self.globalIdMap.insert(dbID, newID);
            self.DBReplaceMap
                .insert(dbID, NewDBReplace(String::new(), newID));
        }
        // 刚插入或已存在，unwrap 安全。
        Ok(self.DBReplaceMap.get_mut(&dbID).unwrap())
    }

    // 懒创建 TableReplace；表 ID 全局复用已有下游映射。
    fn getOrCreateTableReplace(
        &mut self,
        dbID: i64,
        tableID: i64,
    ) -> Result<&mut TableReplace, Error> {
        self.getOrCreateDBReplace(dbID)?;
        // 表 ID 可能已在其它库上下文分配，复用同一下游 ID。
        let newID = if let Some(id) = self.globalIdMap.get(&tableID) {
            *id
        } else {
            let id = self.generateTempID();
            self.globalIdMap.insert(tableID, id);
            id
        };
        let dbReplace = self.DBReplaceMap.get_mut(&dbID).unwrap();
        if !dbReplace.TableMap.contains_key(&tableID) {
            dbReplace
                .TableMap
                .insert(tableID, NewTableReplace(String::new(), newID));
        }
        // 同上，表项必存在。
        Ok(dbReplace.TableMap.get_mut(&tableID).unwrap())
    }

    // 通用：从 key/field 解析 db+table 并建映射（用于 IID/TID/SID/TARID）。
    fn parseTableIdAndUpdateIdMapping(
        &mut self,
        key: &[u8],
        field: &[u8],
        parseField: fn(&[u8]) -> Result<i64, String>,
    ) -> Result<(), Error> {
        let dbID = meta::ParseDBKey(key).map_err(Error::new)?;
        let tableID = parseField(field).map_err(Error::new)?;
        self.getOrCreateTableReplace(dbID, tableID)?;
        Ok(())
    }

    // DefaultCF 表值：暂存 TableSimpleInfo。
    fn parseTableValueAndUpdateIdMappingForDefaultCf(
        &mut self,
        dbID: i64,
        value: &[u8],
        ts: u64,
    ) -> Result<(), Error> {
        // JSON TableInfo → (id, 简要信息)。
        let (tableId, tableSimpleInfo) =
            model::table_simple_from_value(value).map_err(Error::new)?;
        let key = tableMetaKey {
            dbId: dbID,
            tableId,
            ts,
        };
        // 表侧重复 DefaultCF：同库侧引用计数策略。
        if let Some(existingValue) = self.tempDefaultKVTableMap.get_mut(&key) {
            existingValue.count += 1;
            return Ok(());
        }
        self.tempDefaultKVTableMap.insert(
            key,
            tableMetaValue {
                info: tableSimpleInfo,
                count: 1,
            },
        );
        Ok(())
    }

    // WriteCF 表值：配对逻辑同库侧；short value 优先。
    fn parseTableValueAndUpdateIdMappingForWriteCf(
        &mut self,
        dbId: i64,
        tableId: i64,
        value: &[u8],
        commitTs: u64,
        collector: &mut dyn MetaInfoCollector,
    ) -> Result<(), Error> {
        let mut rawWriteCFValue = RawWriteCFValue::default();
        rawWriteCFValue.ParseFrom(value)?;
        // 表侧 Delete/Rollback：按 startTs 清暂存。
        if rawWriteCFValue.IsDelete() || rawWriteCFValue.IsRollback() {
            self.tempDefaultKVTableMap.remove(&tableMetaKey {
                dbId,
                tableId,
                ts: rawWriteCFValue.GetStartTs(),
            });
            return Ok(());
        }
        if !rawWriteCFValue.IsPut() {
            return Ok(());
        }
        let startTs = rawWriteCFValue.GetStartTs();
        // short value 内嵌表信息；tableId 以 value 为准。
        if rawWriteCFValue.HasShortValue() {
            let (tableIdFromValue, tableSimpleInfo) =
                model::table_simple_from_value(&rawWriteCFValue.GetShortValue())
                    .map_err(Error::new)?;
            return self.parseTableValueAndUpdateIdMapping(
                dbId,
                tableIdFromValue,
                commitTs,
                tableSimpleInfo,
                collector,
            );
        }
        let idx = tableMetaKey {
            dbId,
            tableId,
            ts: startTs,
        };
        // 从 DefaultCF 暂存取回 info。
        if let Some(tableValue) = self.tempDefaultKVTableMap.get_mut(&idx) {
            tableValue.count -= 1;
            let info = tableValue.info.clone();
            return self
                .parseTableValueAndUpdateIdMapping(dbId, tableId, commitTs, info, collector);
        }
        // 表侧缺 DefaultCF：按 commitTs 记录。
        self.noDefaultKVErrorMap.insert(
            commitTs,
            Error::new(format!(
                "{}(db id:{dbId}, table id:{tableId}, value {})",
                errMsgDefaultCFKVLost,
                base64::engine::general_purpose::STANDARD.encode(value)
            )),
        );
        Ok(())
    }

    // 更新表名/分区映射并回调 OnTableInfo。
    // 新分区先收集再写回，避免双重借用不稳定。
    fn parseTableValueAndUpdateIdMapping(
        &mut self,
        dbId: i64,
        tableId: i64,
        commitTs: u64,
        tableSimpleInfo: TableSimpleInfo,
        collector: &mut dyn MetaInfoCollector,
    ) -> Result<(), Error> {
        self.getOrCreateDBReplace(dbId)?;
        self.getOrCreateTableReplace(dbId, tableId)?;
        // 先收集需新建的分区映射，避免在持有可变借用时改 globalIdMap。
        let mut new_partitions = Vec::new();
        for partitionId in &tableSimpleInfo.PartitionIds {
            if !self
                .DBReplaceMap
                .get(&dbId)
                .unwrap()
                .TableMap
                .get(&tableId)
                .unwrap()
                .PartitionMap
                .contains_key(partitionId)
            {
                // 分区 ID 全局去重分配。
                let newID = if let Some(id) = self.globalIdMap.get(partitionId) {
                    *id
                } else {
                    let id = self.generateTempID();
                    self.globalIdMap.insert(*partitionId, id);
                    id
                };
                new_partitions.push((*partitionId, newID));
            }
        }
        let tableReplace = self
            .DBReplaceMap
            .get_mut(&dbId)
            .unwrap()
            .TableMap
            .get_mut(&tableId)
            .unwrap();
        // 非空表名才覆盖。
        if !tableSimpleInfo.Name.is_empty() {
            tableReplace.Name = tableSimpleInfo.Name.clone();
        }
        for (part_id, new_id) in new_partitions {
            tableReplace.PartitionMap.insert(part_id, new_id);
        }
        // 通知历史/过滤等下游收集器。
        collector.OnTableInfo(dbId, tableId, &tableSimpleInfo, commitTs);
        Ok(())
    }

    /// 清除某一 rewriteTs 上记录的缺 DefaultCF 错误。
    pub fn CleanError(&mut self, rewriteTs: u64) {
        self.noDefaultKVErrorMap.remove(&rewriteTs);
    }

    /// 若仍有缺 DefaultCF 错误则 Trace 返回第一条。
    pub fn ReportIfError(&self) -> Result<(), Error> {
        for err in self.noDefaultKVErrorMap.values() {
            return Err(Error::Trace(err.clone()));
        }
        Ok(())
    }

    /// 合并全量备份/已有 id map：先灌 globalIdMap，再回填现有项，最后并入缺失库表。
    pub fn MergeBaseDBReplace(&mut self, baseMap: HashMap<UpstreamID, DBReplace>) {
        // 阶段1：base 的正式下游 ID 写入 globalIdMap。
        for (upstreamID, baseDBReplace) in &baseMap {
            self.globalIdMap.insert(*upstreamID, baseDBReplace.DbID);
            for (tableUpID, baseTableReplace) in &baseDBReplace.TableMap {
                self.globalIdMap
                    .insert(*tableUpID, baseTableReplace.TableID);
                for (partUpID, partDownID) in &baseTableReplace.PartitionMap {
                    self.globalIdMap.insert(*partUpID, *partDownID);
                }
            }
        }

        // 阶段2：用 globalIdMap/base 修正已有项的 ID、名称、Reused。
        for (upDBID, existingDBReplace) in self.DBReplaceMap.iter_mut() {
            if let Some(newID) = self.globalIdMap.get(upDBID) {
                existingDBReplace.DbID = *newID;
            }
            if let Some(baseDBReplace) = baseMap.get(upDBID) {
                // 仅在现有名为空时用 base 名填充。
                if existingDBReplace.Name.is_empty() && !baseDBReplace.Name.is_empty() {
                    existingDBReplace.Name = baseDBReplace.Name.clone();
                }
                // Reused 只升不降。
                if baseDBReplace.Reused {
                    existingDBReplace.Reused = true;
                }
            }
            for (upTableID, existingTableReplace) in existingDBReplace.TableMap.iter_mut() {
                if let Some(newID) = self.globalIdMap.get(upTableID) {
                    existingTableReplace.TableID = *newID;
                }
                if existingTableReplace.Name.is_empty() {
                    if let Some(baseDBReplace) = baseMap.get(upDBID) {
                        if let Some(baseTableReplace) = baseDBReplace.TableMap.get(upTableID) {
                            if !baseTableReplace.Name.is_empty() {
                                existingTableReplace.Name = baseTableReplace.Name.clone();
                            }
                        }
                    }
                }
                for (partUpID, partDownID) in existingTableReplace.PartitionMap.iter_mut() {
                    if let Some(newID) = self.globalIdMap.get(partUpID) {
                        *partDownID = *newID;
                    }
                }
            }
        }

        // 阶段3：base 中尚未出现的库/表/分区并入 DBReplaceMap。
        for (upstreamID, baseDBReplace) in baseMap {
            self.DBReplaceMap
                .entry(upstreamID)
                .and_modify(|existingDBReplace| {
                    for (tableUpID, baseTableReplace) in &baseDBReplace.TableMap {
                        existingDBReplace
                            .TableMap
                            .entry(*tableUpID)
                            .and_modify(|existingTableReplace| {
                                for (partUpID, partDownID) in &baseTableReplace.PartitionMap {
                                    existingTableReplace
                                        .PartitionMap
                                        .insert(*partUpID, *partDownID);
                                }
                            })
                            // 现有库缺该表则整表克隆插入。
                            .or_insert_with(|| baseTableReplace.clone());
                    }
                })
                // 现有映射无此上游库则整库插入。
                .or_insert(baseDBReplace);
        }
    }

    /// DBReplaceMap 是否为空。
    pub fn IsEmpty(&self) -> bool {
        self.DBReplaceMap.is_empty()
    }

    /// Apply the PiTR selection to every database and table mapping.
    ///
    /// Like Go, filtering is monotonic: selected entries are left unchanged,
    /// while entries absent from the tracker are marked filtered out.
    pub fn ApplyFilterToDBReplaceMap(&mut self, tracker: &impl PiTRIdTrackerLookup) {
        for (db_id, db_replace) in &mut self.DBReplaceMap {
            if !tracker.ContainsDB(*db_id) {
                db_replace.FilteredOut = true;
            }
            for (table_id, table_replace) in &mut db_replace.TableMap {
                if !tracker.ContainsDBAndTableId(*db_id, *table_id) {
                    table_replace.FilteredOut = true;
                }
            }
        }
    }

    /// Reuse an existing downstream database ID when its name is present.
    /// Filtered databases and mappings that already own a positive ID are skipped.
    pub fn ReuseExistingDatabaseIDs(&mut self, schemas: &impl DatabaseSchemaLookup) {
        for db_replace in self.DBReplaceMap.values_mut() {
            if db_replace.FilteredOut || db_replace.DbID > 0 || db_replace.Name.is_empty() {
                continue;
            }
            if let Some(db_id) = schemas.SchemaIDByName(&db_replace.Name) {
                db_replace.DbID = db_id;
                db_replace.Reused = true;
            }
        }
    }

    /// 收集负临时 ID，排序后批量换成 genGlobalIDs 结果；重复临时 ID 冲突则失败。
    pub fn ReplaceTemporaryIDs(
        &mut self,
        genGlobalIDs: fn(usize) -> Result<Vec<i64>, Error>,
    ) -> Result<(), Error> {
        let mut usedTempIDs: HashMap<DownstreamID, UpstreamID> = HashMap::new();
        let mut addTempIDIfNeeded = |downID: DownstreamID, upID: UpstreamID| -> Result<(), Error> {
            // 仅负 ID 视为临时；正 ID 已是正式下游。
            if downID < 0 {
                if let Some(prevUpID) = usedTempIDs.get(&downID) {
                    if *prevUpID == upID {
                        return Ok(());
                    }
                    // 同一临时下游被两个上游占用 → 映射损坏。
                    return Err(Error::new(format!(
                        "found duplicate temporary ID {downID}, existing upstream ID: {prevUpID}, new upstream ID: {upID}"
                    ))
                    .Annotate(berrors::ErrRestoreInvalidRewrite));
                }
                usedTempIDs.insert(downID, upID);
            }
            Ok(())
        };
        for (upDBId, dr) in &self.DBReplaceMap {
            // 收集库级临时 ID。
            addTempIDIfNeeded(dr.DbID, *upDBId)?;
            for (upTableID, tr) in &dr.TableMap {
                // 收集表级临时 ID。
                addTempIDIfNeeded(tr.TableID, *upTableID)?;
                // 分区下游也可能是临时 ID。
                for (upPartID, partID) in &tr.PartitionMap {
                    addTempIDIfNeeded(*partID, *upPartID)?;
                }
            }
        }
        // 无需替换。
        if usedTempIDs.is_empty() {
            return Ok(());
        }
        let mut tempIDs: Vec<DownstreamID> = usedTempIDs.keys().copied().collect();
        // 降序保证与 Go 分配顺序一致。
        tempIDs.sort_by(|a, b| b.cmp(a));
        let newIDs = genGlobalIDs(tempIDs.len())?;
        let mut idMapping = HashMap::new();
        // tempID[i] → newIDs[i] 一一对应。
        for (i, tempID) in tempIDs.iter().enumerate() {
            idMapping.insert(*tempID, newIDs[i]);
        }
        // 回写正式 ID 到所有 DB/Table/Partition。
        for dr in self.DBReplaceMap.values_mut() {
            if let Some(newID) = idMapping.get(&dr.DbID) {
                dr.DbID = *newID;
            }
            for tr in dr.TableMap.values_mut() {
                if let Some(newID) = idMapping.get(&tr.TableID) {
                    tr.TableID = *newID;
                }
                for tempPID in tr.PartitionMap.values_mut() {
                    // 分区下游临时 ID 一并替换。
                    if let Some(newID) = idMapping.get(tempPID) {
                        *tempPID = *newID;
                    }
                }
            }
        }
        // 重置临时计数，避免后续继续沿用旧区间。
        self.tempIDCounter = InitialTempId;
        Ok(())
    }

    /// 导出为 PitrDBMap protobuf 友好结构。
    pub fn ToProto(&self) -> Vec<PitrDBMap> {
        let mut dbMaps = Vec::with_capacity(self.DBReplaceMap.len());
        for (dbID, dr) in &self.DBReplaceMap {
            // 逐库展开为 proto；分区列表扁平存放。
            let mut dbm = PitrDBMap {
                Name: dr.Name.clone(),
                IdMap: crate::stubs::backuppb::IDMap {
                    UpstreamId: *dbID,
                    DownstreamId: dr.DbID,
                },
                Tables: Vec::with_capacity(dr.TableMap.len()),
                FilteredOut: dr.FilteredOut,
            };
            for (tblID, tr) in &dr.TableMap {
                let mut tm = PitrTableMap {
                    Name: tr.Name.clone(),
                    IdMap: crate::stubs::backuppb::IDMap {
                        UpstreamId: *tblID,
                        DownstreamId: tr.TableID,
                    },
                    Partitions: Vec::with_capacity(tr.PartitionMap.len()),
                    FilteredOut: tr.FilteredOut,
                };
                // 分区映射序列化。
                for (upID, downID) in &tr.PartitionMap {
                    tm.Partitions.push(crate::stubs::backuppb::IDMap {
                        UpstreamId: *upID,
                        DownstreamId: *downID,
                    });
                }
                // 追加表映射。
                dbm.Tables.push(tm);
            }
            // 追加库映射。
            dbMaps.push(dbm);
        }
        dbMaps
    }

    // 递减生成临时下游 ID（-1,-2,...）。
    fn generateTempID(&mut self) -> DownstreamID {
        self.tempIDCounter -= 1;
        self.tempIDCounter
    }

    /// 记录预分配下游 ID 区间。
    pub fn SetPreallocatedRange(&mut self, start: i64, end: i64) {
        self.PreallocatedRange = [start, end];
    }
}

/// meta 解析侧回调：库/表信息变更时通知历史管理器等。
pub trait MetaInfoCollector {
    fn OnDatabaseInfo(&mut self, dbId: i64, dbName: String, commitTs: u64);
    /// 表（含分区列表）在 commitTs 生效时回调。
    fn OnTableInfo(
        &mut self,
        dbID: i64,
        tableId: i64,
        tableSimpleInfo: &TableSimpleInfo,
        commitTs: u64,
    );
}

/// Adapter for the two membership queries used from Go's `utils.PiTRIdTracker`.
pub trait PiTRIdTrackerLookup {
    fn ContainsDB(&self, db_id: i64) -> bool;
    fn ContainsDBAndTableId(&self, db_id: i64, table_id: i64) -> bool;
}

/// Adapter for Go's `infoschema.InfoSchema.SchemaByName` lookup.
pub trait DatabaseSchemaLookup {
    fn SchemaIDByName(&self, name: &str) -> Option<i64>;
}

/// 从 PitrDBMap 列表还原 DBReplaceMap。
pub fn FromDBMapProto(dbMaps: Vec<PitrDBMap>) -> HashMap<UpstreamID, DBReplace> {
    let mut dbReplaces = HashMap::new();
    for db in dbMaps {
        // 还原 FilteredOut，供恢复过滤。
        let mut dr = NewDBReplace(db.Name.clone(), db.IdMap.DownstreamId);
        dr.FilteredOut = db.FilteredOut;
        for tbl in db.Tables {
            // 表级 FilteredOut 同步。
            let mut tr = NewTableReplace(tbl.Name.clone(), tbl.IdMap.DownstreamId);
            tr.FilteredOut = tbl.FilteredOut;
            // 还原分区上下游对。
            for p in tbl.Partitions {
                tr.PartitionMap.insert(p.UpstreamId, p.DownstreamId);
            }
            // 表上游 ID 作 key。
            dr.TableMap.insert(tbl.IdMap.UpstreamId, tr);
        }
        // 库上游 ID 作 key。
        dbReplaces.insert(db.IdMap.UpstreamId, dr);
    }
    dbReplaces
}
