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

//! RawKV backup matching `br/pkg/task/backup_raw.go`.
//!
//! 本模块实现 RawKV（非事务）备份任务编排，与 Go `backup_raw.go` 对齐。
//! 职责边界：解析 CLI 键范围/CF/压缩选项，驱动 BackupClient 落盘，并写 RawRanges 元数据。
//! 与 TxnKV 差异：`IsRawKv=true`、起止版本固定为 0，且必须携带列族；不做 BackupTS 采集。
//! 调度器移除是可选副作用：备份结束必须调用 restore 闭包，避免集群长期处于降配状态。
//! 存储侧先 `SetStorageAndCheckNotInUse`，保证同一 backend 不会被并发任务踩踏。
//! CipherInfo 随 BackupRequest 下发，加密失败应在客户端侧暴露而非静默落明文。

use std::sync::Arc;

use crate::backup::{
    CompressionConfig, flagCompressionLevel, flagCompressionType, flagKeyspaceName,
    flagRemoveSchedulers, parseCompressionFlags,
};
use crate::common::{Config, GetKeepalive, NewMgr};
use crate::stubs::backuppb::{BackupRequest, RawRange};
use crate::stubs::{
    BackupClient, CollectInt, Error, FlagSet, Glue, KeyRange, MemBackupClient, MetaWriter, Mgr,
    ParseKey, RestoreSchedulers, Result, SetSuccessStatus, StorageOptions, Summary, UnitRange,
    berrors,
};

/// Mirrors Go's deferred scheduler restoration on every exit path.
struct SchedulerRestoreGuard(Option<RestoreSchedulers>);

impl Drop for SchedulerRestoreGuard {
    fn drop(&mut self) {
        if let Some(restore) = self.0.take() {
            // Go logs this best-effort cleanup error and preserves the task result.
            let _ = restore();
        }
    }
}

/// CLI：键编码格式（默认 hex），供 `ParseKey` 解码 start/end。
pub const flagKeyFormat: &str = "format";
/// CLI：TiKV 列族名；RawKV 备份必须指定，默认 `default`。
pub const flagTiKVColumnFamily: &str = "cf";
/// CLI：备份区间起始键（经 format 解码后的字节序比较）。
pub const flagStartKey: &str = "start";
/// CLI：备份区间结束键；两端非空时必须严格大于 start。
pub const flagEndKey: &str = "end";

/// RawKV 备份配置：公共 `Config` + 键范围/CF/压缩/是否摘除调度器。
/// 字段语义与 Go `RawKvConfig` 一致，供 `br backup raw` 与测试注入共用。
#[derive(Clone, Debug, Default)]
pub struct RawKvConfig {
    /// 公共 BR 配置（PD/存储/限速/加密等）。
    pub Config: Config,
    /// 已解码的起始键；空表示不限制下界。
    pub StartKey: Vec<u8>,
    /// 已解码的结束键；空表示不限制上界。
    pub EndKey: Vec<u8>,
    /// 目标列族；写入 meta.RawRanges.Cf。
    pub CF: String,
    /// SST 压缩算法与级别。
    pub CompressionConfig: CompressionConfig,
    /// 为 true 时备份前摘除 PD 调度器。
    pub RemoveSchedulers: bool,
}

/// 注册 Raw 备份专用 flag；`remove-schedulers` 默认隐藏，与 Go 一致。
pub fn DefineRawBackupFlags(flags: &mut FlagSet) {
    flags.DefineString(flagKeyFormat, "hex");
    flags.DefineString(flagTiKVColumnFamily, "default");
    flags.DefineString(flagStartKey, "");
    flags.DefineString(flagEndKey, "");
    flags.DefineString(flagKeyspaceName, "");
    flags.DefineString(flagCompressionType, "zstd");
    flags.DefineBool(flagRemoveSchedulers, false);
    let _ = flags.MarkHidden(flagRemoveSchedulers);
}

impl RawKvConfig {
    /// 解析键范围与 CF，再委托公共 `Config.ParseFromFlags`。
    /// 两端键均非空且 start>=end 时返回 `ErrBackupInvalidRange`，避免空区间或反向区间。
    pub fn ParseFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        let format = flags.GetString(flagKeyFormat)?;
        let start = flags.GetString(flagStartKey)?;
        self.StartKey = ParseKey(&format, &start)?;
        let end = flags.GetString(flagEndKey)?;
        self.EndKey = ParseKey(&format, &end)?;
        // 字节序比较与 Go bytes.Compare 一致：仅在两端都给出时校验。
        if !self.StartKey.is_empty()
            && !self.EndKey.is_empty()
            && self.StartKey.as_slice() >= self.EndKey.as_slice()
        {
            return Err(Error::Annotate(
                berrors::ErrBackupInvalidRange,
                "endKey must be greater than startKey",
            ));
        }
        self.CF = flags.GetString(flagTiKVColumnFamily)?;
        self.Config.ParseFromFlags(flags)?;
        Ok(())
    }

    /// 完整备份配置：在 ParseFromFlags 之上叠加 keyspace、压缩与调度器开关。
    pub fn ParseBackupConfigFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        self.ParseFromFlags(flags)?;
        self.Config.KeyspaceName = flags.GetString(flagKeyspaceName)?;
        self.CompressionConfig = parseCompressionFlags(flags)?;
        self.RemoveSchedulers = flags.GetBool(flagRemoveSchedulers)?;
        self.CompressionConfig.CompressionLevel = flags.GetInt32(flagCompressionLevel)?;
        Ok(())
    }

    /// 填充公共配置默认值（gRPC keepalive、checksum 并发等）。
    pub fn Adjust(&mut self) {
        self.Config.adjust();
    }
}

/// RawKV 备份主流程：校验存储 →（可选）摘调度器 → BackupRanges → 写 meta。
/// `StartVersion`/`EndVersion` 恒为 0，因 RawKV 无 MVCC 备份点；进度条按 region 估算。
pub fn RunBackupRaw(
    g: &dyn Glue,
    cmdName: &str,
    cfg: &mut RawKvConfig,
    mgr: Arc<dyn Mgr>,
    client: &dyn BackupClient,
) -> Result<()> {
    cfg.Adjust();
    Summary(cmdName);

    // S3 Object Lock 检查开启，防止备份写到不可变桶导致后续失败难排查。
    let opts = StorageOptions {
        NoCredentials: cfg.Config.NoCreds,
        SendCredentials: cfg.Config.SendCreds,
        CheckS3ObjectLockOptions: true,
    };
    let backend = client.GetStorageBackend().unwrap_or_default();
    client.SetStorageAndCheckNotInUse(&backend, &opts)?;

    let backupRange = KeyRange {
        StartKey: cfg.StartKey.clone(),
        EndKey: cfg.EndKey.clone(),
    };

    // 摘除 PD 调度器可降低备份期间 balance 干扰；必须在成功/失败路径后 restore。
    let _restore_schedulers = SchedulerRestoreGuard(if cfg.RemoveSchedulers {
        Some(mgr.RemoveSchedulers()?)
    } else {
        None
    });

    let brVersion = g.GetVersion();
    let clusterVersion = mgr.GetClusterVersion()?;
    let approximateRegions = mgr.GetRegionCount(&backupRange.StartKey, &backupRange.EndKey)?;
    CollectInt("backup total regions", approximateRegions as i64);
    let updateCh = g.StartProgress(cmdName, approximateRegions as i64, !cfg.Config.LogProgress);

    let req = BackupRequest {
        ClusterId: client.GetClusterID(),
        StartKey: backupRange.StartKey.clone(),
        EndKey: backupRange.EndKey.clone(),
        StartVersion: 0,
        EndVersion: 0,
        RateLimit: cfg.Config.RateLimit,
        Concurrency: cfg.Config.Concurrency,
        StorageBackend: client.GetStorageBackend(),
        IsRawKv: true,
        Cf: cfg.CF.clone(),
        CompressionType: cfg.CompressionConfig.CompressionType,
        CompressionLevel: cfg.CompressionConfig.CompressionLevel,
        CipherInfo: Some(cfg.Config.CipherInfo.clone()),
    };
    // UnitRange 在 Go 侧用于进度计量；此处保留引用以免优化掉对齐符号。
    let _ = UnitRange;
    client.BackupRanges(std::slice::from_ref(&backupRange), &req)?;
    updateCh.Close();

    // Meta 必须记录 RawRanges+CF，恢复侧据此还原非事务数据范围。
    let metaWriter = MetaWriter::new();
    metaWriter.StartWriteMetasAsync();
    let rawRanges = vec![RawRange {
        StartKey: backupRange.StartKey,
        EndKey: backupRange.EndKey,
        Cf: cfg.CF.clone(),
    }];
    metaWriter.Update(|m| {
        m.StartVersion = req.StartVersion;
        m.EndVersion = req.EndVersion;
        m.IsRawKv = req.IsRawKv;
        m.RawRanges = rawRanges.clone();
        m.ClusterId = req.ClusterId;
        m.ClusterVersion = clusterVersion.clone();
        m.BrVersion = brVersion.clone();
        m.ApiVersion = client.GetApiVersion();
    });
    metaWriter.FinishWriteMetas()?;
    metaWriter.FlushBackupMeta()?;
    g.Record(crate::stubs::BackupDataSize, metaWriter.ArchiveSize());
    SetSuccessStatus(true);

    // 保留 NewMgr/GetKeepalive 符号引用，供 WithDefaults 与 Go 入口对称。
    let _ = (GetKeepalive, NewMgr);
    Ok(())
}

/// 测试/简易入口：用 MemBackupClient + NewMgr 组装依赖后调用 `RunBackupRaw`。
pub fn RunBackupRawWithDefaults(g: &dyn Glue, cmdName: &str, cfg: &mut RawKvConfig) -> Result<()> {
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
        ..Default::default()
    };
    RunBackupRaw(g, cmdName, cfg, mgr, &client)
}
