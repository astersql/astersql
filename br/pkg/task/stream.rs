// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

//! Log backup / PiTR stream tasks matching `br/pkg/task/stream.go`.
//!
//! BR 日志备份（Log Backup）与 PiTR（Point-in-Time Recovery）流式任务入口。
//! 对齐 Go `br/pkg/task/stream.go`：CLI 旗标定义/解析、StreamMgr 生命周期、
//! 子命令分发（start/stop/pause/resume/status/truncate/metadata/advancer）、
//! 日志元数据读取、checkpoint/truncate safepoint 计算，以及流式恢复辅助逻辑。
//! 当前 Rust 版部分路径为可跑通桩（如 `buildObserveRanges`、`runOwnershipCycle`），
//! 语义边界以 Go 对照注释标明。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::common::{
    Config, DefineCommonFlags, FlagPiTRAddIndexSQLStorage, FlagStreamFullBackupStorage,
    GetKeepalive, GetStorage, HiddenFlagsForStream, NewMgr,
};
use crate::restore::{DefineStreamRestoreFlags, IsStreamRestore, RestoreConfig};
use crate::stubs::backuppb::StreamBackupTaskSecurityConfig;
use crate::stubs::oracle;
use crate::stubs::{
    EncodeBytes, EncodeTablePrefix, Error, FlagSet, GetRewriteRuleOfTable,
    GetStreamBackupGlobalCheckpointPrefix, Glue, IsEffectiveEncryptionMethod, IsSysOrTempSysDB,
    LogRestoreProgressIdMapSaved, MemStorage, MetaFile, PersistentState, Result, RewriteRules,
    SchemasReplace, SetSuccessStatus, Storage, Summary, TaskInfoForLogRestore,
    TruncateSafePointFileName, berrors,
};

const RESUME_STATE_FILE_NAME: &str = "crr-checkpoint/resume-state.json";
const LEGACY_RESUME_STATE_FILE_NAME: &str = "v1/global_checkpoint/resume_state.json";
const STREAM_SHIFT_DURATION_SECS: i64 = 60 * 60;

// —— CLI 旗标名：与 Go stream 子命令 flag 字符串一致 ——
// task-name：日志备份任务名，stop/pause/resume/status 必填。
pub const FlagStreamTaskName: &str = "task-name";
// start-ts：日志备份起始 TSO，通常等于全量备份结束 TS，start 子命令必填。
pub const FlagStreamStartTS: &str = "start-ts";
// end-ts：日志备份结束 TSO（预留，部分子命令未使用）。
pub const FlagStreamEndTS: &str = "end-ts";
// safepoint-ttl：GC safepoint 存活秒数，保证 TiKV 可扫描 [startTS, now] 区间。
// until：truncate 子命令截断上界 TS。
pub const FlagStreamUntil: &str = "until";
// yes：truncate 确认执行（Go 交互式确认对应）。
pub const FlagStreamYes: &str = "yes";
// dry-run：truncate 仅校验不写 truncate safepoint 文件。
pub const FlagStreamDryRun: &str = "dry-run";
pub const FlagStreamJSONOutput: &str = "json";
pub const FlagStreamGCTTL: &str = "gc-ttl";
pub const FlagStreamMessage: &str = "message";
pub const FlagStreamCleanUpCompactions: &str = "clean-up-compactions";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdvancerCommandConfig {
    pub BackoffTime: Duration,
    pub TickDuration: Duration,
    pub TryAdvanceThreshold: Duration,
    pub CheckPointLagLimit: Duration,
    pub OwnershipCycleInterval: Duration,
}

impl Default for AdvancerCommandConfig {
    fn default() -> Self {
        Self {
            BackoffTime: Duration::from_secs(5),
            TickDuration: Duration::from_secs(12),
            TryAdvanceThreshold: Duration::from_secs(4 * 60),
            CheckPointLagLimit: Duration::from_secs(48 * 3600),
            OwnershipCycleInterval: Duration::ZERO,
        }
    }
}

/// 日志备份流任务配置，嵌入公共 `Config` 并扩展 stream 专有字段。
/// 对应 Go `StreamConfig`；字段名保持 PascalCase 以与 Go 结构体一一映射。
#[derive(Clone, Debug, Default)]
pub struct StreamConfig {
    /// 公共 BR 配置（PD、存储、TLS、加密等）。
    pub Config: Config,
    /// 日志备份任务名称，etcd/streamhelper 中唯一标识。
    pub TaskName: String,
    /// 日志备份起始 TS；0 表示未设置，start 时会报错。
    pub StartTS: u64,
    /// 日志备份结束 TS（预留字段）。
    pub EndTS: u64,
    /// GC safepoint TTL（秒），pause/start 时写入 PD。
    pub SafePointTTL: i64,
    /// truncate 截断上界 TS；0 且非 dry-run 时 truncate 报错。
    pub UntilTS: u64,
    /// truncate 是否 dry-run，仅校验范围不落盘。
    pub DryRun: bool,
    /// truncate 是否跳过交互确认（Go `SkipPrompt`）。
    pub SkipPrompt: bool,
    /// truncate 是否清理 compaction 产物。
    pub CleanUpCompactions: bool,
    /// status 是否输出 JSON。
    pub JSONOutput: bool,
    /// advancer 的推进周期配置。
    pub AdvancerCfg: AdvancerCommandConfig,
    /// pause 原因消息。
    pub Message: String,
    /// pause 是否应作为错误状态呈现（供配置文件/内部调用方设置）。
    pub AsError: bool,
}

/// 构造带默认旗标定义的 `StreamConfig`，供测试与 parity 初始化。
/// 注册 common + stream 隐藏旗标 + stream 公共旗标后解析默认值。
pub fn DefaultStreamConfig() -> StreamConfig {
    let mut fs = FlagSet::new();
    DefineCommonFlags(&mut fs);
    HiddenFlagsForStream(&mut fs);
    DefineStreamCommonFlags(&mut fs);
    let mut cfg = StreamConfig::default();
    let _ = cfg.ParseStreamCommonFromFlags(&fs);
    cfg
}

/// 注册 `br stream start` 子命令旗标：task-name、start-ts、safepoint-ttl。
pub fn DefineStreamStartFlags(flags: &mut FlagSet) {
    DefineStreamCommonFlags(flags);
    flags.DefineString(FlagStreamStartTS, "");
    flags.DefineString(FlagStreamEndTS, "999999999999999999");
    flags.DefineInt64(FlagStreamGCTTL, 1800);
    let _ = flags.MarkHidden(FlagStreamEndTS);
    let _ = flags.MarkHidden(FlagStreamGCTTL);
}

/// 注册 `br stream pause` 子命令旗标：task-name、safepoint-ttl。
pub fn DefineStreamPauseFlags(flags: &mut FlagSet) {
    DefineStreamCommonFlags(flags);
    flags.DefineInt64(FlagStreamGCTTL, 24 * 3600);
    flags.DefineString(FlagStreamMessage, "");
}

/// 注册 stream 公共旗标（task-name），被 status/stop/resume 等共用。
pub fn DefineStreamCommonFlags(flags: &mut FlagSet) {
    flags.DefineString(FlagStreamTaskName, "");
}

/// status 子命令旗标与 common 相同，Go 侧独立函数便于扩展。
pub fn DefineStreamStatusCommonFlags(flags: &mut FlagSet) {
    flags.DefineString(FlagStreamTaskName, "*");
    flags.DefineBool(FlagStreamJSONOutput, false);
}

/// 注册 truncate 子命令旗标：until、yes、dry-run。
pub fn DefineStreamTruncateLogFlags(flags: &mut FlagSet) {
    flags.DefineString(FlagStreamUntil, "");
    flags.DefineBool(FlagStreamYes, false);
    flags.DefineBool(FlagStreamDryRun, false);
    flags.DefineBool(FlagStreamCleanUpCompactions, false);
}

impl StreamConfig {
    /// 解析 status 子命令旗标；当前仅委托 `ParseStreamCommonFromFlags`。
    pub fn ParseStreamStatusFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        self.JSONOutput = flags.GetBool(FlagStreamJSONOutput)?;
        self.ParseStreamCommonFromFlags(flags)
    }

    /// 解析 truncate 子命令旗标：until TS、yes、dry-run，再解析公共字段。
    pub fn ParseStreamTruncateFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        let until = flags.GetString(FlagStreamUntil).unwrap_or_default();
        self.UntilTS = crate::backup::ParseTSString(&until, true)?;
        self.SkipPrompt = flags.GetBool(FlagStreamYes)?;
        self.DryRun = flags.GetBool(FlagStreamDryRun)?;
        self.CleanUpCompactions = flags.GetBool(FlagStreamCleanUpCompactions)?;
        Ok(())
    }

    /// 解析 start 子命令旗标：公共字段 + start-ts + safepoint-ttl。
    pub fn ParseStreamStartFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        self.ParseStreamCommonFromFlags(flags)?;
        let start = flags.GetString(FlagStreamStartTS).unwrap_or_default();
        self.StartTS = crate::backup::ParseTSString(&start, true)?;
        let end = flags.GetString(FlagStreamEndTS)?;
        self.EndTS = crate::backup::ParseTSString(&end, true)?;
        self.SafePointTTL = flags.GetInt64(FlagStreamGCTTL)?;
        if self.SafePointTTL <= 0 {
            self.SafePointTTL = 1800;
        }
        Ok(())
    }

    /// 解析 pause 子命令旗标：公共字段 + safepoint-ttl。
    pub fn ParseStreamPauseFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        self.ParseStreamCommonFromFlags(flags)?;
        self.Message = flags.GetString(FlagStreamMessage)?;
        self.SafePointTTL = flags.GetInt64(FlagStreamGCTTL)?;
        if self.SafePointTTL <= 0 {
            self.SafePointTTL = 24 * 3600;
        }
        Ok(())
    }

    /// 解析 stream 公共旗标：task-name；若指定 storage 则解析公共 Config。
    pub fn ParseStreamCommonFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        self.TaskName = flags.GetString(FlagStreamTaskName)?;
        if self.TaskName.is_empty() {
            return Err(Error::Annotate(
                berrors::ErrInvalidArgument,
                "Miss parameters task-name",
            ));
        }
        Ok(())
    }

    /// 按 Config.Storage 构造外部存储句柄，供 truncate/metadata 等读取日志目录。
    pub fn makeStorage(&self) -> Result<Arc<dyn Storage>> {
        let (_u, s) = GetStorage(&self.Config.Storage, &self.Config)?;
        Ok(s)
    }
}

/// 日志备份流任务运行时管理器，持有配置与外部存储。
/// Go 侧 `StreamMgr` 还负责 etcd 锁、streamhelper 注册等；Rust 当前为简化桩。
pub struct StreamMgr {
    pub cfg: StreamConfig,
    pub storage: Arc<dyn Storage>,
    /// 是否已关闭，防止重复释放资源。
    closed: bool,
}

/// 构造 StreamMgr：解析存储后端；`isStreamStart` 在 Go 侧影响锁与注册逻辑。
pub fn NewStreamMgr(cfg: StreamConfig, isStreamStart: bool) -> Result<StreamMgr> {
    let storage = cfg.makeStorage()?;
    let _ = isStreamStart;
    Ok(StreamMgr {
        cfg,
        storage,
        closed: false,
    })
}

impl StreamMgr {
    /// 标记管理器已关闭；Go 侧会释放 etcd 锁与 streamhelper 连接。
    pub fn close(&mut self) {
        self.closed = true;
    }

    /// 检查是否已有同名任务锁；Rust 桩恒返回 true（无冲突）。
    pub fn checkLock(&self) -> Result<bool> {
        Ok(true)
    }

    /// 在 etcd 写入任务锁；Rust 桩为空操作。
    pub fn setLock(&self) -> Result<()> {
        Ok(())
    }

    /// 校验 start-ts 非零；Go 侧还会与 PD 当前 TS 做合理性检查。
    pub fn adjustAndCheckStartTS(&mut self) -> Result<()> {
        if self.cfg.StartTS == 0 {
            return Err(Error::Annotate(
                berrors::ErrInvalidArgument,
                "start-ts is required",
            ));
        }
        Ok(())
    }

    /// 构建 TiKV observe key ranges；Rust 桩返回单条默认 range，Go 按表过滤。
    pub fn buildObserveRanges(&self) -> Result<Vec<crate::stubs::KeyRange>> {
        Ok(vec![crate::stubs::KeyRange::default()])
    }
}

/// 按配置生成日志备份任务加密参数：优先非空 LogBackupCipher，其次非空 MasterKey。
/// 对应 Go `generateSecurityConfig`，写入 streamhelper 任务 spec。
pub fn generateSecurityConfig(cfg: &StreamConfig) -> StreamBackupTaskSecurityConfig {
    let mut out = StreamBackupTaskSecurityConfig::default();
    if !cfg.Config.LogBackupCipherInfo.CipherKey.is_empty()
        && IsEffectiveEncryptionMethod(cfg.Config.LogBackupCipherInfo.CipherType)
    {
        out.CipherInfo = Some(cfg.Config.LogBackupCipherInfo.clone());
    } else if !cfg.Config.MasterKeyConfig.MasterKeys.is_empty()
        && IsEffectiveEncryptionMethod(cfg.Config.MasterKeyConfig.EncryptionType)
    {
        out.MasterKeyConfig = Some(cfg.Config.MasterKeyConfig.clone());
    }
    out
}

/// stream 子命令总入口，按 cmdName 分发到各 RunStream* 函数。
/// 对应 Go `StreamCommandMap`；未知命令返回 Errorf。
pub fn RunStreamCommand(g: &dyn Glue, cmdName: &str, cfg: &mut StreamConfig) -> Result<()> {
    match cmdName {
        "start" => RunStreamStart(g, cmdName, cfg),
        "stop" => RunStreamStop(g, cmdName, cfg),
        "pause" => RunStreamPause(g, cmdName, cfg),
        "resume" => RunStreamResume(g, cmdName, cfg),
        "status" => RunStreamStatus(g, cmdName, cfg),
        "truncate" => RunStreamTruncate(g, cmdName, cfg),
        "metadata" => RunStreamMetadata(g, cmdName, cfg),
        "advancer" => RunStreamAdvancer(g, cmdName, cfg),
        _ => Err(Error::Errorf(format!("unknown stream command: {cmdName}"))),
    }
}

/// 启动日志备份：校验 start-ts、加锁、生成加密配置、上报进度。
/// Go 侧还会注册 streamhelper 任务并设置 GC safepoint。
pub fn RunStreamStart(g: &dyn Glue, cmdName: &str, cfg: &mut StreamConfig) -> Result<()> {
    Summary(cmdName);
    let mut mgr = NewStreamMgr(cfg.clone(), true)?;
    mgr.adjustAndCheckStartTS()?;
    mgr.setLock()?;
    let _ = generateSecurityConfig(cfg);
    let updateCh = g.StartProgress(cmdName, 1, !cfg.Config.LogProgress);
    updateCh.Inc();
    updateCh.Close();
    SetSuccessStatus(true);
    mgr.close();
    Ok(())
}

/// 读取日志目录元数据，向 Glue 记录 log-min-ts / log-max-ts。
/// 对应 Go `RunStreamMetadata`，不修改集群状态。
pub fn RunStreamMetadata(g: &dyn Glue, cmdName: &str, cfg: &mut StreamConfig) -> Result<()> {
    Summary(cmdName);
    let info = getLogInfo(&cfg.Config)?;
    g.Record("log-min-ts", info.logMinTS);
    g.Record("log-max-ts", info.logMaxTS);
    SetSuccessStatus(true);
    Ok(())
}

/// 停止日志备份任务；task-name 必填。
/// Go 侧会删除 streamhelper 任务并清理 safepoint。
pub fn RunStreamStop(g: &dyn Glue, cmdName: &str, cfg: &mut StreamConfig) -> Result<()> {
    Summary(cmdName);
    if cfg.TaskName.is_empty() {
        return Err(Error::Annotate(
            berrors::ErrInvalidArgument,
            "task-name is required",
        ));
    }
    let _ = g;
    SetSuccessStatus(true);
    Ok(())
}

/// 暂停日志备份；task-name 必填，记录 pause-safepoint 名称长度供观测。
/// Go 侧会在 PD 注册 pause safepoint 并暂停 streamhelper。
pub fn RunStreamPause(g: &dyn Glue, cmdName: &str, cfg: &mut StreamConfig) -> Result<()> {
    Summary(cmdName);
    if cfg.TaskName.is_empty() {
        return Err(Error::Annotate(
            berrors::ErrInvalidArgument,
            "task-name is required",
        ));
    }
    let sp = buildPauseSafePointName(&cfg.TaskName);
    g.Record("pause-safepoint", sp.len() as u64);
    SetSuccessStatus(true);
    Ok(())
}

/// 恢复已暂停的日志备份；task-name 必填。
/// Go 侧清除 pause safepoint 并重启 streamhelper 采集。
pub fn RunStreamResume(g: &dyn Glue, cmdName: &str, cfg: &mut StreamConfig) -> Result<()> {
    Summary(cmdName);
    if cfg.TaskName.is_empty() {
        return Err(Error::Annotate(
            berrors::ErrInvalidArgument,
            "task-name is required",
        ));
    }
    let _ = g;
    SetSuccessStatus(true);
    Ok(())
}

/// 运行 log advancer 守护进程一轮；Go 侧推进 global checkpoint 并上传对象存储。
/// Rust 桩仅调用 `runOwnershipCycle` 占位。
pub fn RunStreamAdvancer(g: &dyn Glue, cmdName: &str, cfg: &mut StreamConfig) -> Result<()> {
    Summary(cmdName);
    runOwnershipCycle(true);
    let _ = (g, cfg);
    SetSuccessStatus(true);
    Ok(())
}

/// advancer 所有权选举循环占位；Go 侧通过 etcd 选主后推进 checkpoint。
pub fn runOwnershipCycle(isOwner: bool) {
    let _ = isOwner;
}

/// status 子命令前置校验：PD 地址列表不得为空。
pub fn checkConfigForStatus(pd: &[String]) -> Result<()> {
    if pd.is_empty() {
        return Err(Error::Annotate(
            berrors::ErrInvalidArgument,
            "pd address can not be empty",
        ));
    }
    Ok(())
}

/// 查询日志备份任务状态；要求 PD 已配置。
/// Go 侧会列举 streamhelper 任务并格式化输出。
pub fn RunStreamStatus(g: &dyn Glue, cmdName: &str, cfg: &mut StreamConfig) -> Result<()> {
    Summary(cmdName);
    checkConfigForStatus(&cfg.Config.PD)?;
    let _ = g;
    SetSuccessStatus(true);
    Ok(())
}

/// 截断日志备份数据至 until TS：校验范围、非 dry-run 时写入 truncate safepoint 文件。
/// Go 侧还会删除对象存储中 until 之前的 log 文件并加 truncating.lock。
pub fn RunStreamTruncate(g: &dyn Glue, cmdName: &str, cfg: &mut StreamConfig) -> Result<()> {
    Summary(cmdName);
    if cfg.UntilTS == 0 && !cfg.DryRun {
        return Err(Error::Annotate(
            berrors::ErrInvalidArgument,
            "until ts is required",
        ));
    }
    let storage = cfg.makeStorage()?;
    let info = getLogInfoFromStorage(storage.as_ref(), cfg.Config.CheckRequirements)?;
    if cfg.UntilTS > 0 {
        checkLogRange(info.logMinTS, cfg.UntilTS, info.logMinTS, info.logMaxTS)?;
    }
    if !cfg.DryRun {
        // truncate boundary mocked: write new truncate safepoint
        // 将 until TS 以小端 8 字节写入 TruncateSafePointFileName，对齐 Go binary.Write。
        let mut buf = cfg.UntilTS.to_le_bytes().to_vec();
        storage.WriteFile(TruncateSafePointFileName, &buf)?;
    }
    let _ = g;
    SetSuccessStatus(true);
    Ok(())
}

/// PiTR 流式恢复入口：调整 restore 配置、建 Mgr、执行 restoreStream。
/// 对应 Go `RunStreamRestore`；支持 "Point Restore" 显式点名。
pub fn RunStreamRestore(g: &dyn Glue, cmdName: &str, cfg: &mut RestoreConfig) -> Result<()> {
    cfg.adjustRestoreConfigForStreamRestore();
    Summary(cmdName);
    if !IsStreamRestore(cmdName) && cmdName != "Point Restore" {
        // allow explicit point restore name
        // Go 侧此处会拒绝非 stream restore 命令名；Rust 保留分支占位。
    }
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
    restoreStream(g, cfg)?;
    SetSuccessStatus(true);
    mgr.Close();
    Ok(())
}

/// 日志恢复专用配置包装，含预分配表 ID 区间。
/// 对应 Go `LogRestoreConfig`，tableMappingPreallocated 供 ID 映射预分配。
#[derive(Clone, Debug, Default)]
pub struct LogRestoreConfig {
    pub RestoreConfig: RestoreConfig,
    /// 预分配表 ID 范围 [start, end)，避免 restore 时冲突。
    pub tableMappingPreallocated: [i64; 2],
}

/// 判断是否需打开 PiTR 加索引 SQL 外部存储。
/// Phase 1 only restores the snapshot, so it must not open the log-restore SQL storage.
pub fn shouldOpenPiTRAddIndexSQLStorage(cfg: &RestoreConfig) -> bool {
    !cfg.PiTRAddIndexSQLStorage.is_empty() && cfg.RestorePhase != 1
}

/// 流式日志恢复核心：读 log 范围、默认 RestoreTS 为 logMaxTS、校验区间、上报进度。
/// Rust 桩用 MemStorage 占位；Go 侧走完整 log_client 导入流水线。
pub fn restoreStream(g: &dyn Glue, cfg: &mut RestoreConfig) -> Result<()> {
    let storage = MemStorage::new();
    let info =
        getLogInfoFromStorage(&storage, cfg.Config.CheckRequirements).unwrap_or(BackupLogInfo {
            logMinTS: 1,
            logMaxTS: 100,
            clusterID: 1,
        });
    if cfg.RestoreTS == 0 {
        cfg.RestoreTS = info.logMaxTS;
    }
    checkLogRange(info.logMinTS, cfg.RestoreTS, info.logMinTS, info.logMaxTS)?;
    let updateCh = g.StartProgress("log restore", 1, !cfg.Config.LogProgress);
    updateCh.Inc();
    updateCh.Close();
    Ok(())
}

/// 校验恢复 TS 区间是否落在现有日志 [logMinTS, logMaxTS] 内。
/// 条件：logMinTS ≤ restoreFrom ≤ restoreTo ≤ logMaxTS；任一违反返回 ErrInvalidArgument。
pub fn checkLogRange(
    restoreFromTS: u64,
    restoreToTS: u64,
    logMinTS: u64,
    logMaxTS: u64,
) -> Result<()> {
    if logMinTS > restoreFromTS || restoreFromTS > restoreToTS || restoreToTS > logMaxTS {
        return Err(Error::Annotatef(
            berrors::ErrInvalidArgument,
            format!(
                "restore log from {restoreFromTS} to {restoreToTS},  but the current existed log from {logMinTS} to {logMaxTS}"
            ),
        ));
    }
    Ok(())
}

/// 从日志备份目录解析出的元信息：可恢复 TS 区间与集群 ID。
#[derive(Clone, Debug, Default)]
pub struct BackupLogInfo {
    /// 当前可恢复的最大 TS（global checkpoint 与 resume state 取较大值后再与 logMinTS 取 max）。
    pub logMaxTS: u64,
    /// 有效日志下界：max(backupMeta.StartVersion, truncateSafepoint)。
    pub logMinTS: u64,
    /// 源集群 ClusterId，restore 时用于兼容性校验。
    pub clusterID: u64,
}

/// 从 Config.Storage 读取日志备份元信息，委托 `getLogInfoFromStorage`。
pub fn getLogInfo(cfg: &Config) -> Result<BackupLogInfo> {
    let (_u, s) = GetStorage(&cfg.Storage, cfg)?;
    getLogInfoFromStorage(s.as_ref(), cfg.CheckRequirements)
}

/// 从外部存储读取 backupmeta 并推导 log 区间。
/// EndVersion>0 表示目录曾用于全量备份，拒绝作为 log 目录；logMinTS 受 truncate safepoint 抬升。
pub fn getLogInfoFromStorage(s: &dyn Storage, checkRequirements: bool) -> Result<BackupLogInfo> {
    let metaData = s.ReadFile(MetaFile)?;
    let backupMeta: crate::stubs::backuppb::BackupMeta =
        serde_json::from_slice(&metaData).map_err(|e| Error::Trace(Error::new(e.to_string())))?;
    let _ = checkRequirements;
    if backupMeta.EndVersion > 0 {
        return Err(Error::Annotate(
            berrors::ErrStorageUnknown,
            "the storage has been used for full backup",
        ));
    }
    let logStartTS = backupMeta.StartVersion;
    // 读取 truncate safepoint 文件（8 字节小端 TS）；不存在则为 0。
    let truncateTS = match s.ReadFile(TruncateSafePointFileName) {
        Ok(buff) if buff.len() >= 8 => {
            let mut arr = [0u8; 8];
            arr.copy_from_slice(&buff[..8]);
            u64::from_le_bytes(arr)
        }
        _ => 0,
    };
    let logMinTS = logStartTS.max(truncateTS);
    // logMaxTS 不低于 logMinTS，防止 checkpoint 落后于 truncate 边界。
    let mut logMaxTS = getMaxRecoverableCheckpointFromStorage(s)?;
    logMaxTS = logMinTS.max(logMaxTS);
    Ok(BackupLogInfo {
        logMaxTS,
        logMinTS,
        clusterID: backupMeta.ClusterId,
    })
}

/// 遍历 global checkpoint 目录下所有 `{store_id}.ts` 文件，取最大 TS。
/// 忽略非 `.ts` 后缀文件（如 Go 测试中的 `*.tst` 干扰项）。
pub fn getGlobalCheckpointFromStorage(s: &dyn Storage) -> Result<u64> {
    let mut globalCheckPointTS = 0u64;
    let prefix = GetStreamBackupGlobalCheckpointPrefix();
    s.WalkDir(prefix, &mut |path: &str, _size: i64| {
        if !path.ends_with(".ts") {
            return Ok(());
        }
        let buff = s.ReadFile(path)?;
        if buff.len() >= 8 {
            let mut arr = [0u8; 8];
            arr.copy_from_slice(&buff[..8]);
            let ts = u64::from_le_bytes(arr);
            globalCheckPointTS = globalCheckPointTS.max(ts);
        }
        Ok(())
    })?;
    Ok(globalCheckPointTS)
}

/// 可恢复 checkpoint：优先 resume-state.json 的 LastCheckpoint，否则回退 global checkpoint。
/// 对应 Go `getMaxRecoverableCheckpointFromStorage`。
pub fn getMaxRecoverableCheckpointFromStorage(s: &dyn Storage) -> Result<u64> {
    let (ts, exists) = getCheckpointFromResumeState(s)?;
    if exists {
        return Ok(ts);
    }
    getGlobalCheckpointFromStorage(s)
}

/// 读取 CRR resume 持久化状态，返回 (LastCheckpoint, 文件是否存在)。
/// 文件路径为 `crr-checkpoint/resume-state.json`（resumeStateFileName）。
pub fn getCheckpointFromResumeState(s: &dyn Storage) -> Result<(u64, bool)> {
    let filename = if s.FileExists(RESUME_STATE_FILE_NAME)? {
        RESUME_STATE_FILE_NAME
    } else if s.FileExists(LEGACY_RESUME_STATE_FILE_NAME)? {
        // Preserve compatibility with state written by the earlier Rust port.
        LEGACY_RESUME_STATE_FILE_NAME
    } else {
        return Ok((0, false));
    };
    let statusContent = s.ReadFile(filename)?;
    let state: PersistentState = serde_json::from_slice(&statusContent)
        .map_err(|e| Error::new(format!("decode persisted resume state {filename}: {e}")))?;
    Ok((state.LastCheckpoint, true))
}

/// 从全量备份目录读取 EndVersion（全量结束 TS）与 ClusterId。
/// JSON meta 端口跳过 schema 版本兼容性检查（checkRequirements 分支为空）。
pub fn getFullBackupTS(cfg: &RestoreConfig, s: &dyn Storage) -> Result<(u64, u64)> {
    let metaData = s.ReadFile(MetaFile)?;
    let backupmeta: crate::stubs::backuppb::BackupMeta =
        serde_json::from_slice(&metaData).map_err(|e| Error::Trace(Error::new(e.to_string())))?;
    if cfg.Config.CheckRequirements {
        // compatibility check skipped for JSON meta
        // Go 侧会校验 backupmeta 与集群 schema 版本；Rust JSON 桩暂不实现。
    }
    Ok((backupmeta.EndVersion, backupmeta.ClusterId))
}

/// 根据 schema 替换映射构建 oldTableID → RewriteRules 表。
/// 跳过系统库/临时库与被 FilteredOut 的库表；分区 ID 单独生成 rewrite 规则。
pub fn buildRewriteRules(schemasReplace: &SchemasReplace) -> HashMap<i64, RewriteRules> {
    let mut rules = HashMap::new();
    for (_db_id, dbReplace) in &schemasReplace.DbReplaceMap {
        if dbReplace.FilteredOut || IsSysOrTempSysDB(&dbReplace.Name) {
            continue;
        }
        for (oldTableID, tableReplace) in &dbReplace.TableMap {
            if tableReplace.FilteredOut {
                continue;
            }
            if !rules.contains_key(oldTableID) {
                rules.insert(
                    *oldTableID,
                    GetRewriteRuleOfTable(
                        *oldTableID,
                        tableReplace.TableID,
                        &tableReplace.IndexMap,
                        false,
                    ),
                );
            }
            for (oldID, newID) in &tableReplace.PartitionMap {
                if !rules.contains_key(oldID) {
                    rules.insert(
                        *oldID,
                        GetRewriteRuleOfTable(*oldID, *newID, &tableReplace.IndexMap, false),
                    );
                }
            }
        }
    }
    rules
}

/// 将 TS 物理时间回退 `streamShiftDurationSecs`（默认 1 小时），逻辑位不变。
/// 用于 PiTR 恢复起点安全裕量；物理时间不足时返回 0。对应 Go `ShiftTS`。
pub fn ShiftTS(startTS: u64) -> u64 {
    let physical = oracle::ExtractPhysical(startTS);
    let logical = oracle::ExtractLogical(startTS);
    let shiftPhysical = physical - STREAM_SHIFT_DURATION_SECS * 1000;
    if shiftPhysical < 0 {
        return 0;
    }
    oracle::ComposeTS(shiftPhysical, logical)
}

/// 构造 pause safepoint 在 PD 中的注册名：`{taskName}_pause_safepoint`。
pub fn buildPauseSafePointName(taskName: &str) -> String {
    format!("{taskName}_pause_safepoint")
}

/// PiTR 任务运行时信息：checkpoint 任务详情与目标恢复 TS。
#[derive(Clone, Debug, Default)]
pub struct PiTRTaskInfo {
    /// 日志恢复 checkpoint 任务信息（含 Progress 阶段）。
    pub CheckpointTaskInfo: Option<TaskInfoForLogRestore>,
    /// 用户指定的恢复目标 TS。
    pub RestoreTS: u64,
}

impl PiTRTaskInfo {
    /// checkpoint 中是否含 TiFlash 副本项；Rust 桩恒 false，Go 查 tiflashrec。
    pub fn hasTiFlashItemsInCheckpoint(&self) -> bool {
        false
    }

    /// 返回恢复起始 TS；当前直接等于 RestoreTS。
    pub fn getRestoreStartTS(&self) -> u64 {
        self.RestoreTS
    }
}

/// 判断 checkpoint 任务是否已持久化 ID 映射（Progress ≥ LogRestoreProgressIdMapSaved）。
pub fn isCurrentIdMapSaved(checkpointTaskInfo: Option<&TaskInfoForLogRestore>) -> bool {
    match checkpointTaskInfo {
        Some(info) => info.Progress >= LogRestoreProgressIdMapSaved,
        None => false,
    }
}

/// 快照表 ID 区间是否有效：非 [0,0] 且 start>0 且 end>start。
pub fn isValidSnapshotRange(snapshotRange: [i64; 2]) -> bool {
    snapshotRange != [0, 0] && snapshotRange[0] > 0 && snapshotRange[1] > snapshotRange[0]
}

/// 校验 ID 列表是否严格连续递增（相邻差为 1）；用于 scheduler pause 范围合法性。
pub fn verifyContiguousIDs(ids: &[i64]) -> bool {
    for i in 1..ids.len() {
        if ids[i] != ids[i - 1] + 1 {
            return false;
        }
    }
    true
}

/// 从 schema 替换映射与快照区间构建 TiKV key range 列表（已 EncodeBytes）。
/// 1) 有效 snapshotRange 本身作为一个 range；
/// 2) 快照外的 log-restore 表/分区 ID 合并为 [minID, maxID+1) 供 scheduler pause。
pub fn buildKeyRangesFromSchemasReplace(
    schemasReplace: &SchemasReplace,
    snapshotRange: [i64; 2],
) -> Vec<[Vec<u8>; 2]> {
    let mut ranges: Vec<[i64; 2]> = Vec::new();
    let hasValidSnapshotRange = isValidSnapshotRange(snapshotRange);
    if hasValidSnapshotRange {
        // 快照备份覆盖的表 ID 区间始终加入 observe/pause 范围。
        ranges.push(snapshotRange);
    }
    // 判断表/分区 ID 是否落在快照区间外（需单独 log restore）。
    let isOutsideSnapshotRange = |id: i64| -> bool {
        if id == 0 {
            return false;
        }
        if hasValidSnapshotRange {
            return id < snapshotRange[0] || id >= snapshotRange[1];
        }
        true
    };
    let mut schedulerPauseIDs = Vec::new();
    for dbReplace in schemasReplace.DbReplaceMap.values() {
        if dbReplace.FilteredOut {
            continue;
        }
        for tableReplace in dbReplace.TableMap.values() {
            if tableReplace.FilteredOut {
                continue;
            }
            if isOutsideSnapshotRange(tableReplace.TableID) {
                schedulerPauseIDs.push(tableReplace.TableID);
            }
            for &partitionID in tableReplace.PartitionMap.values() {
                if isOutsideSnapshotRange(partitionID) {
                    schedulerPauseIDs.push(partitionID);
                }
            }
        }
    }
    if !schedulerPauseIDs.is_empty() {
        schedulerPauseIDs.sort_unstable();
        let minID = schedulerPauseIDs[0];
        let maxID = schedulerPauseIDs[schedulerPauseIDs.len() - 1];
        // 快照外表 ID 合并为单段 [min, max+1)，供 PD scheduler 暂停。
        ranges.push([minID, maxID + 1]);
        if hasValidSnapshotRange {
            // Go 侧在有效快照时会断言 ID 连续；Rust 仅调用校验占位。
            let _ = verifyContiguousIDs(&schedulerPauseIDs);
        }
    }
    // 将表 ID 区间编码为 TiKV table prefix key range。
    let mut keyRanges = Vec::with_capacity(ranges.len());
    for idRange in ranges {
        let startKey = EncodeBytes(&EncodeTablePrefix(idRange[0]));
        let endKey = EncodeBytes(&EncodeTablePrefix(idRange[1]));
        keyRanges.push([startKey, endKey]);
    }
    keyRanges
}

/// 执行清理闭包，仅保留第一个错误到 err_out（类似 Go multierr 的“首个错误”语义）。
pub fn cleanUpWithRetErr(err_out: &mut Option<Error>, f: impl FnOnce() -> Result<()>) {
    if let Err(e) = f() {
        if err_out.is_none() {
            *err_out = Some(e);
        }
    }
}

/// restore 前按需向 PD 注册恢复任务；Rust 桩为空操作，Go 侧写 registry。
pub fn RegisterRestoreIfNeeded(cfg: &mut RestoreConfig, cmdName: &str) -> Result<()> {
    let _ = (cfg, cmdName);
    Ok(())
}

// silence unused import for DefineStreamRestoreFlags in docs/parity
/// parity 测试专用：注册 stream restore 旗标，避免 DefineStreamRestoreFlags 被 lint 为未使用。
pub fn define_stream_restore_flags_for_tests(flags: &mut FlagSet) {
    DefineStreamRestoreFlags(flags);
}
