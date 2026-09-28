// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! PiTR ID map persist/load, matching `id_map.go`.
//!
//! 本文件为 `LogClient` 扩展 PiTR 表 ID 映射的持久化与加载，对齐 Go `id_map.go`。
//! 映射把上游库表 ID 对应到下游恢复后的 ID，跨重启/断点续传时必须可恢复。
//! 存储优先级：检查点外部存储 → `mysql.tidb_pitr_id_map` 表 → 备份存储回退。
//! 表路径会按是否存在 `restore_id` 列切换新旧 schema，保证旧集群可兼容。
//! 大块元数据按 `PITRIdMapBlockSize` 切段写入/按 `segment_id` 拼接读回。
//! 写路径与读路径必须使用同一套存储选择逻辑，否则断点续传会读到空映射。
//! `saveIDMap` 成功后才推进检查点进度，避免“进度已过、映射未落盘”。
//! 表写入按段 REPLACE，读取按 segment_id 连续拼接，缺段即视为损坏。
//! 外部存储文件名绑定 cluster_id 与 restored_ts，防止跨集群误读。
//! 兼容模式在缺少 restore_id 列时退化为旧主键，语义与 Go 分支一致。
//! 本文件是 LogClient 的方法扩展，不单独持有状态。

use std::sync::Arc;

use crate::client::LogClient;
use crate::stubs::backuppb::{self, BackupMeta, PitrDBMap};
use crate::stubs::checkpoint::{
    CheckpointProgress, InLogRestoreAndIdMapPersisted, LogMetaManagerT,
};
use crate::stubs::glue::SqlArg;
use crate::stubs::kv;
use crate::stubs::log;
use crate::stubs::metautil;
use crate::stubs::restore_misc;
use crate::stubs::storeapi::Storage;
use crate::stubs::stream::TableMappingManager;
use crate::stubs::{Context, Error, Result};

/// Split the pitr_id_map data into 512 KiB chunks.
// 常量值与 Go PITRIdMapBlockSize 保持一致，勿随意调整。
/// 单段 512KiB，避免单行过大冲垮 TiDB / 外部对象存储限制。
pub const PITRIdMapBlockSize: usize = 524_288;

/// 生成外部存储上的 id map 路径：绑定 cluster_id 与 restored_ts，防止跨任务串写。
pub fn PitrIDMapsFilename(clusterID: u64, restoredTS: u64) -> String {
    format!("pitr_id_maps/pitr_id_map.cluster_id:{clusterID}.restored_ts:{restoredTS}")
}

impl LogClient {
    /// 探测目标集群是否已有 `mysql.tidb_pitr_id_map`；无 domain 时视为不存在。
    pub fn pitrIDMapTableExists(&self) -> bool {
        // InfoSchema 探测；dom 为空说明未绑定 TiDB domain，只能走存储回退。
        self.dom
            .as_ref()
            .map(|d| d.InfoSchema().TableExists("mysql", "tidb_pitr_id_map"))
            .unwrap_or(false)
    }

    /// 是否具备 `restore_id` 列：决定 DELETE/REPLACE/SELECT 走新 schema 还是兼容模式。
    pub fn pitrIDMapHasRestoreIDColumn(&self) -> bool {
        // 委托 restore_misc，集中处理 schema 演进探测。
        self.dom
            .as_ref()
            .map(restore_misc::HasRestoreIDColumn)
            .unwrap_or(false)
    }

    /// 仅在启用检查点时返回检查点存储；否则强制走表或备份存储路径。
    pub fn tryGetCheckpointStorage(
        &self,
        logCheckpointMetaManager: &LogMetaManagerT,
    ) -> Option<Arc<dyn Storage>> {
        // useCheckpoint=false 时即使 manager 有存储也不使用，保持开关语义。
        if !self.useCheckpoint {
            return None;
        }
        logCheckpointMetaManager.TryGetStorage()
    }

    /// 按优先级落盘 id map，并在启用检查点时标记 `InLogRestoreAndIdMapPersisted`。
    /// 顺序与 Go 一致：checkpoint storage → 系统表 → 备份 storage。
    pub fn saveIDMap(
        &self,
        ctx: &Context,
        manager: &TableMappingManager,
        logCheckpointMetaManager: &LogMetaManagerT,
    ) -> Result<()> {
        // ToProto 把内存 TableMapping 转成可序列化的 PitrDBMap 列表。
        let dbmaps = manager.ToProto();
        // 优先检查点存储：断点续传场景下映射与进度同仓，便于一致性恢复。
        if let Some(checkpointStorage) = self.tryGetCheckpointStorage(logCheckpointMetaManager) {
            log::Info(
                "checkpoint storage is specified, load pitr id map from the checkpoint storage.",
            );
            self.saveIDMap2Storage(ctx, checkpointStorage.as_ref(), &dbmaps)?;
        } else if self.pitrIDMapTableExists() {
            // 系统表路径：适合无独立检查点仓、但仍有 TiDB 元数据能力的集群。
            self.saveIDMap2Table(ctx, &dbmaps)?;
        } else {
            // 旧集群无系统表时回退到备份存储，避免任务无法继续。
            log::Info(
                "the table mysql.tidb_pitr_id_map does not exist, maybe the cluster version is old.",
            );
            let storage = self
                .storage
                .as_ref()
                .ok_or_else(|| Error::new("storage is nil"))?;
            self.saveIDMap2Storage(ctx, storage.as_ref(), &dbmaps)?;
        }

        // 进度落盘必须在 id map 写成功之后，防止恢复侧误判“映射已就绪”。
        if self.useCheckpoint {
            log::Info("save checkpoint task info with InLogRestoreAndIdMapPersist status");
            logCheckpointMetaManager.SaveCheckpointProgress(
                ctx,
                &CheckpointProgress {
                    Progress: InLogRestoreAndIdMapPersisted,
                },
            )?;
        }
        Ok(())
    }

    /// 将 DbMaps 封进 BackupMeta 后整文件写入外部存储。
    pub fn saveIDMap2Storage(
        &self,
        ctx: &Context,
        storage: &dyn Storage,
        dbMaps: &[PitrDBMap],
    ) -> Result<()> {
        // 文件名含 restoreTS，同一集群多次 PiTR 互不覆盖。
        let clusterID = self.GetClusterID(ctx);
        let metaFileName = PitrIDMapsFilename(clusterID, self.restoreTS);
        // 复用 BackupMeta 容器承载 DbMaps，与 Go marshal 形状对齐。
        let meta = BackupMeta {
            ClusterId: clusterID,
            DbMaps: dbMaps.to_vec(),
            BackupSchemaVersion: backuppb::BackupSchemaVersion,
            ..Default::default()
        };
        let data = meta.Marshal()?;
        storage.WriteFile(ctx, &metaFileName, &data)
    }

    /// 按段 REPLACE 写入系统表；先 DELETE 同键旧行，保证幂等覆盖。
    /// 有 `restore_id` 时键含 restore_id，否则只按 restored_ts + upstream_cluster_id。
    pub fn saveIDMap2Table(&self, ctx: &Context, dbMaps: &[PitrDBMap]) -> Result<()> {
        let backupmeta = BackupMeta {
            BackupSchemaVersion: backuppb::BackupSchemaVersion,
            DbMaps: dbMaps.to_vec(),
            ..Default::default()
        };
        // 先整包序列化，再按块切段，保证各段拼接后仍是合法 BackupMeta。
        let data = backupmeta.Marshal()?;
        let session = self
            .unsafeSession
            .as_ref()
            .ok_or_else(|| Error::new("unsafeSession is nil"))?;
        // 列探测结果决定 DELETE/REPLACE 的列清单。
        let hasRestoreIDColumn = self.pitrIDMapHasRestoreIDColumn();

        if hasRestoreIDColumn {
            // 新 schema：同一次恢复任务用 restore_id 隔离，避免并发还原互相覆盖。
            session.ExecuteInternal(
                ctx,
                "DELETE FROM mysql.tidb_pitr_id_map WHERE restored_ts = %? and upstream_cluster_id = %? and restore_id = %?;",
                &[
                    SqlArg::U64(self.restoreTS),
                    SqlArg::U64(self.upstreamClusterID),
                    SqlArg::U64(self.restoreID),
                ],
            )?;
            // REPLACE 幂等：同键重跑任务时覆盖旧段。
            let replaceSQL = "REPLACE INTO mysql.tidb_pitr_id_map (restore_id, restored_ts, upstream_cluster_id, segment_id, id_map) VALUES (%?, %?, %?, %?, %?);";
            let mut startIdx = 0usize;
            let mut segmentId = 0u64;
            // 按固定块大小切片；最后一段可短于 BlockSize。
            while startIdx < data.len() {
                let endIdx = (startIdx + PITRIdMapBlockSize).min(data.len());
                session.ExecuteInternal(
                    ctx,
                    replaceSQL,
                    &[
                        SqlArg::U64(self.restoreID),
                        SqlArg::U64(self.restoreTS),
                        SqlArg::U64(self.upstreamClusterID),
                        SqlArg::U64(segmentId),
                        SqlArg::Bytes(data[startIdx..endIdx].to_vec()),
                    ],
                )?;
                startIdx = endIdx;
                segmentId += 1;
            }
        } else {
            // 兼容旧表：缺少 restore_id，同一 restored_ts+cluster 只能有一份映射。
            log::Info(
                "mysql.tidb_pitr_id_map table does not have restore_id column, using backward compatible mode",
            );
            session.ExecuteInternal(
                ctx,
                "DELETE FROM mysql.tidb_pitr_id_map WHERE restored_ts = %? and upstream_cluster_id = %?;",
                &[
                    SqlArg::U64(self.restoreTS),
                    SqlArg::U64(self.upstreamClusterID),
                ],
            )?;
            // 旧表无 restore_id：多任务并发同一 restored_ts 会互相覆盖，与 Go 限制相同。
            let replaceSQL = "REPLACE INTO mysql.tidb_pitr_id_map (restored_ts, upstream_cluster_id, segment_id, id_map) VALUES (%?, %?, %?, %?);";
            let mut startIdx = 0usize;
            let mut segmentId = 0u64;
            // 切段逻辑与新 schema 分支相同，仅列集合不同。
            while startIdx < data.len() {
                let endIdx = (startIdx + PITRIdMapBlockSize).min(data.len());
                session.ExecuteInternal(
                    ctx,
                    replaceSQL,
                    &[
                        SqlArg::U64(self.restoreTS),
                        SqlArg::U64(self.upstreamClusterID),
                        SqlArg::U64(segmentId),
                        SqlArg::Bytes(data[startIdx..endIdx].to_vec()),
                    ],
                )?;
                startIdx = endIdx;
                segmentId += 1;
            }
        }
        Ok(())
    }

    /// 加载路径与 save 对称：checkpoint → 表 → 备份存储。
    pub fn loadSchemasMap(
        &self,
        ctx: &Context,
        restoredTS: u64,
        logCheckpointMetaManager: &LogMetaManagerT,
    ) -> Result<Vec<PitrDBMap>> {
        // 读优先级与 saveIDMap 完全对称，避免写到 A 却从 B 读。
        if let Some(checkpointStorage) = self.tryGetCheckpointStorage(logCheckpointMetaManager) {
            log::Info(
                "checkpoint storage is specified, load pitr id map from the checkpoint storage.",
            );
            return self.loadSchemasMapFromStorage(ctx, checkpointStorage.as_ref(), restoredTS);
        }
        if self.pitrIDMapTableExists() {
            // 表存在即优先表，即使备份存储也有同名文件。
            return self.loadSchemasMapFromTable(ctx, restoredTS);
        }
        log::Info(
            "the table mysql.tidb_pitr_id_map does not exist, maybe the cluster version is old.",
        );
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| Error::new("storage is nil"))?;
        self.loadSchemasMapFromStorage(ctx, storage.as_ref(), restoredTS)
    }

    /// 反序列化 BackupMeta；若为 stub 的 `BM` 简易格式则手工还原 DbMaps。
    /// `checkRequirements` 为真时，兼容性检查失败直接返回错误。
    pub fn loadPITRIDMapBackupMeta(&self, metaData: &[u8]) -> Result<BackupMeta> {
        let mut backupMeta = BackupMeta::default();
        // 先走通用 Unmarshal；stub 简易格式可能只填部分字段。
        backupMeta.Unmarshal(metaData)?;
        // Re-hydrate DbMaps from our simple marshal format.
        // Rust stub 的简易编码以 "BM" 开头，需按长度前缀解析库名列表。
        if metaData.starts_with(b"BM") {
            if metaData.len() < 14 {
                return Err(Error::new("invalid truncated pitr id map metadata header"));
            }
            // 布局：magic(2) + cluster(8) + count(4) + 重复(name_len + name)。
            let cluster = u64::from_le_bytes(metaData[2..10].try_into().unwrap());
            let n = u32::from_le_bytes(metaData[10..14].try_into().unwrap()) as usize;
            let mut off = 14usize;
            let mut maps = Vec::new();
            // 与 Go protobuf.Unmarshal 一致，截断元数据必须失败而不能返回部分映射。
            for _ in 0..n {
                if off + 4 > metaData.len() {
                    return Err(Error::new("invalid truncated pitr id map name length"));
                }
                let nl = u32::from_le_bytes(metaData[off..off + 4].try_into().unwrap()) as usize;
                off += 4;
                let name_end = off
                    .checked_add(nl)
                    .ok_or_else(|| Error::new("invalid overflowing pitr id map name length"))?;
                if name_end > metaData.len() {
                    return Err(Error::new("invalid truncated pitr id map name"));
                }
                let name = String::from_utf8_lossy(&metaData[off..name_end]).into_owned();
                off = name_end;
                maps.push(PitrDBMap {
                    Name: name,
                    ..Default::default()
                });
            }
            backupMeta.ClusterId = cluster;
            backupMeta.DbMaps = maps;
        }
        if let Err(err) = metautil::CheckBackupMetaCompatibilityFromBytes(metaData, &backupMeta) {
            if self.checkRequirements {
                return Err(Error::Trace(err));
            }
            // 非严格模式仅告警，便于旧备份继续加载。
            log::Warn("skip backupmeta compatibility check error");
        }
        Ok(backupMeta)
    }

    /// 从外部存储读整文件；文件不存在返回空映射而非错误。
    pub fn loadSchemasMapFromStorage(
        &self,
        ctx: &Context,
        storage: &dyn Storage,
        restoredTS: u64,
    ) -> Result<Vec<PitrDBMap>> {
        let clusterID = self.GetClusterID(ctx);
        let metaFileName = PitrIDMapsFilename(clusterID, restoredTS);
        // FileExists 失败与“文件不存在”区分：前者是存储故障，需上抛。
        let exist = storage.FileExists(ctx, &metaFileName).map_err(|err| {
            Error::Annotatef(err, format!("failed to check filename:{metaFileName} "))
        })?;
        if !exist {
            // 首次还原或映射尚未写出时返回空列表，由调用方决定是否新建。
            log::Info("pitr id map does not exist");
            return Ok(Vec::new());
        }
        // 读全量字节后再统一走 loadPITRIDMapBackupMeta。
        let metaData = storage.ReadFile(ctx, &metaFileName)?;
        let backupMeta = self.loadPITRIDMapBackupMeta(&metaData)?;
        Ok(backupMeta.GetDbMaps())
    }

    /// 按 segment_id 有序读出各段并拼接；缺段或空段视为损坏并报错。
    pub fn loadSchemasMapFromTable(
        &self,
        ctx: &Context,
        restoredTS: u64,
    ) -> Result<Vec<PitrDBMap>> {
        let hasRestoreIDColumn = self.pitrIDMapHasRestoreIDColumn();
        // SQL 条件与 save 路径对称，保证读写同一逻辑键。
        let (sql, args): (&str, Vec<SqlArg>) = if hasRestoreIDColumn {
            (
                "SELECT segment_id, id_map FROM mysql.tidb_pitr_id_map WHERE restore_id = %? and restored_ts = %? and upstream_cluster_id = %? ORDER BY segment_id;",
                vec![
                    SqlArg::U64(self.restoreID),
                    SqlArg::U64(restoredTS),
                    SqlArg::U64(self.upstreamClusterID),
                ],
            )
        } else {
            log::Info(
                "mysql.tidb_pitr_id_map table does not have restore_id column, using backward compatible mode",
            );
            (
                "SELECT segment_id, id_map FROM mysql.tidb_pitr_id_map WHERE restored_ts = %? and upstream_cluster_id = %? ORDER BY segment_id;",
                vec![SqlArg::U64(restoredTS), SqlArg::U64(self.upstreamClusterID)],
            )
        };
        let session = self
            .unsafeSession
            .as_ref()
            .ok_or_else(|| Error::new("unsafeSession is nil"))?;
        let execCtx = session.GetSessionCtx().GetRestrictedSQLExecutor();
        // 标记 InternalTxnBR，避免内部查询干扰用户事务统计。
        let rows = execCtx
            .ExecRestrictedSQL(
                &kv::WithInternalSourceType(ctx.clone(), kv::InternalTxnBR),
                None,
                sql,
                &args,
            )
            .map_err(|err| {
                Error::Annotatef(err, "failed to get pitr id map from mysql.tidb_pitr_id_map")
            })?;
        if rows.is_empty() {
            // 与存储路径一致：空结果不是错误。
            log::Info("pitr id map does not exist");
            return Ok(Vec::new());
        }
        // 预分配近似容量，减少段拼接时的反复扩容。
        let mut metaData = Vec::with_capacity(rows.len() * PITRIdMapBlockSize);
        for (i, row) in rows.iter().enumerate() {
            // segment_id 必须与行序 0..n-1 连续，否则说明中间段丢失。
            let elementID = row.GetUint64(0);
            if i as u64 != elementID {
                return Err(Error::Errorf(format!(
                    "the part(segment_id = {i}) of pitr id map is lost"
                )));
            }
            let d = row.GetBytes(1);
            // 空段无法拼接出合法 meta，直接失败比静默跳过更安全。
            if d.is_empty() {
                return Err(Error::Errorf(format!(
                    "get the empty part(segment_id = {i}) of pitr id map"
                )));
            }
            metaData.extend_from_slice(&d);
        }
        // 拼接完成后与文件路径共用同一反序列化入口。
        let backupMeta = self.loadPITRIDMapBackupMeta(&metaData)?;
        Ok(backupMeta.GetDbMaps())
    }
}
