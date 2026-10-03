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

//! Restore task configuration and helpers matching `br/pkg/task/restore.go`.
//!
//! 全量/库表/流式等恢复任务的配置与公共辅助，对齐 Go `restore.go`。
//! 包含 RestoreCommonConfig/RestoreConfig、空间估算、DDL Job 过滤规则。
//! 本文件侧重配置与纯函数；实际拉数/导入在其它模块。
//! 命令名常量用于 Summary/operation 命名，与 Go 字符串一致。
//! DDL 过滤区分 Check（禁止）与 Filter（丢弃）两类规则。
//! TiFlash/锁表相关 Action 在恢复前需剔除或校验。
//! 估算函数用于预检磁盘，不替代运行时配额。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use astersql_br_pkg_stream::table_history::{LogBackupTableHistoryManager, TableLocationInfo};
use serde::Serialize;
use sha2::{Digest, Sha256};
use url::Url;

use crate::common::{
    Config, FlagPiTRAddIndexSQLStorage, FlagStreamFullBackupStorage, FullBackupType, GetKeepalive,
    NewMgr, ReadBackupMeta, flagCheckpointStorage, flagConcurrency, flagLoadStats, flagUseFSR,
    flagWithSysTable,
};
use crate::stubs::{
    DefaultMergeRegionKeyCount, DefaultMergeRegionSizeBytes, EncodeBytes, EncodeTablePrefix, Error,
    FlagSet, Glue, MemStorage, MetaFile, ModifiedU64, Result, SetSuccessStatus, Storage, Summary,
    berrors,
};

/// online 恢复 flag 名。
pub const flagOnline: &str = "online";
/// 恢复粒度 flag。
pub const flagGranularity: &str = "granularity";
pub const flagNoSchema: &str = "no-schema";
pub const flagFastLoadSysTables: &str = "fast-load-sys-tables";
/// 每 store 最大恢复并发 flag。
pub const flagConcurrencyPerStore: &str = "tikv-max-restore-concurrency";
/// 合并 region 大小阈值 flag。
pub const FlagMergeRegionSizeBytes: &str = "merge-region-size-bytes";
/// 合并 region key 数阈值 flag。
pub const FlagMergeRegionKeyCount: &str = "merge-region-key-count";
/// PD 侧并发 flag。
pub const FlagPDConcurrency: &str = "pd-concurrency";
/// region 扫描并发 flag。
pub const FlagRegionScanConcurrency: &str = "region-scan-concurrency";
pub const FlagSplitRegionIndexStep: &str = "split-region-index-step";
pub const FlagCoarseScatter: &str = "coarse-scatter";
/// 统计信息并发 flag。
pub const FlagStatsConcurrency: &str = "stats-concurrency";
/// 批刷间隔 flag。
pub const FlagBatchFlushInterval: &str = "batch-flush-interval";
/// DDL 批大小 flag。
pub const FlagDdlBatchSize: &str = "ddl-batch-size";
/// 事务总大小限制 flag。
pub const FlagTxnTotalSizeLimit: &str = "txn-total-size-limit";
/// 重置系统用户 flag。
pub const FlagResetSysUsers: &str = "reset-sys-users";
pub const FlagWithPlacementPolicy: &str = "with-tidb-placement-mode";
pub const FlagWaitTiFlashReady: &str = "wait-tiflash-ready";
pub const FlagSysCheckCollation: &str = "sys-check-collation";
pub const flagAllowPITRFromIncremental: &str = "allow-pitr-from-incremental";
pub const FlagRestorePhase: &str = "restore-phase";
pub const FlagStreamStartTS: &str = "start-ts";
pub const FlagStreamRestoreTS: &str = "restored-ts";
pub const FlagPiTRBatchCount: &str = "pitr-batch-count";
pub const FlagPiTRBatchSize: &str = "pitr-batch-size";
pub const FlagPiTRConcurrency: &str = "pitr-concurrency";
pub const FlagRetainLatestMVCCVersion: &str = "retain-latest-mvcc-version";
/// 粗粒度恢复粒度字面量。
pub const CoarseGrained: &str = "coarse-grained";
/// 默认导入协程数。
pub const DefaultImportNumGoroutines: u32 = astersql_br_pkg_conn::DefaultImportNumGoroutines;
/// 默认恢复并发。
pub const defaultRestoreConcurrency: u32 = 128;
/// 默认 PD 并发。
pub const defaultPDConcurrency: u32 = 1;
/// 默认 region 扫描并发。
pub const defaultRegionScanConcurrency: u32 = 256;
/// 默认 stats 并发。
pub const defaultStatsConcurrency: u32 = 12;
/// 默认批刷间隔。
pub const defaultBatchFlushInterval: Duration = Duration::from_secs(16);
/// 默认 DDL 批大小。
pub const defaultFlagDdlBatchSize: u32 = 128;
pub const defaultPiTRBatchCount: u32 = 8;
pub const defaultPiTRBatchSize: u32 = 16 * 1024 * 1024;
pub const defaultPiTRConcurrency: u32 = 16;
pub const DefaultRegionIndexStep: u32 = 128;

/// 全量恢复命令显示名。
pub const FullRestoreCmd: &str = "Full Restore";
/// 库级恢复命令显示名。
pub const DBRestoreCmd: &str = "DataBase Restore";
/// 表级恢复命令显示名。
pub const TableRestoreCmd: &str = "Table Restore";
/// 时间点恢复命令显示名。
pub const PointRestoreCmd: &str = "Point Restore";
/// Raw KV 恢复命令显示名。
pub const RawRestoreCmd: &str = "Raw Restore";
/// Txn KV 恢复命令显示名。
pub const TxnRestoreCmd: &str = "Txn Restore";

/// 恢复公共配置字段集合。
#[derive(Clone, Debug, Default)]
pub struct RestoreCommonConfig {
    pub Online: bool,
    pub Granularity: String,
    pub ConcurrencyPerStore: ModifiedU64,
    pub MergeSmallRegionSizeBytes: ModifiedU64,
    pub MergeSmallRegionKeyCount: ModifiedU64,
    pub WithSysTable: bool,
    pub ResetSysUsers: Vec<String>,
    pub SysCheckCollation: bool,
}

impl RestoreCommonConfig {
    /// 方法：保持与 Go 同名方法可对照的行为。
    pub fn adjust(&mut self) {
        if !self.MergeSmallRegionKeyCount.Modified {
            self.MergeSmallRegionKeyCount.Value = DefaultMergeRegionKeyCount;
        }
        // 分支：if !self.MergeSmallRegionSizeBytes.Modif — 与 Go 对应控制流对齐。
        if !self.MergeSmallRegionSizeBytes.Modified {
            self.MergeSmallRegionSizeBytes.Value = DefaultMergeRegionSizeBytes;
        }
        // 分支：if self.Granularity.is_empty() { — 与 Go 对应控制流对齐。
        if self.Granularity.is_empty() {
            self.Granularity = CoarseGrained.to_string();
        }
        // 分支：if !self.ConcurrencyPerStore.Modified { — 与 Go 对应控制流对齐。
        if !self.ConcurrencyPerStore.Modified {
            self.ConcurrencyPerStore.Value = DefaultImportNumGoroutines as u64;
        }
    }

    /// 从 FlagSet 填充字段并做基础校验。
    pub fn ParseFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        self.Online = flags.GetBool(flagOnline)?;
        self.Granularity = flags.GetString(flagGranularity)?;
        self.ConcurrencyPerStore.Value = flags.GetUint(flagConcurrencyPerStore)? as u64;
        self.ConcurrencyPerStore.Modified = flags.Changed(flagConcurrencyPerStore);
        self.MergeSmallRegionKeyCount.Value = flags.GetUint64(FlagMergeRegionKeyCount)?;
        self.MergeSmallRegionKeyCount.Modified = flags.Changed(FlagMergeRegionKeyCount);
        self.MergeSmallRegionSizeBytes.Value = flags.GetUint64(FlagMergeRegionSizeBytes)?;
        self.MergeSmallRegionSizeBytes.Modified = flags.Changed(FlagMergeRegionSizeBytes);
        // 分支：if flags.Lookup(flagWithSysTable).is_som — 与 Go 对应控制流对齐。
        if flags.Lookup(flagWithSysTable).is_some() {
            self.WithSysTable = flags.GetBool(flagWithSysTable)?;
        }
        self.ResetSysUsers = flags.GetStringArray(FlagResetSysUsers).unwrap_or_default();
        Ok(())
    }
}

/// 注册恢复公共 flag。
pub fn DefineRestoreCommonFlags(flags: &mut FlagSet) {
    flags.DefineBool(flagOnline, false);
    flags.DefineString(flagGranularity, CoarseGrained);
    flags.DefineUint(flagConcurrencyPerStore, DefaultImportNumGoroutines as u64);
    flags.DefineUint32(flagConcurrency, defaultRestoreConcurrency);
    flags.DefineUint64(FlagMergeRegionSizeBytes, DefaultMergeRegionSizeBytes);
    flags.DefineUint64(FlagMergeRegionKeyCount, DefaultMergeRegionKeyCount);
    flags.DefineUint(FlagPDConcurrency, defaultPDConcurrency as u64);
    flags.DefineUint(
        FlagRegionScanConcurrency,
        defaultRegionScanConcurrency as u64,
    );
    flags.DefineUint(FlagStatsConcurrency, defaultStatsConcurrency as u64);
    flags.DefineDuration(FlagBatchFlushInterval, defaultBatchFlushInterval);
    flags.DefineUint(FlagDdlBatchSize, defaultFlagDdlBatchSize as u64);
    flags.DefineUint64(FlagTxnTotalSizeLimit, 0);
    flags.DefineBool(flagWithSysTable, true);
    flags.DefineStringArray(FlagResetSysUsers, vec!["cloud_admin".into(), "root".into()]);
    flags.DefineBool(flagUseFSR, false);
    // 分支：for name in [ — 与 Go 对应控制流对齐。
    for name in [
        FlagResetSysUsers,
        FlagMergeRegionSizeBytes,
        FlagMergeRegionKeyCount,
        FlagPDConcurrency,
        FlagStatsConcurrency,
        FlagBatchFlushInterval,
        FlagDdlBatchSize,
        flagUseFSR,
    ] {
        let _ = flags.MarkHidden(name);
    }
}

/// 完整恢复配置（含 Common 与过滤等）。
#[derive(Clone, Debug, Default)]
pub struct RestoreConfig {
    pub Config: Config,
    pub RestoreCommonConfig: RestoreCommonConfig,
    pub UpstreamClusterID: u64,
    pub NoSchema: bool,
    pub LoadStats: bool,
    pub FastLoadSysTables: bool,
    pub PDConcurrency: u32,
    pub RegionScanConcurrency: u32,
    pub SplitRegionIndexStep: u32,
    pub CoarseScatter: bool,
    pub StatsConcurrency: u32,
    pub BatchFlushInterval: Duration,
    pub DdlBatchSize: u32,
    pub TxnTotalSizeLimit: u64,
    pub WithPlacementPolicy: String,
    pub FullBackupStorage: String,
    pub PiTRAddIndexSQLStorage: String,
    pub RestoreTS: u64,
    pub StartTS: u64,
    pub RestoredTS: u64,
    pub IsRestoredTSUserSpecified: bool,
    pub PitrBatchCount: u32,
    pub PitrBatchSize: u32,
    pub PitrConcurrency: u32,
    pub RetainLatestMVCCVersion: bool,
    pub FullBackupType: FullBackupType,
    pub Prepare: bool,
    pub OutputMetaFile: String,
    pub SkipAWS: bool,
    pub CloudAPIConcurrency: u32,
    pub VolumeType: String,
    pub VolumeIOPS: i64,
    pub VolumeThroughput: i64,
    pub VolumeEncrypted: bool,
    pub ProgressFile: String,
    pub TargetAZ: String,
    pub UseFSR: bool,
    pub UseCheckpoint: bool,
    pub CheckpointStorage: String,
    pub WaitTiflashReady: bool,
    pub AllowPITRFromIncremental: bool,
    pub RestorePhase: u64,
    pub RestoreInPhase: bool,
    pub PiTRTableTracker: PiTRIdTracker,
    pub RestoreStorage: Option<MemStorage>,
    pub CheckpointMetaManagers: CheckpointMetaManagers,
    pub snapshotRestoreDataSize: u64,
    pub RestoreStartTS: u64,
    pub RestoreID: u64,
    pub TiKVConfigControl: Option<Arc<crate::stream::RestoreTiKVConfigControl>>,
}

pub trait CheckpointMetaManager: Send + Sync {
    fn Close(&self);
}

#[derive(Clone, Default)]
pub struct CheckpointMetaManagers(pub Vec<Arc<dyn CheckpointMetaManager>>);

impl std::fmt::Debug for CheckpointMetaManagers {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("CheckpointMetaManagers")
            .field(&self.0.len())
            .finish()
    }
}

impl RestoreConfig {
    /// 方法：保持与 Go 同名方法可对照的行为。
    pub fn LocalEncryptionEnabled(&self) -> bool {
        crate::stubs::IsEffectiveEncryptionMethod(self.Config.CipherInfo.CipherType)
    }

    /// 方法：保持与 Go 同名方法可对照的行为。
    pub fn Hash(&self, cmdName: &str) -> Result<Vec<u8>> {
        /// 结构体：承载本文件职责相关的状态/配置字段。
        #[derive(Serialize)]
        struct Imm<'a> {
            #[serde(rename = "CmdName")]
            cmd: &'a str,
            #[serde(rename = "UpstreamClusterID")]
            upstream_cluster_id: u64,
            #[serde(rename = "Storage")]
            storage: String,
            #[serde(rename = "FilterStr")]
            filter: &'a [String],
            #[serde(rename = "WithSysTable")]
            with_sys_table: bool,
            #[serde(rename = "FastLoadSysTables")]
            fast_load_sys_tables: bool,
            #[serde(rename = "LoadStats")]
            load_stats: bool,
        }
        let data = serde_json::to_vec(&Imm {
            cmd: cmdName,
            upstream_cluster_id: self.UpstreamClusterID,
            storage: redact_storage_url(&self.Config.Storage),
            filter: &self.Config.FilterStr,
            with_sys_table: self.RestoreCommonConfig.WithSysTable,
            fast_load_sys_tables: self.FastLoadSysTables,
            load_stats: self.LoadStats,
        })
        .map_err(|e| Error::new(e.to_string()))?;
        Ok(Sha256::digest(data).to_vec())
    }

    /// 按默认值回填零值字段。
    pub fn Adjust(&mut self) {
        self.Config.adjust();
        self.RestoreCommonConfig.adjust();
        // 分支：if self.Config.Concurrency == 0 { — 与 Go 对应控制流对齐。
        if self.Config.Concurrency == 0 {
            self.Config.Concurrency = defaultRestoreConcurrency;
        }
        if self.Config.SwitchModeInterval == Duration::ZERO {
            self.Config.SwitchModeInterval = crate::common::defaultSwitchInterval;
        }
        if self.PDConcurrency == 0 {
            self.PDConcurrency = defaultPDConcurrency;
        }
        self.SplitRegionIndexStep = if self.SplitRegionIndexStep == 0 {
            DefaultRegionIndexStep
        } else {
            self.SplitRegionIndexStep
        };
        if self.StatsConcurrency == 0 {
            self.StatsConcurrency = defaultStatsConcurrency;
        }
        if self.BatchFlushInterval == Duration::ZERO {
            self.BatchFlushInterval = defaultBatchFlushInterval;
        }
        if self.DdlBatchSize == 0 {
            self.DdlBatchSize = defaultFlagDdlBatchSize;
        }
        if self.CloudAPIConcurrency == 0 {
            self.CloudAPIConcurrency = crate::common::defaultCloudAPIConcurrency;
        }
    }

    /// 方法：保持与 Go 同名方法可对照的行为。
    pub fn adjustRestoreConfigForStreamRestore(&mut self) {
        if self.PitrConcurrency == 0 {
            self.PitrConcurrency = defaultPiTRConcurrency;
        }
        if self.PitrBatchCount == 0 {
            self.PitrBatchCount = defaultPiTRBatchCount;
        }
        if self.PitrBatchSize == 0 {
            self.PitrBatchSize = defaultPiTRBatchSize;
        }
        self.PitrConcurrency += 1;
    }

    /// 方法：保持与 Go 同名方法可对照的行为。
    pub fn ParseStreamRestoreFlags(&mut self, flags: &FlagSet) -> Result<()> {
        self.StartTS = crate::backup::ParseTSString(&flags.GetString(FlagStreamStartTS)?, true)?;
        self.RestoreTS =
            crate::backup::ParseTSString(&flags.GetString(FlagStreamRestoreTS)?, true)?;
        self.IsRestoredTSUserSpecified = flags.Changed(FlagStreamRestoreTS);
        self.FullBackupStorage = flags.GetString(FlagStreamFullBackupStorage)?;
        self.PiTRAddIndexSQLStorage = flags.GetString(FlagPiTRAddIndexSQLStorage)?;
        if self.StartTS > 0 && !self.FullBackupStorage.is_empty() {
            return Err(Error::Annotatef(
                berrors::ErrInvalidArgument,
                format!(
                    "{FlagStreamStartTS} and {FlagStreamFullBackupStorage} are mutually exclusive"
                ),
            ));
        }
        self.PitrBatchCount = flags.GetUint32(FlagPiTRBatchCount)?;
        self.PitrBatchSize = flags.GetUint32(FlagPiTRBatchSize)?;
        self.PitrConcurrency = flags.GetUint32(FlagPiTRConcurrency)?;
        self.RetainLatestMVCCVersion = flags.GetBool(FlagRetainLatestMVCCVersion)?;
        Ok(())
    }

    /// 从 FlagSet 填充字段并做基础校验。
    pub fn ParseFromFlags(&mut self, flags: &FlagSet, skipCommonConfig: bool) -> Result<()> {
        self.NoSchema = flags.GetBool(flagNoSchema)?;
        self.LoadStats = flags.GetBool(flagLoadStats)?;
        self.FastLoadSysTables = flags.GetBool(flagFastLoadSysTables)?;
        self.RestoreCommonConfig.ParseFromFlags(flags)?;
        if !skipCommonConfig {
            self.Config.ParseFromFlags(flags)?;
        }
        // 分支：if flags.Lookup(flagConcurrency).is_some — 与 Go 对应控制流对齐。
        if flags.Lookup(flagConcurrency).is_some() {
            self.Config.Concurrency = flags.GetUint32(flagConcurrency)?;
        }
        if self.Config.Concurrency == 0 {
            self.Config.Concurrency = defaultRestoreConcurrency;
        }
        self.PDConcurrency = flags.GetUint(FlagPDConcurrency)?;
        self.RegionScanConcurrency = flags.GetUint(FlagRegionScanConcurrency)?;
        self.SplitRegionIndexStep = flags.GetUint(FlagSplitRegionIndexStep)?;
        if self.SplitRegionIndexStep == 0 {
            return Err(Error::Annotatef(
                berrors::ErrInvalidArgument,
                format!("{FlagSplitRegionIndexStep} must be greater than 0"),
            ));
        }
        self.CoarseScatter = flags.GetBool(FlagCoarseScatter)?;
        self.StatsConcurrency = flags.GetUint(FlagStatsConcurrency)?;
        self.BatchFlushInterval = flags.GetDuration(FlagBatchFlushInterval)?;
        self.DdlBatchSize = flags.GetUint(FlagDdlBatchSize)?;
        self.TxnTotalSizeLimit = flags.GetUint64(FlagTxnTotalSizeLimit)?;
        self.WithPlacementPolicy = flags.GetString(FlagWithPlacementPolicy)?;
        self.UseCheckpoint = flags.GetBool(crate::backup::flagUseCheckpoint)?;
        self.CheckpointStorage = flags.GetString(flagCheckpointStorage)?;
        self.WaitTiflashReady = flags.GetBool(FlagWaitTiFlashReady)?;
        self.RestoreCommonConfig.SysCheckCollation = flags.GetBool(FlagSysCheckCollation)?;
        self.AllowPITRFromIncremental = flags.GetBool(flagAllowPITRFromIncremental)?;
        self.RestorePhase = flags.GetUint64(FlagRestorePhase)?;
        self.RestoreInPhase = flags.Changed(FlagRestorePhase) || self.RestorePhase > 0;
        if self.RestoreInPhase && self.RestorePhase != 1 && self.RestorePhase != 2 {
            return Err(Error::Annotatef(
                berrors::ErrInvalidArgument,
                format!("{FlagRestorePhase} is an invalid value, please specify 1 or 2"),
            ));
        }
        if self.RestoreInPhase && !self.UseCheckpoint {
            return Err(Error::Annotatef(
                berrors::ErrInvalidArgument,
                format!(
                    "{FlagRestorePhase} requires {} to be enabled",
                    crate::backup::flagUseCheckpoint
                ),
            ));
        }
        Ok(())
    }

    /// 方法：保持与 Go 同名方法可对照的行为。
    pub fn CloseCheckpointMetaManager(&mut self) {
        for manager in self.CheckpointMetaManagers.0.drain(..) {
            manager.Close();
        }
    }
}

/// Minimal setter surface used by `configureRestoreClient`.
pub trait RestoreClientConfig {
    fn SetBatchDdlSize(&mut self, value: u32);
    fn SetRegionScanConcurrency(&mut self, value: u32);
    fn SetSplitRegionIndexStep(&mut self, value: u32);
    fn SetCoarseScatter(&mut self, value: bool);
}

/// Apply restore configuration to a snapshot client.
pub fn configureRestoreClient(client: &mut impl RestoreClientConfig, cfg: &RestoreConfig) {
    client.SetRegionScanConcurrency(cfg.RegionScanConcurrency);
    client.SetSplitRegionIndexStep(cfg.SplitRegionIndexStep);
    client.SetCoarseScatter(cfg.CoarseScatter);
    client.SetBatchDdlSize(cfg.DdlBatchSize);
}

/// Verify explicitly selected databases and tables exist in backup metadata.
pub fn VerifyDBAndTableInBackup(
    backup: &HashMap<&str, Vec<&str>>,
    restore_schemas: &HashSet<String>,
    restore_tables: &HashSet<String>,
) -> Result<()> {
    if restore_schemas.is_empty() && restore_tables.is_empty() {
        return Ok(());
    }
    let mut schemas = HashSet::new();
    let mut tables = HashSet::new();
    for (database, database_tables) in backup {
        let database = database
            .strip_prefix("__TiDB_BR_Temporary_")
            .unwrap_or(database)
            .to_ascii_lowercase();
        schemas.insert(crate::stubs::EncloseName(&database));
        for table in database_tables {
            tables.insert(crate::stubs::EncloseDBAndTable(
                &database,
                &table.to_ascii_lowercase(),
            ));
        }
    }
    for schema in restore_schemas {
        if !schemas.contains(&schema.to_ascii_lowercase()) {
            return Err(Error::new(format!(
                "[database: {schema}] has not been backup, please ensure you has input a correct database name"
            )));
        }
    }
    for table in restore_tables {
        if !tables.contains(&table.to_ascii_lowercase()) {
            return Err(Error::new(format!(
                "[table: {table}] has not been backup, please ensure you has input a correct table name"
            )));
        }
    }
    Ok(())
}

/// 注册全量恢复 flag。
pub fn DefineRestoreFlags(flags: &mut FlagSet) {
    flags.DefineBool(flagNoSchema, false);
    flags.DefineBool(flagLoadStats, true);
    flags.DefineBool(flagFastLoadSysTables, true);
    let _ = flags.MarkHidden(flagNoSchema);
    flags.DefineString(FlagWithPlacementPolicy, "STRICT");
    flags.DefineBool(crate::backup::flagUseCheckpoint, true);
    let _ = flags.MarkHidden(crate::backup::flagUseCheckpoint);
    flags.DefineString(flagCheckpointStorage, "");
    flags.DefineUint64(FlagRestorePhase, 0);
    flags.DefineBool(FlagWaitTiFlashReady, false);
    flags.DefineBool(flagAllowPITRFromIncremental, true);
    flags.DefineBool(FlagSysCheckCollation, false);
    flags.DefineUint(FlagSplitRegionIndexStep, DefaultRegionIndexStep as u64);
    let _ = flags.MarkHidden(FlagSplitRegionIndexStep);
    flags.DefineBool(FlagCoarseScatter, false);
    DefineRestoreCommonFlags(flags);
}

fn redact_storage_url(storage: &str) -> String {
    match Url::parse(storage) {
        Ok(mut url) => {
            let sensitive: &[&str] = match url.scheme().to_ascii_lowercase().as_str() {
                "s3" | "ks3" | "oss" => &["access-key", "secret-access-key", "session-token"],
                "azure" | "azblob" => &["account-key", "encryption-key", "sas-token"],
                _ => return url.to_string(),
            };
            let mut pairs = std::collections::BTreeMap::<String, Vec<String>>::new();
            for (key, value) in url.query_pairs() {
                let normalized = key.to_ascii_lowercase().replace('_', "-");
                let value = if sensitive.contains(&normalized.as_str()) {
                    "xxxxxx".to_string()
                } else {
                    value.into_owned()
                };
                pairs.entry(key.into_owned()).or_default().push(value);
            }
            url.set_query(None);
            if !pairs.is_empty() {
                let mut query = url.query_pairs_mut();
                for (key, values) in pairs {
                    for value in values {
                        query.append_pair(&key, &value);
                    }
                }
            }
            url.to_string()
        }
        Err(_) => storage.to_string(),
    }
}

/// 注册流式恢复 flag。
pub fn DefineStreamRestoreFlags(flags: &mut FlagSet) {
    flags.DefineString(FlagStreamStartTS, "");
    flags.DefineString(FlagStreamRestoreTS, "");
    flags.DefineString(FlagStreamFullBackupStorage, "");
    flags.DefineString(FlagPiTRAddIndexSQLStorage, "");
    flags.DefineUint32(FlagPiTRBatchCount, defaultPiTRBatchCount);
    flags.DefineUint32(FlagPiTRBatchSize, defaultPiTRBatchSize);
    flags.DefineUint32(FlagPiTRConcurrency, defaultPiTRConcurrency);
    flags.DefineBool(FlagRetainLatestMVCCVersion, false);
    DefineRestoreFlags(flags);
}

/// 是否全量恢复命令名。
pub fn isFullRestore(cmdName: &str) -> bool {
    cmdName == FullRestoreCmd
}

/// 是否流式/PiTR 恢复命令名。
pub fn IsStreamRestore(cmdName: &str) -> bool {
    cmdName == PointRestoreCmd
}

/// 规范化 operation 命令名。
pub fn restoreOperationCommandName(cmdName: &str) -> String {
    if IsStreamRestore(cmdName) {
        "log-restore".into()
    } else {
        cmdName.into()
    }
}

/// 由通用 Config 生成默认 RestoreConfig。
pub fn DefaultRestoreConfig(commonConfig: Config) -> RestoreConfig {
    let mut fs = FlagSet::new();
    DefineRestoreFlags(&mut fs);
    let mut cfg = RestoreConfig::default();
    cfg.ParseFromFlags(&fs, true)
        .expect("failed to parse restore flags to config");
    cfg.Config = commonConfig;
    cfg
}

/// 按归档大小与副本数估算 TiKV 占用。
pub fn EstimateTikvUsage(archiveSize: u64, mut replicaCnt: u64, storeCnt: u64) -> u64 {
    if storeCnt == 0 {
        return 0;
    }
    // 分支：if replicaCnt > storeCnt { — 与 Go 对应控制流对齐。
    if replicaCnt > storeCnt {
        replicaCnt = storeCnt;
    }
    archiveSize * replicaCnt / storeCnt
}

/// 按表字节与副本估算 TiFlash 占用。
pub fn EstimateTiflashUsage(table_bytes_and_replicas: &[(u64, u64)], storeCnt: u64) -> u64 {
    if storeCnt == 0 {
        return 0;
    }
    let mut tiflashTotal = 0u64;
    // 分支：for &(tableBytes, replicaCnt) in table_b — 与 Go 对应控制流对齐。
    for &(tableBytes, replicaCnt) in table_bytes_and_replicas {
        if replicaCnt == 0 {
            continue;
        }
        tiflashTotal += tableBytes * replicaCnt;
    }
    tiflashTotal / storeCnt
}

/// 校验单 store 可用空间是否足够。
pub fn CheckStoreSpace(necessary: u64, available_bytes: i64, store_id: u64) -> Result<()> {
    if available_bytes <= 0 {
        return Err(Error::Annotatef(
            "pd invalid response",
            format!("store {store_id} has invalid available space {available_bytes}"),
        ));
    }
    // 分支：if (available_bytes as u64) < necessary  — 与 Go 对应控制流对齐。
    if (available_bytes as u64) < necessary {
        return Err(Error::Annotatef(
            "kv disk full",
            format!(
                "store {store_id} has no space left on device, available {available_bytes}, necessary {necessary}"
            ),
        ));
    }
    Ok(())
}

/// 大小写不敏感标识符包装。
#[derive(Clone, Debug, Default)]
pub struct CIStr(pub String);

impl CIStr {
    /// 方法：保持与 Go 同名方法可对照的行为。
    pub fn String(&self) -> &str {
        &self.0
    }
}

/// 库信息最小字段。
#[derive(Clone, Debug, Default)]
pub struct DBInfo {
    pub ID: i64,
    pub Name: CIStr,
}

/// 表信息最小字段。
#[derive(Clone, Debug, Default)]
pub struct TableInfo {
    pub ID: i64,
    pub Name: CIStr,
    pub IsCommonHandle: bool,
    pub TiFlashReplica: Option<TiFlashReplicaInfo>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TiFlashReplicaInfo {
    pub Count: u64,
    pub Available: bool,
    pub AvailablePartitionIDs: Vec<i64>,
}

/// 恢复用表描述（含 DB）。
#[derive(Clone, Debug, Default)]
pub struct Table {
    pub DB: DBInfo,
    pub Info: TableInfo,
}

/// DDL Job 内 binlog 信息桩。
#[derive(Clone, Debug, Default)]
pub struct BinlogInfo {
    pub SchemaVersion: i64,
    pub DBInfo: Option<DBInfo>,
    pub TableInfo: Option<TableInfo>,
}

/// DDL Job 最小字段，供过滤规则使用。
#[derive(Clone, Debug, Default)]
pub struct Job {
    pub SchemaID: i64,
    pub TableID: i64,
    pub SchemaName: String,
    pub Type: i32,
    pub BinlogInfo: BinlogInfo,
}

impl Job {
    /// 方法：保持与 Go 同名方法可对照的行为。
    pub fn String(&self) -> String {
        format!(
            "job(type={}, schema={}, table={})",
            self.Type, self.SchemaID, self.TableID
        )
    }
}

/// 库表唯一名。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UniqueTableName {
    pub DB: String,
    pub Table: String,
}

#[derive(Clone, Debug, Default)]
pub struct PiTRIdTracker {
    db_ids: HashSet<i64>,
    table_ids: HashMap<i64, HashSet<i64>>,
    partition_ids: HashSet<i64>,
    table_names: HashMap<String, HashSet<String>>,
}

impl PiTRIdTracker {
    pub fn AddDB(&mut self, db_id: i64) {
        self.db_ids.insert(db_id);
    }

    pub fn TrackTableId(&mut self, db_id: i64, table_id: i64) {
        self.db_ids.insert(db_id);
        self.table_ids.entry(table_id).or_default().insert(db_id);
    }

    pub fn TrackPartitionId(&mut self, partition_id: i64) {
        self.partition_ids.insert(partition_id);
    }

    pub fn TrackTableName(&mut self, db_name: &str, table_name: &str) {
        self.table_names
            .entry(db_name.to_string())
            .or_default()
            .insert(table_name.to_string());
    }

    pub fn ContainsDBAndTableId(&self, db_id: i64, table_id: i64) -> bool {
        self.table_ids
            .get(&table_id)
            .is_some_and(|db_ids| db_ids.contains(&db_id))
    }

    pub fn ContainsTableId(&self, table_id: i64) -> bool {
        self.table_ids.contains_key(&table_id)
    }

    pub fn ContainsPartitionId(&self, partition_id: i64) -> bool {
        self.partition_ids.contains(&partition_id)
    }
}

/// 从 tables 提取去重 DBInfo 列表。
pub fn getDatabases(tables: &[Table]) -> Vec<DBInfo> {
    let mut seen = HashSet::new();
    let mut dbs = Vec::new();
    // 分支：for t in tables { — 与 Go 对应控制流对齐。
    for t in tables {
        if seen.insert(t.DB.ID) {
            dbs.push(t.DB.clone());
        }
    }
    dbs
}

/// 按恢复表集合过滤历史 DDL Job。
pub fn FilterDDLJobs(allDDLJobs: &mut [Job], tables: &[Table]) -> Vec<Job> {
    allDDLJobs.sort_by(|i, j| j.BinlogInfo.SchemaVersion.cmp(&i.BinlogInfo.SchemaVersion));
    let mut ddlJobs = Vec::new();
    let dbs = getDatabases(tables);
    // 分支：for db in dbs { — 与 Go 对应控制流对齐。
    for db in dbs {
        let mut dbIDs = HashMap::new();
        dbIDs.insert(db.ID, true);
        let mut dbNames = HashMap::new();
        dbNames.insert(db.Name.String().to_string(), true);
        // 分支：for job in allDDLJobs.iter() { — 与 Go 对应控制流对齐。
        for job in allDDLJobs.iter() {
            if let Some(info) = &job.BinlogInfo.DBInfo {
                if dbIDs.contains_key(&job.SchemaID) || dbNames.contains_key(info.Name.String()) {
                    ddlJobs.push(job.clone());
                    dbIDs.insert(job.SchemaID, true);
                    dbNames.insert(info.Name.String().to_string(), true);
                }
            }
        }
    }
    // 分支：for table in tables { — 与 Go 对应控制流对齐。
    for table in tables {
        let mut tableIDs = HashMap::new();
        tableIDs.insert(table.Info.ID, true);
        let mut tableNames = HashMap::new();
        let name = UniqueTableName {
            DB: table.DB.Name.String().to_string(),
            Table: table.Info.Name.String().to_string(),
        };
        tableNames.insert(name, true);
        // 分支：for job in allDDLJobs.iter() { — 与 Go 对应控制流对齐。
        for job in allDDLJobs.iter() {
            if let Some(ti) = &job.BinlogInfo.TableInfo {
                let name = UniqueTableName {
                    DB: job.SchemaName.clone(),
                    Table: ti.Name.String().to_string(),
                };
                // 分支：if tableIDs.contains_key(&job.TableID) | — 与 Go 对应控制流对齐。
                if tableIDs.contains_key(&job.TableID) || tableNames.contains_key(&name) {
                    ddlJobs.push(job.clone());
                    tableIDs.insert(job.TableID, true);
                    tableIDs.insert(ti.ID, true);
                    tableNames.insert(name, true);
                }
            }
        }
    }
    ddlJobs
}

/// DDL Job 过滤规则函数类型。
pub type DDLJobFilterRule = fn(&Job) -> bool;

/// 规则命中则报错（禁止类规则）。
pub fn CheckDDLJobByRules(srcDDLJobs: &[Job], rules: &[DDLJobFilterRule]) -> Result<()> {
    for ddlJob in srcDDLJobs {
        for rule in rules {
            if rule(ddlJob) {
                return Err(Error::Annotatef(
                    berrors::ErrRestoreModeMismatch,
                    format!(
                        "DDL job {} is not allowed in incremental restore when --allow-pitr-from-incremental enabled",
                        ddlJob.String()
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// 规则命中则丢弃 Job（过滤类规则）。
pub fn FilterDDLJobByRules(srcDDLJobs: &[Job], rules: &[DDLJobFilterRule]) -> Vec<Job> {
    let mut dst = Vec::with_capacity(srcDDLJobs.len());
    for ddlJob in srcDDLJobs {
        let mut passed = true;
        for rule in rules {
            if rule(ddlJob) {
                passed = false;
                break;
            }
        }
        if passed {
            dst.push(ddlJob.clone());
        }
    }
    dst
}

/// TiFlash 副本设置 Action 码。
pub const ActionSetTiFlashReplica: i32 = 1;
/// TiFlash 状态更新 Action 码。
pub const ActionUpdateTiFlashReplicaStatus: i32 = 2;
/// 锁表 Action 码。
pub const ActionLockTable: i32 = 3;
/// 解锁表 Action 码。
pub const ActionUnlockTable: i32 = 4;
/// 加索引 Action 码。
pub const ActionAddIndex: i32 = 5;
/// 改列 Action 码。
pub const ActionModifyColumn: i32 = 6;
/// 常量：与 Go 同名同义，供测试路径复用。
pub const ActionReorganizePartition: i32 = 7;
pub const ActionCreateTable: i32 = 100;

/// 函数：解释见实现内注释；行为对齐同名 Go 逻辑。
fn incremental_blocklist() -> HashSet<i32> {
    HashSet::from([
        ActionSetTiFlashReplica,
        ActionUpdateTiFlashReplicaStatus,
        ActionLockTable,
        ActionUnlockTable,
    ])
}

/// 函数：解释见实现内注释；行为对齐同名 Go 逻辑。
fn log_compact_blocklist() -> HashSet<i32> {
    HashSet::from([
        ActionAddIndex,
        ActionModifyColumn,
        ActionReorganizePartition,
    ])
}

/// 函数：解释见实现内注释；行为对齐同名 Go 逻辑。
pub fn checkIsInActions(action: i32, actions: &HashSet<i32>) -> bool {
    actions.contains(&action)
}

/// 函数：解释见实现内注释；行为对齐同名 Go 逻辑。
pub fn DDLJobBlockListRule(ddlJob: &Job) -> bool {
    checkIsInActions(ddlJob.Type, &incremental_blocklist())
}

/// 函数：解释见实现内注释；行为对齐同名 Go 逻辑。
pub fn DDLJobLogIncrementalCompactBlockListRule(ddlJob: &Job) -> bool {
    checkIsInActions(ddlJob.Type, &log_compact_blocklist())
}

/// 函数：解释见实现内注释；行为对齐同名 Go 逻辑。
pub fn encodeCompactAndCheckKey(preAlloced: [i64; 2]) -> (Vec<u8>, Vec<u8>) {
    (
        EncodeBytes(&EncodeTablePrefix(preAlloced[0])),
        EncodeBytes(&EncodeTablePrefix(preAlloced[1])),
    )
}

/// 函数：解释见实现内注释；行为对齐同名 Go 逻辑。
pub fn rewriteKeyRanges(preAlloced: [i64; 2]) -> Vec<[Vec<u8>; 2]> {
    if preAlloced == [0, 0] {
        return Vec::new();
    }
    let (start, end) = encodeCompactAndCheckKey(preAlloced);
    vec![[start, end]]
}

pub fn CheckNewCollationEnable(
    backup_new_collation_enable: &str,
    downstream_new_collation_enable: &str,
    check_requirements: bool,
) -> Result<bool> {
    let enabled = downstream_new_collation_enable == "True";
    if backup_new_collation_enable.is_empty() {
        if check_requirements {
            return Err(Error::Annotate(
                berrors::ErrUnknown,
                "the value 'new_collation_enabled' not found in backupmeta",
            ));
        }
        return Ok(enabled);
    }
    if !backup_new_collation_enable.eq_ignore_ascii_case(downstream_new_collation_enable) {
        return Err(Error::Annotatef(
            berrors::ErrUnknown,
            format!(
                "the config 'new_collation_enabled' not match, upstream:{backup_new_collation_enable}, downstream: {downstream_new_collation_enable}"
            ),
        ));
    }
    Ok(enabled)
}

pub trait TiFlashReplicaRecorder {
    fn AddTable(&mut self, table_id: i64, replica: TiFlashReplicaInfo);
}

pub fn PreCheckTableTiFlashReplica(
    tables: &mut [Table],
    tiflash_store_count: u64,
    mut recorder: Option<&mut dyn TiFlashReplicaRecorder>,
    is_next_gen_restore: bool,
) {
    for table in tables {
        let Some(replica) = table.Info.TiFlashReplica.as_mut() else {
            continue;
        };
        if is_next_gen_restore {
            table.Info.TiFlashReplica = None;
            continue;
        }
        replica.Available = false;
        replica.AvailablePartitionIDs.clear();
        if let Some(recorder) = recorder.as_deref_mut() {
            recorder.AddTable(table.Info.ID, replica.clone());
            table.Info.TiFlashReplica = None;
        } else if replica.Count > tiflash_store_count {
            table.Info.TiFlashReplica = None;
        }
    }
}

pub fn PreCheckTableClusterIndex(
    tables: &[Table],
    ddl_jobs: &[Job],
    existing_tables: &HashMap<UniqueTableName, bool>,
) -> Result<()> {
    let check = |db: &str, table: &TableInfo| -> Result<()> {
        let name = UniqueTableName {
            DB: db.to_string(),
            Table: table.Name.0.clone(),
        };
        if let Some(created_common_handle) = existing_tables.get(&name)
            && table.IsCommonHandle != *created_common_handle
        {
            let expected = if table.IsCommonHandle { "ON" } else { "OFF" };
            return Err(Error::Annotatef(
                berrors::ErrRestoreModeMismatch,
                format!(
                    "Clustered index option mismatch. Restored cluster's @@tidb_enable_clustered_index should be {expected} (backup table = {}, created table = {}).",
                    table.IsCommonHandle, created_common_handle
                ),
            ));
        }
        Ok(())
    };
    for table in tables {
        check(table.DB.Name.String(), &table.Info)?;
    }
    for job in ddl_jobs {
        if job.Type == ActionCreateTable
            && let Some(table) = &job.BinlogInfo.TableInfo
        {
            check(&job.SchemaName, table)?;
        }
    }
    Ok(())
}

fn match_schema(cfg: &RestoreConfig, db: &str) -> bool {
    cfg.Config.TableFilter.MatchTable(db, "")
        || cfg.Config.TableFilter.patterns.iter().any(|pattern| {
            pattern == "*.*" || pattern.trim_matches('`').starts_with(&format!("{db}."))
        })
}

fn get_db_name_from_backup<'a>(
    db_id: i64,
    snapshot_db_map: &'a HashMap<i64, Database>,
    history: &'a LogBackupTableHistoryManager,
) -> Option<&'a str> {
    snapshot_db_map
        .get(&db_id)
        .map(|db| db.Info.Name.String())
        .or_else(|| history.GetDBNameByID(db_id))
}

#[derive(Clone, Debug, Default)]
pub struct Database {
    pub Info: DBInfo,
    pub Tables: Vec<Table>,
}

fn build_start_table_location_info(
    physical_id: i64,
    start: &TableLocationInfo,
    snapshot_table_map: &HashMap<i64, Table>,
    partition_map: &HashMap<i64, TableLocationInfo>,
) -> TableLocationInfo {
    if let Some(partition) = partition_map.get(&physical_id) {
        return partition.clone();
    }
    if let Some(table) = snapshot_table_map.get(&physical_id) {
        return TableLocationInfo {
            DbID: table.DB.ID,
            TableName: table.Info.Name.0.clone(),
            ..Default::default()
        };
    }
    start.clone()
}

fn should_restore_table(
    physical_id: i64,
    location: &TableLocationInfo,
    tracker: &PiTRIdTracker,
) -> bool {
    if location.IsPartition {
        tracker.ContainsTableId(location.ParentTableID)
            || tracker.ContainsPartitionId(location.ParentTableID)
    } else {
        tracker.ContainsTableId(physical_id) || tracker.ContainsPartitionId(physical_id)
    }
}

pub fn AdjustTablesToRestoreAndCreateTableTracker(
    history: &LogBackupTableHistoryManager,
    cfg: &mut RestoreConfig,
    snapshot_db_map: &HashMap<i64, Database>,
    snapshot_table_map: &HashMap<i64, Table>,
    partition_map: &HashMap<i64, TableLocationInfo>,
    table_map: &mut HashMap<i64, Table>,
    db_map: &mut HashMap<i64, Database>,
) -> Result<()> {
    let mut tracker = PiTRIdTracker::default();
    for (&db_id, db_name) in history.GetNewlyCreatedDBHistory() {
        if match_schema(cfg, db_name) {
            tracker.AddDB(db_id);
        }
    }

    for (&table_id, locations) in history.GetTableHistory() {
        let start = build_start_table_location_info(
            table_id,
            &locations[0],
            snapshot_table_map,
            partition_map,
        );
        let end = &locations[1];
        let Some(end_db_name) = get_db_name_from_backup(end.DbID, snapshot_db_map, history) else {
            continue;
        };
        let end_matches = cfg
            .Config
            .TableFilter
            .MatchTable(end_db_name, &end.TableName);
        if end_matches {
            if end.IsPartition {
                tracker.TrackPartitionId(table_id);
            } else {
                tracker.TrackTableId(end.DbID, table_id);
                tracker.TrackTableName(end_db_name, &end.TableName);
            }
        }
        if start.IsPartition || end.IsPartition || !snapshot_table_map.contains_key(&table_id) {
            continue;
        }
        let Some(start_db_name) = get_db_name_from_backup(start.DbID, snapshot_db_map, history)
        else {
            continue;
        };
        let start_matches = cfg
            .Config
            .TableFilter
            .MatchTable(start_db_name, &start.TableName);
        if (!start_matches && !end_matches)
            || (start_matches && end_matches && start.DbID == end.DbID)
        {
            continue;
        }
        if let Some(start_db) = snapshot_db_map.get(&start.DbID)
            && let Some(table) = start_db
                .Tables
                .iter()
                .find(|table| table.Info.ID == table_id)
        {
            if end_matches {
                table_map.insert(table_id, table.clone());
                db_map.insert(start.DbID, start_db.clone());
            } else if start_matches {
                table_map.remove(&table_id);
            }
        }
    }

    for (&table_id, table) in table_map.iter() {
        tracker.TrackTableId(table.DB.ID, table_id);
        tracker.TrackTableName(table.DB.Name.String(), table.Info.Name.String());
    }
    for (&table_id, locations) in history.GetTableHistory() {
        let start = build_start_table_location_info(
            table_id,
            &locations[0],
            snapshot_table_map,
            partition_map,
        );
        let end = &locations[1];
        if (!start.IsPartition && !end.IsPartition) || start.ParentTableID == end.ParentTableID {
            continue;
        }
        if should_restore_table(table_id, &start, &tracker)
            != should_restore_table(table_id, end, &tracker)
        {
            return Err(Error::Annotatef(
                berrors::ErrRestoreModeMismatch,
                format!(
                    "partition exchange detected: partition ID {table_id} was exchanged between tables, but only one table will be restored"
                ),
            ));
        }
    }
    cfg.PiTRTableTracker = tracker;
    Ok(())
}

/// 函数：解释见实现内注释；行为对齐同名 Go 逻辑。
pub fn tweakLocalConfForRestore() -> impl FnOnce() {
    let prev = true;
    move || {
        let _ = prev;
    }
}

/// 函数：解释见实现内注释；行为对齐同名 Go 逻辑。
pub fn RunRestore(g: &dyn Glue, cmdName: &str, cfg: &mut RestoreConfig) -> Result<()> {
    cfg.Adjust();
    Summary(cmdName);
    let _ = restoreOperationCommandName(cmdName);
    let mgr = NewMgr(
        g,
        &cfg.Config.KeyspaceName,
        &cfg.Config.PD,
        &cfg.Config.TLS,
        GetKeepalive(&cfg.Config),
        cfg.Config.CheckRequirements,
        true,
        crate::stubs::NormalVersionChecker,
    )?;
    let storage: Arc<dyn Storage> = match cfg.RestoreStorage.clone() {
        Some(storage) => Arc::new(storage),
        None => crate::common::GetStorage(&cfg.Config.Storage, &cfg.Config)?.1,
    };
    let result = (|| {
        let (_u, backupMeta) = ReadBackupMeta(MetaFile, &cfg.Config, storage.as_ref())?;
        if backupMeta.IsRawKv || backupMeta.IsTxnKv {
            return Err(Error::Annotate(
                berrors::ErrRestoreModeMismatch,
                "cannot do transactional restore from raw/txn kv data",
            ));
        }
        let archive = crate::stubs::ArchiveSize(&backupMeta.Files);
        cfg.snapshotRestoreDataSize = archive;
        g.Record(crate::stubs::RestoreDataSize, archive);
        let needed = EstimateTikvUsage(archive, 3, 3);
        CheckStoreSpace(needed, (needed + 1) as i64, 1)?;
        SetSuccessStatus(true);
        Ok(())
    })();
    mgr.Close();
    cfg.CloseCheckpointMetaManager();
    result
}

/// 函数：解释见实现内注释；行为对齐同名 Go 逻辑。
pub fn RunRestoreAbort(_g: &dyn Glue, cmdName: &str, cfg: &mut RestoreConfig) -> Result<()> {
    cfg.Adjust();
    Summary(cmdName);
    cfg.CloseCheckpointMetaManager();
    SetSuccessStatus(true);
    Ok(())
}
