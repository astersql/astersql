// Copyright 2026 AsterSQL.
// Copyright 2022-present PingCAP, Inc.
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

//! 日志恢复中的 meta RawKV 模式重写。
//! 对应 Go `br/pkg/stream/rewrite_meta_rawkv.go`：按上游→下游
//! DB/表/分区 ID 映射改写事务 meta 键与 Default/Write CF 值。
//! `fromPitrIdMap=true` 时缺映射返回 None（跳过）；否则报错。
//! FilteredOut/Reused/系统库等路径静默跳过，避免污染下游集群。
//! WriteCF 在重写键时统一替换为 `RewriteTS`，保证提交序可对齐。
//! 本文件不负责数据面 table 键重写，仅处理 meta 命名空间。
//! 与 log_client 批处理协作：None 表示跳过该 Entry。

use std::collections::{HashMap, HashSet};

use astersql_br_pkg_utils_consts::{DefaultCF, WriteCF};

use crate::meta_kv::{ParseTxnMetaKeyFrom, RawWriteCFValue};
use crate::stubs::errors::Error;
use crate::stubs::errors::berrors;
use crate::stubs::model;
use crate::stubs::{DBReplace, UpstreamID, meta, utils};

// 对外再导出替身类型，调用方不必深入 stubs。
// DownstreamID/TableReplace 等与 Go 类型别名对应。
pub use crate::stubs::{DownstreamID, NewDBReplace, NewTableReplace, TableReplace};

/// 模式级重写器：持有 ID 映射、恢复 TS 与删除追踪。
/// 与 Go `SchemasReplace` 对齐；Rust 侧 ingest/tiflash recorder 仍为桩扩展点。
pub struct SchemasReplace {
    /// 上游 DB ID → 下游替换信息（含表/分区映射）。
    /// 键为上游 ID，值内 DbID/TableID 为下游。
    pub DbReplaceMap: HashMap<UpstreamID, DBReplace>,
    /// 是否来自 PITR id map：缺项时跳过而非失败。
    fromPitrIdMap: bool,
    /// WriteCF 键上使用的重写提交时间戳。
    pub RewriteTS: u64,
    /// 观察到 Delete/Rollback 的表：dbID → tableID 集合。
    /// 供恢复后清理或校验下游残留。
    deletedTables: HashMap<UpstreamID, HashSet<UpstreamID>>,
    /// 表 JSON 重写后的可选钩子（如恢复模式标记）。
    pub AfterTableRewrittenFn: Option<Box<dyn FnMut(bool, &mut model::TableInfo) + Send>>,
    /// 收集 delete-range SQL 的回调（对应 Go delRangeRecorder）。
    record_delete_range: Option<Box<dyn FnMut(PreDelRangeQuery) + Send>>,
    /// 扁平表/分区 ID 映射，供 delete-range 键重映射。
    global_table_id_map: HashMap<UpstreamID, DownstreamID>,
}

/// 值重写内部结果：新字节与是否视为删除类记录。
/// Deleted=true 不代表物理删键，而是 write 类型为删/回滚。
struct rewriteResult {
    NewValue: Vec<u8>,
    Deleted: bool,
    Put: bool,
}

/// 单条 gc_delete_range 插入参数。
/// StartKey/EndKey 通常为 hex 字符串形式的表范围。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DelRangeParams {
    pub JobID: i64,
    pub ElemID: i64,
    pub StartKey: String,
    pub EndKey: String,
}

/// 预生成的 delete-range 批量插入语句与参数列表。
/// Sql 模板固定，ParamsList 按 Job 参数填充。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreDelRangeQuery {
    pub Sql: String,
    pub ParamsList: Vec<DelRangeParams>,
}

/// INSERT IGNORE 前缀，写入 `mysql.gc_delete_range`。
pub const BRInsertDeleteRangeSQLPrefix: &str = "INSERT IGNORE INTO mysql.gc_delete_range VALUES ";
/// 单行占位符模板，与 Go 常量一致。
pub const BRInsertDeleteRangeSQLValue: &str = "(%?, %?, %?, %?, %?)";

/// 无钩子构造；`restoreTS` 写入 `RewriteTS`。
/// 等价于 WithHooks(..., None)。
pub fn NewSchemasReplace(
    dbReplaceMap: HashMap<UpstreamID, DBReplace>,
    fromPitrIdMap: bool,
    restoreTS: u64,
) -> SchemasReplace {
    NewSchemasReplaceWithHooks(dbReplaceMap, fromPitrIdMap, restoreTS, None)
}

/// 可挂 delete-range 记录回调的构造。
/// 同时展开未过滤的表/分区到 `global_table_id_map`。
pub fn NewSchemasReplaceWithHooks(
    dbReplaceMap: HashMap<UpstreamID, DBReplace>,
    fromPitrIdMap: bool,
    restoreTS: u64,
    record_delete_range: Option<Box<dyn FnMut(PreDelRangeQuery) + Send>>,
) -> SchemasReplace {
    let mut global = HashMap::new();
    // 跳过 FilteredOut 的库/表；分区 ID 一并纳入全局映射。
    for dr in dbReplaceMap.values() {
        if dr.FilteredOut {
            continue;
        }
        for (tbl_id, tr) in &dr.TableMap {
            if tr.FilteredOut {
                continue;
            }
            global.insert(*tbl_id, tr.TableID);
            for (pid, nid) in &tr.PartitionMap {
                global.insert(*pid, *nid);
            }
        }
    }
    SchemasReplace {
        DbReplaceMap: dbReplaceMap,
        fromPitrIdMap,
        RewriteTS: restoreTS,
        deletedTables: HashMap::new(),
        AfterTableRewrittenFn: None,
        record_delete_range,
        global_table_id_map: global,
    }
}

impl SchemasReplace {
    /// 公开包装：重写 DB 列表项（Field 为 DBkey）的事务键。
    pub fn RewriteKeyForDB(&self, key: &[u8], cf: &str) -> Result<Option<Vec<u8>>, Error> {
        self.rewriteKeyForDB(key, cf)
    }

    /// 将 Field 中的上游 dbID 换成下游；WriteCF 另改 Ts。
    /// FilteredOut/Reused 返回 None，表示调用方应丢弃该键。
    fn rewriteKeyForDB(&self, key: &[u8], cf: &str) -> Result<Option<Vec<u8>>, Error> {
        let mut rawMetaKey = ParseTxnMetaKeyFrom(key)?;
        // DB 列表项：dbID 在 Field。
        // 与表键（dbID 在 Key）相反，勿混用解析位置。
        let dbID = meta::ParseDBKey(&rawMetaKey.Field).map_err(Error::new)?;
        let dbMap = match self.DbReplaceMap.get(&dbID) {
            Some(v) => v,
            // PITR 映射不完整时跳过未知库。
            None if self.fromPitrIdMap => return Ok(None),
            None => {
                return Err(Error::new(format!("failed to find db id:{dbID} in maps"))
                    .Annotatef(berrors::ErrInvalidArgument));
            }
        };
        if dbMap.FilteredOut || dbMap.Reused {
            return Ok(None);
        }
        rawMetaKey.UpdateField(meta::DBkey(dbMap.DbID));
        // 仅 WriteCF 需要统一恢复 TS；DefaultCF 保留原提交 Ts。
        // 这样 Default/Write 对在恢复点仍可配对。
        if cf == WriteCF {
            rawMetaKey.UpdateTS(self.RewriteTS);
        }
        Ok(Some(rawMetaKey.EncodeMetaKey()))
    }

    /// 重写 DBInfo JSON：替换 ID 字段后重新序列化。
    pub fn rewriteDBInfo(&self, value: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        let mut dbInfo: model::DBInfo =
            serde_json::from_slice(value).map_err(|e| Error::new(format!("{e}")))?;
        let dbMap = match self.DbReplaceMap.get(&dbInfo.ID) {
            Some(v) => v,
            None if self.fromPitrIdMap => return Ok(None),
            None => {
                return Err(
                    Error::new(format!("failed to find db id:{} in maps", dbInfo.ID))
                        .Annotatef(berrors::ErrInvalidArgument),
                );
            }
        };
        if dbMap.FilteredOut || dbMap.Reused {
            return Ok(None);
        }
        dbInfo.ID = dbMap.DbID;
        serde_json::to_vec(&dbInfo)
            .map(Some)
            .map_err(|e| Error::new(format!("{e}")))
    }

    /// 按 CF 分流值重写。
    /// DefaultCF：直接对整值调用 rewriteFunc。
    /// WriteCF：Rollback 原样保留；Delete 标 Deleted；Put 标 Put；
    /// 非 Rollback 记录统一标记物理导入来源，shortValue 按需重写。
    fn rewriteValue(
        value: &[u8],
        cf: &str,
        rewriteFunc: impl FnOnce(&[u8]) -> Result<Option<Vec<u8>>, Error>,
    ) -> Result<rewriteResult, Error> {
        if cf == DefaultCF {
            let newValue = rewriteFunc(value)?;
            // Go 的 nil 结果对应空值；调用方的键过滤独立完成。
            return Ok(rewriteResult {
                NewValue: newValue.unwrap_or_default(),
                Deleted: false,
                Put: false,
            });
        }
        if cf != WriteCF {
            panic!("not support cf:{cf}");
        }
        let mut rawWriteCFValue = RawWriteCFValue::default();
        rawWriteCFValue.ParseFrom(value)?;
        if rawWriteCFValue.IsRollback() {
            return Ok(rewriteResult {
                NewValue: value.to_vec(),
                Deleted: false,
                Put: false,
            });
        }
        rawWriteCFValue.MarkPhysicalImportTxnSource();
        if rawWriteCFValue.IsDelete() {
            return Ok(rewriteResult {
                NewValue: rawWriteCFValue.EncodeTo(),
                Deleted: true,
                Put: false,
            });
        }
        if !rawWriteCFValue.HasShortValue() {
            return Ok(rewriteResult {
                NewValue: rawWriteCFValue.EncodeTo(),
                Deleted: false,
                Put: true,
            });
        }
        let new_value = rewriteFunc(&rawWriteCFValue.GetShortValue())?.unwrap_or_default();
        rawWriteCFValue.UpdateShortValue(new_value);
        Ok(rewriteResult {
            NewValue: rawWriteCFValue.EncodeTo(),
            Deleted: false,
            Put: true,
        })
    }

    /// 入口：识别 meta DB/表键并分发到对应重写路径。
    /// 非 meta 键或未识别形态返回 None。
    pub fn RewriteMetaKvEntry(
        &mut self,
        key: &[u8],
        value: &[u8],
        cf: &str,
    ) -> Result<Option<crate::stubs::kv::Entry>, Error> {
        if !utils::IsMetaDBKey(key) {
            return Ok(None);
        }
        let rawMetaKey = ParseTxnMetaKeyFrom(key)?;
        // Field 为 DBkey → DB 列表项。
        // 对应 meta 中 DBs/{id} 形态。
        if meta::IsDBkey(&rawMetaKey.Field) {
            return self.rewriteEntryForDB(key, value, cf);
        }
        if !meta::IsDBkey(&rawMetaKey.Key) {
            return Ok(None);
        }
        if meta::IsTableKey(&rawMetaKey.Field) {
            self.rewriteEntryForTable(key, value, cf)
        } else if meta::IsAutoIncrementIDKey(&rawMetaKey.Field) {
            self.rewriteEntryForTableScopedKey(
                key,
                value,
                cf,
                meta::ParseAutoIncrementIDKey,
                meta::AutoIncrementIDKey,
            )
        } else if meta::IsAutoTableIDKey(&rawMetaKey.Field) {
            self.rewriteEntryForTableScopedKey(
                key,
                value,
                cf,
                meta::ParseAutoTableIDKey,
                meta::AutoTableIDKey,
            )
        } else if meta::IsSequenceKey(&rawMetaKey.Field) {
            self.rewriteEntryForTableScopedKey(
                key,
                value,
                cf,
                meta::ParseSequenceKey,
                meta::SequenceKey,
            )
        } else if meta::IsAutoRandomTableIDKey(&rawMetaKey.Field) {
            self.rewriteEntryForTableScopedKey(
                key,
                value,
                cf,
                meta::ParseAutoRandomTableIDKey,
                meta::AutoRandomTableIDKey,
            )
        } else {
            Ok(None)
        }
    }

    /// 重写 DB 列表项条目：值走 rewriteDBInfo，键走 rewriteKeyForDB。
    fn rewriteEntryForDB(
        &mut self,
        key: &[u8],
        value: &[u8],
        cf: &str,
    ) -> Result<Option<crate::stubs::kv::Entry>, Error> {
        let dbID = meta::ParseDBKey(&ParseTxnMetaKeyFrom(key)?.Field).map_err(Error::new)?;
        let r = Self::rewriteValue(value, cf, |v| self.rewriteDBInfo(v))?;
        let newKey = self.rewriteKeyForDB(key, cf)?;
        let Some(newKey) = newKey else {
            return Ok(None);
        };
        // DB 级删除只确保 entry 存在，不插入具体 tableID。
        // 与表路径 insert(tableID) 区分。
        if r.Deleted {
            self.deletedTables.entry(dbID).or_default();
        }
        Ok(Some(crate::stubs::kv::Entry {
            Key: newKey,
            Value: r.NewValue,
        }))
    }

    /// 重写表 meta 条目：Field→下游 tableID，值用轻量 rewriteTableInfoInner。
    fn rewriteEntryForTable(
        &mut self,
        key: &[u8],
        value: &[u8],
        cf: &str,
    ) -> Result<Option<crate::stubs::kv::Entry>, Error> {
        let raw_meta_key = ParseTxnMetaKeyFrom(key)?;
        let dbID = meta::ParseDBKey(&raw_meta_key.Key).map_err(Error::new)?;
        let mut old_table_id = 0;
        let mut new_table_id = 0;
        let r = Self::rewriteValue(value, cf, |v| self.rewriteTableInfo(v, dbID))?;
        let newKey = self.rewriteKeyForTable(
            key,
            cf,
            |field| {
                let id = meta::ParseTableKey(field)?;
                old_table_id = id;
                Ok(id)
            },
            |id| {
                new_table_id = id;
                meta::TableKey(id)
            },
        )?;
        let Some(newKey) = newKey else {
            return Ok(None);
        };
        if r.Deleted {
            self.deletedTables
                .entry(dbID)
                .or_default()
                .insert(old_table_id);
            if let Some(cb) = self.AfterTableRewrittenFn.as_mut() {
                cb(
                    true,
                    &mut model::TableInfo {
                        ID: new_table_id,
                        ..Default::default()
                    },
                );
            }
        } else if r.Put {
            if let Some(tables) = self.deletedTables.get_mut(&dbID) {
                tables.remove(&old_table_id);
            }
        }
        Ok(Some(crate::stubs::kv::Entry {
            Key: newKey,
            Value: r.NewValue,
        }))
    }

    fn rewriteEntryForTableScopedKey(
        &self,
        key: &[u8],
        value: &[u8],
        cf: &str,
        parse_field: impl FnMut(&[u8]) -> Result<i64, String>,
        encode_field: impl FnMut(i64) -> Vec<u8>,
    ) -> Result<Option<crate::stubs::kv::Entry>, Error> {
        Ok(self
            .rewriteKeyForTable(key, cf, parse_field, encode_field)?
            .map(|new_key| crate::stubs::kv::Entry {
                Key: new_key,
                Value: value.to_vec(),
            }))
    }

    /// 通用表键重写：Field 解析/编码由调用方注入（表或索引等）。
    /// 系统/临时系统库直接跳过，避免改写内部元数据。
    pub fn rewriteKeyForTable(
        &self,
        key: &[u8],
        cf: &str,
        mut parse_field: impl FnMut(&[u8]) -> Result<i64, String>,
        mut encode_field: impl FnMut(i64) -> Vec<u8>,
    ) -> Result<Option<Vec<u8>>, Error> {
        let mut rawMetaKey = ParseTxnMetaKeyFrom(key)?;
        let dbID = meta::ParseDBKey(&rawMetaKey.Key).map_err(Error::new)?;
        let tableID = parse_field(&rawMetaKey.Field).map_err(Error::new)?;
        let dbReplace = match self.DbReplaceMap.get(&dbID) {
            Some(v) => v,
            None if self.fromPitrIdMap => return Ok(None),
            None => {
                return Err(Error::new(format!("failed to find db id:{dbID} in maps"))
                    .Annotatef(berrors::ErrInvalidArgument));
            }
        };
        if dbReplace.FilteredOut {
            return Ok(None);
        }
        let tableReplace = match dbReplace.TableMap.get(&tableID) {
            Some(v) => v,
            None if self.fromPitrIdMap => return Ok(None),
            None => {
                return Err(
                    Error::new(format!("failed to find table id:{tableID} in maps"))
                        .Annotatef(berrors::ErrInvalidArgument),
                );
            }
        };
        if tableReplace.FilteredOut || utils::IsSysOrTempSysDB(&dbReplace.Name) {
            return Ok(None);
        }
        // Key/Field 双侧替换为下游 ID。
        rawMetaKey.UpdateKey(meta::DBkey(dbReplace.DbID));
        rawMetaKey.UpdateField(encode_field(tableReplace.TableID));
        if cf == WriteCF {
            rawMetaKey.UpdateTS(self.RewriteTS);
        }
        Ok(Some(rawMetaKey.EncodeMetaKey()))
    }

    /// 完整表 JSON 重写：分区缺映射报错；关闭 TTL；触发 AfterTableRewrittenFn。
    /// 与 Go `rewriteTableInfo` 语义对齐，供非流式/测试路径使用。
    pub fn rewriteTableInfo(&mut self, value: &[u8], dbID: i64) -> Result<Option<Vec<u8>>, Error> {
        let mut tableInfo: model::TableInfo =
            serde_json::from_slice(value).map_err(|e| Error::new(format!("{e}")))?;
        let tableID = tableInfo.ID;
        let dbMap = match self.DbReplaceMap.get(&dbID) {
            Some(v) => v,
            None if self.fromPitrIdMap => return Ok(None),
            None => {
                return Err(Error::new(format!("failed to find db id:{dbID} in maps"))
                    .Annotatef(berrors::ErrInvalidArgument));
            }
        };
        if dbMap.FilteredOut {
            return Ok(None);
        }
        let tableMap = match dbMap.TableMap.get(&tableID) {
            Some(v) => v,
            None if self.fromPitrIdMap => return Ok(None),
            None => {
                return Err(
                    Error::new(format!("failed to find table id:{tableID} in maps"))
                        .Annotatef(berrors::ErrInvalidArgument),
                );
            }
        };
        if tableMap.FilteredOut {
            return Ok(None);
        }
        tableInfo.ID = tableMap.TableID;
        if !tableMap.Name.is_empty() {
            tableInfo.Name = model::CIStr {
                O: tableMap.Name.clone(),
                L: tableMap.Name.to_lowercase(),
            };
        }
        // 分区定义必须全部可映射，否则视为映射表损坏。
        // 与 Inner 的“有则改”策略形成对照。
        if let Some(partitions) = tableInfo.Partition.as_mut() {
            for def in &mut partitions.Definitions {
                let new_id = tableMap.PartitionMap.get(&def.ID).copied().ok_or_else(|| {
                    Error::new(format!(
                        "failed to find partition id:{} in replace maps",
                        def.ID
                    ))
                    .Annotatef(berrors::ErrInvalidArgument)
                })?;
                def.ID = new_id;
            }
        }
        // 恢复后禁用 TTL，避免立即触发清理。
        // 用户可在恢复完成后手动再启用。
        if let Some(ttl) = tableInfo.TTLInfo.as_mut() {
            ttl.Enable = false;
        }
        if let Some(cb) = self.AfterTableRewrittenFn.as_mut() {
            // deleted=false：当前为内容重写而非删除通知。
            cb(false, &mut tableInfo);
        }
        serde_json::to_vec(&tableInfo)
            .map(Some)
            .map_err(|e| Error::new(format!("{e}")))
    }

    /// 流式轻量重写：已知 tableID，分区缺项静默跳过。
    /// 不关 TTL、不调钩子，降低热路径开销。
    /// tableID 参数来自键解析，避免再从 JSON 取 ID 不一致。
    fn rewriteTableInfoInner(
        &self,
        value: &[u8],
        dbID: i64,
        tableID: i64,
    ) -> Result<Option<Vec<u8>>, Error> {
        let mut tableInfo: model::TableInfo =
            serde_json::from_slice(value).map_err(|e| Error::new(format!("{e}")))?;
        let dbMap = match self.DbReplaceMap.get(&dbID) {
            Some(v) => v,
            None if self.fromPitrIdMap => return Ok(None),
            None => {
                return Err(Error::new(format!("failed to find db id:{dbID} in maps"))
                    .Annotatef(berrors::ErrInvalidArgument));
            }
        };
        let tableMap = match dbMap.TableMap.get(&tableID) {
            Some(v) => v,
            None if self.fromPitrIdMap => return Ok(None),
            None => {
                return Err(
                    Error::new(format!("failed to find table id:{tableID} in maps"))
                        .Annotatef(berrors::ErrInvalidArgument),
                );
            }
        };
        tableInfo.ID = tableMap.TableID;
        if let Some(partitions) = tableInfo.Partition.as_mut() {
            for def in &mut partitions.Definitions {
                // 缺映射保留原 ID，由后续完整路径或校验兜底。
                if let Some(new_id) = tableMap.PartitionMap.get(&def.ID) {
                    def.ID = *new_id;
                }
            }
        }
        serde_json::to_vec(&tableInfo)
            .map(Some)
            .map_err(|e| Error::new(format!("{e}")))
    }

    /// Test/helper surface for DDL delete-range recording (Go processIngestIndexAndDeleteRangeFromJob).
    /// 仅当 Job.NeedGC 时收集参数；键经 `remap_hex_table_key` 映射到下游 tableID。
    pub fn processIngestIndexAndDeleteRangeFromJob(
        &mut self,
        job: &model::Job,
    ) -> Result<(), Error> {
        if !job.NeedGC {
            return Ok(());
        }
        let mut query = PreDelRangeQuery {
            Sql: format!(
                "{}{}",
                BRInsertDeleteRangeSQLPrefix, BRInsertDeleteRangeSQLValue
            ),
            ParamsList: Vec::new(),
        };
        for arg in &job.DelRangeArgs {
            // 无全局映射时回退上游 ID，保持可插入。
            let new_tid = self
                .global_table_id_map
                .get(&arg.TableID)
                .copied()
                .unwrap_or(arg.TableID);
            query.ParamsList.push(DelRangeParams {
                JobID: job.ID,
                ElemID: arg.ElemID,
                StartKey: remap_hex_table_key(&arg.StartKey, arg.TableID, new_tid),
                EndKey: remap_hex_table_key(&arg.EndKey, arg.TableID, new_tid),
            });
        }
        if let Some(cb) = self.record_delete_range.as_mut() {
            cb(query);
        }
        Ok(())
    }

    /// 返回累计的删除表集合（只读视图）。
    pub fn GetDeletedTables(&self) -> &HashMap<UpstreamID, HashSet<UpstreamID>> {
        &self.deletedTables
    }
}

/// 将 hex 编码的表前缀键从 old_id 重映射到 new_id。
/// 优先按 tablecodec 前缀字节替换；失败则回退字符串 `t{id}` 替换。
fn remap_hex_table_key(key_hex: &str, old_id: i64, new_id: i64) -> String {
    if old_id == new_id {
        return key_hex.to_string();
    }
    if let Ok(mut bytes) = hex::decode(key_hex) {
        let old_prefix = crate::stubs::tablecodec::EncodeTablePrefix(old_id);
        let new_prefix = crate::stubs::tablecodec::EncodeTablePrefix(new_id);
        if bytes.starts_with(&old_prefix) {
            bytes.splice(0..old_prefix.len(), new_prefix);
            return hex::encode(bytes);
        }
    }
    // 非标准 hex 或前缀不匹配时的尽力替换。
    // 先替换 t{id}_ 再替换 t{id}，降低误伤更长数字 ID。
    key_hex
        .replace(&format!("t{old_id}_"), &format!("t{new_id}_"))
        .replace(&format!("t{old_id}"), &format!("t{new_id}"))
}
