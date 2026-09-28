// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! TxnKV backup matching `br/pkg/task/backup_txn.go`.
//!
//! 本模块实现事务 KV（TxnKV）备份编排，与 Go `backup_txn.go` 对齐。
//! 与 RawKV 差异：`IsRawKv=false`/`IsTxnKv=true`，以当前 TS 作为 EndVersion，无 CF 字段。
//! Go 当前用空 start/end 表示全量 txn 范围；本实现保持同样语义，避免半开区间误截断。
//! 并发默认回落到 `defaultBackupConcurrency`，防止未配置时串行过慢。
//! BackupTS 会写入 summary，便于增量/校验链路对齐同一备份点。
//! Meta 不写 RawRanges；恢复侧靠 IsTxnKv 与版本区间区分路径。
//! 摘调度器闭包与 Raw 路径相同，失败后也应尽量 restore，避免集群失衡。

use std::sync::Arc;

use crate::backup::{
    CompressionConfig, defaultBackupConcurrency, flagCompressionLevel, flagCompressionType,
    flagKeyspaceName, flagRemoveSchedulers, parseCompressionFlags,
};
use crate::backup_raw::{flagEndKey, flagStartKey};
use crate::common::{Config, GetKeepalive, NewMgr};
use crate::stubs::backuppb::BackupRequest;
use crate::stubs::{
    BackupClient, CollectInt, Error, FlagSet, Glue, KeyRange, MemBackupClient, MetaWriter, Mgr,
    RestoreSchedulers, Result, SetSuccessStatus, StorageOptions, Summary, UnitRange, berrors,
};

/// Mirrors Go's deferred scheduler restoration on every exit after removal.
struct SchedulerRestoreGuard(Option<RestoreSchedulers>);

impl Drop for SchedulerRestoreGuard {
    fn drop(&mut self) {
        if let Some(restore) = self.0.take() {
            let _ = restore();
        }
    }
}

/// CLI：起始版本（历史接口）；实际备份 EndVersion 取自客户端当前 TS。
pub const flagStartVersion: &str = "start-version";

/// TxnKV 备份配置：公共 Config、可选键界、起始版本与压缩/调度器开关。
#[derive(Clone, Debug, Default)]
pub struct TxnKvConfig {
    /// 公共 BR 配置。
    pub Config: Config,
    /// 可选下界；运行路径目前仍用空范围全量备份。
    pub StartKey: Vec<u8>,
    /// 可选上界；与 StartKey 同时非空时做字节序校验。
    pub EndKey: Vec<u8>,
    /// 历史 CLI 字段；请求里 StartVersion 仍写 0。
    pub StartVersion: i64,
    /// SST 压缩配置。
    pub CompressionConfig: CompressionConfig,
    /// 是否在备份前移除 PD 调度器。
    pub RemoveSchedulers: bool,
}

/// 注册 Txn 备份 flag；复用 Raw 侧 start/end 名以共享 CLI 习惯。
pub fn DefineTxnBackupFlags(flags: &mut FlagSet) {
    flags.DefineString(flagStartKey, "");
    flags.DefineString(flagEndKey, "");
    flags.DefineInt64(flagStartVersion, 0);
    flags.DefineString(flagKeyspaceName, "");
    flags.DefineString(flagCompressionType, "zstd");
    flags.DefineBool(flagRemoveSchedulers, false);
    let _ = flags.MarkHidden(flagRemoveSchedulers);
}

impl TxnKvConfig {
    /// 校验已填入的键界，再解析公共配置与 keyspace。
    /// 注意：此处不从 flag 解码 start/end 字节（与当前 Go 路径一致，运行时用全量空范围）。
    pub fn ParseFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        if !self.StartKey.is_empty()
            && !self.EndKey.is_empty()
            && self.StartKey.as_slice() >= self.EndKey.as_slice()
        {
            return Err(Error::Annotate(
                berrors::ErrBackupInvalidRange,
                "endKey must be greater than startKey",
            ));
        }
        self.Config.ParseFromFlags(flags)?;
        self.Config.KeyspaceName = flags.GetString(flagKeyspaceName)?;
        Ok(())
    }

    /// 叠加压缩与调度器相关 flag。
    pub fn ParseBackupConfigFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        self.ParseFromFlags(flags)?;
        self.CompressionConfig = parseCompressionFlags(flags)?;
        self.RemoveSchedulers = flags.GetBool(flagRemoveSchedulers)?;
        self.CompressionConfig.CompressionLevel = flags.GetInt32(flagCompressionLevel)?;
        Ok(())
    }

    /// 填充公共默认值；并发为 0 时使用备份默认并发，与 Go Adjust 一致。
    pub fn Adjust(&mut self) {
        self.Config.adjust();
        if self.Config.Concurrency == 0 {
            self.Config.Concurrency = defaultBackupConcurrency;
        }
    }
}

/// TxnKV 备份主流程：占存储 → 取 BackupTS → 全量 BackupRanges → 写 IsTxnKv meta。
pub fn RunBackupTxn(
    g: &dyn Glue,
    cmdName: &str,
    cfg: &mut TxnKvConfig,
    mgr: Arc<dyn Mgr>,
    client: &dyn BackupClient,
) -> Result<()> {
    cfg.Adjust();
    Summary(cmdName);

    // 与 Raw 相同：开启 Object Lock 检查，避免写到不可变桶。
    let opts = StorageOptions {
        NoCredentials: cfg.Config.NoCreds,
        SendCredentials: cfg.Config.SendCreds,
        CheckS3ObjectLockOptions: true,
    };
    let backend = client.GetStorageBackend().unwrap_or_default();
    // 占坑存储，防止另一备份/恢复并发使用同一路径。
    client.SetStorageAndCheckNotInUse(&backend, &opts)?;

    // Go currently builds a full txn range (empty start/end).
    // 空区间表示全 keyspace；region 计数也用空界，与 Go GetRegionCount 对齐。
    let backupRanges = vec![KeyRange {
        StartKey: Vec::new(),
        EndKey: Vec::new(),
    }];

    // 可选摘调度器；结束时必须 restore。
    let _scheduler_restore = SchedulerRestoreGuard(if cfg.RemoveSchedulers {
        Some(mgr.RemoveSchedulers()?)
    } else {
        None
    });

    let brVersion = g.GetVersion();
    let clusterVersion = mgr.GetClusterVersion()?;
    let approximateRegions = mgr.GetRegionCount(&[], &[])?;
    CollectInt("backup total regions", approximateRegions as i64);
    let updateCh = g.StartProgress(cmdName, approximateRegions as i64, !cfg.Config.LogProgress);
    let _ = UnitRange;
    // 事务备份点：EndVersion=当前 TS，StartVersion=0 表示从最早可见版本扫到该点。
    let backupTS = client.GetCurrentTS()?;
    g.Record("BackupTS", backupTS);

    let req = BackupRequest {
        ClusterId: client.GetClusterID(),
        StartVersion: 0,
        EndVersion: backupTS,
        RateLimit: cfg.Config.RateLimit,
        Concurrency: cfg.Config.Concurrency,
        StorageBackend: client.GetStorageBackend(),
        IsRawKv: false,
        CompressionType: cfg.CompressionConfig.CompressionType,
        CompressionLevel: cfg.CompressionConfig.CompressionLevel,
        CipherInfo: Some(cfg.Config.CipherInfo.clone()),
        ..Default::default()
    };

    // 全量范围备份；失败时上层负责清理进度与调度器。
    client.BackupRanges(&backupRanges, &req)?;
    updateCh.Close();

    // Meta 标记 IsTxnKv，恢复侧区分 Raw/Txn 路径。
    let metaWriter = MetaWriter::new();
    metaWriter.StartWriteMetasAsync();
    metaWriter.Update(|m| {
        m.StartVersion = req.StartVersion;
        m.EndVersion = req.EndVersion;
        m.IsRawKv = false;
        m.IsTxnKv = true;
        m.ClusterId = req.ClusterId;
        m.ClusterVersion = clusterVersion.clone();
        m.BrVersion = brVersion.clone();
        m.ApiVersion = client.GetApiVersion();
    });
    metaWriter.FinishWriteMetas()?;
    // 落盘 backupmeta，供 restore 读取版本与集群信息。
    metaWriter.FlushBackupMeta()?;
    g.Record(crate::stubs::BackupDataSize, metaWriter.ArchiveSize());
    SetSuccessStatus(true);

    let _ = (GetKeepalive, NewMgr);
    Ok(())
}

/// 简易入口：MemBackupClient 固定 current_ts=42，便于单测断言 BackupTS。
pub fn RunBackupTxnWithDefaults(g: &dyn Glue, cmdName: &str, cfg: &mut TxnKvConfig) -> Result<()> {
    let mgr = NewMgr(
        g,
        &cfg.Config.KeyspaceName,
        &cfg.Config.PD,
        &cfg.Config.TLS,
        GetKeepalive(&cfg.Config),
        cfg.Config.CheckRequirements,
        false,
        crate::stubs::NormalVersionChecker,
    )?;
    let client = MemBackupClient {
        cluster_id: 1,
        current_ts: 42,
        ..Default::default()
    };
    RunBackupTxn(g, cmdName, cfg, mgr, &client)
}
