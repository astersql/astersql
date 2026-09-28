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

//! Backup task configuration and helpers matching `br/pkg/task/backup.go`.
//!
//! BR 备份任务配置与执行逻辑：对齐 Go `br/pkg/task/backup.go`。
//! 负责 CLI 旗标定义/解析、并发与压缩默认值校正、不可变配置哈希、
//! 以及可注入 Mgr/BackupClient 的 `RunBackup` 控制流（含调度器恢复 defer）。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::common::{
    Config, FullBackupType, FullBackupTypeEBS, FullBackupTypeKV, GetKeepalive, NewMgr, TLSConfig,
    defaultCloudAPIConcurrency, flagCloudAPIConcurrency, flagConcurrency, flagFullBackupType,
    flagOperatorPausedGCAndSchedulers, flagSkipAWS, unlimited,
};
use crate::stubs::backuppb::{CipherInfo, CompressionType};
use crate::stubs::oracle;
use crate::stubs::{
    BackupClient, CollectInt, DefaultBRGCSafePointTTL, DefaultSchemaConcurrency, Error, FlagSet,
    Glue, KeyRange, MemBackupClient, MetaWriter, Mgr, RangesSentThreshold, Result,
    SetSuccessStatus, StorageOptions, Summary, UnitRange, berrors,
};

// —— CLI 旗标名：与 Go flag 常量字符串保持一致，供 Define/Parse 共用 ——
// timeago：相对当前时间回退，用于推导备份 TS。
pub const flagBackupTimeago: &str = "timeago";
// backupts：显式备份时间戳（TSO 或 datetime 字符串）。
pub const flagBackupTS: &str = "backupts";
// lastbackupts：增量起点；>0 时强制关闭 checkpoint。
pub const flagLastBackupTS: &str = "lastbackupts";
// SST 压缩算法名（lz4/snappy/zstd）。
pub const flagCompressionType: &str = "compression";
// 压缩级别；0 表示算法默认级别。
pub const flagCompressionLevel: &str = "compression-level";
// 隐藏旗标：备份前移除 PD 调度器，退出时恢复。
pub const flagRemoveSchedulers: &str = "remove-schedulers";
// 单次下发 region 范围上限，须 >0。
pub const flagRangeLimit: &str = "range-limit";
// 默认 true：跳过统计信息备份以提速。
pub const flagIgnoreStats: &str = "ignore-stats";
// 是否写 backupmeta v2 格式。
pub const flagUseBackupMetaV2: &str = "use-backupmeta-v2";
// 隐藏旗标：断点续传；与 lastbackupts 互斥。
pub const flagUseCheckpoint: &str = "use-checkpoint";
// 多租户 keyspace 名称，写入公共 Config。
pub const flagKeyspaceName: &str = "keyspace-name";
// 副本读标签，格式 key:value。
pub const flagReplicaReadLabel: &str = "replica-read-label";
// 表级 schema 并发，默认 DefaultSchemaConcurrency。
pub const flagTableConcurrency: &str = "table-concurrency";
// GC safepoint TTL，0 时 Adjust 填默认值。
pub const flagGCTTL: &str = "gcttl";
// EBS/卷备份专用：卷清单与进度文件路径。
pub const flagBackupVolumeFile: &str = "volume-file";
// 卷备份进度落盘路径，供外部观测。
pub const flagProgressFile: &str = "progress-file";

// 备份并发默认值与硬上限（与 Go 常量一致）。
pub const defaultBackupConcurrency: u32 = 4;
pub const maxBackupConcurrency: u32 = 256;

// 摘要日志用的命令显示名；isFullBackup 仅匹配 FullBackupCmd。
pub const FullBackupCmd: &str = "Full Backup";
pub const DBBackupCmd: &str = "Database Backup";
pub const TableBackupCmd: &str = "Table Backup";
pub const RawBackupCmd: &str = "Raw Backup";
pub const TxnBackupCmd: &str = "Txn Backup";

/// SST 压缩参数；UNKNOWN 时 Adjust 回落为 ZSTD。
#[derive(Clone, Debug, Default)]
pub struct CompressionConfig {
    // 对应 backuppb.CompressionType。
    pub CompressionType: CompressionType,
    // 传给底层压缩库的 level。
    pub CompressionLevel: i32,
}

/// 备份任务完整配置：公共 Config + 备份专用字段（含 EBS 扩展）。
#[derive(Clone, Debug, Default)]
pub struct BackupConfig {
    pub Config: Config,
    // 相对时间回退；负数在 Parse 阶段拒绝。
    pub TimeAgo: Duration,
    // 目标备份 TS；0 表示运行时取集群当前 TS。
    pub BackupTS: u64,
    // 增量起点；非 0 时禁用 UseCheckpoint。
    pub LastBackupTS: u64,
    // 保护备份 TS 不被 GC 的 TTL（秒级语义依 PD）。
    pub GCTTL: i64,
    pub RemoveSchedulers: bool,
    pub RangeLimit: i32,
    pub IgnoreStats: bool,
    pub UseBackupMetaV2: bool,
    pub UseCheckpoint: bool,
    pub ReplicaReadLabel: HashMap<String, String>,
    pub TableConcurrency: u32,
    pub CompressionConfig: CompressionConfig,
    // kv / ebs 等全备类型；仅当旗标存在时解析 EBS 相关字段。
    pub FullBackupType: FullBackupType,
    pub VolumeFile: String,
    // 跳过真实 AWS API，便于本地/模拟卷备份。
    pub SkipAWS: bool,
    pub CloudAPIConcurrency: u32,
    pub ProgressFile: String,
    // 运维已暂停 GC/调度时跳过 BR 侧暂停逻辑。
    pub SkipPauseGCAndScheduler: bool,
}

/// 向 FlagSet 注册备份专用旗标及默认值；部分 MarkHidden 与 Go 一致。
pub fn DefineBackupFlags(flags: &mut FlagSet) {
    flags.DefineDuration(flagBackupTimeago, Duration::ZERO);
    flags.DefineUint64(flagLastBackupTS, 0);
    flags.DefineString(flagBackupTS, "");
    flags.DefineInt64(flagGCTTL, DefaultBRGCSafePointTTL);
    // 默认 zstd，与 Go DefineBackupFlags 相同。
    flags.DefineString(flagCompressionType, "zstd");
    flags.DefineInt32(flagCompressionLevel, 0);
    flags.DefineUint32(flagConcurrency, 4);
    flags.DefineUint(flagTableConcurrency, DefaultSchemaConcurrency as u64);
    flags.DefineBool(flagRemoveSchedulers, false);
    let _ = flags.MarkHidden(flagRemoveSchedulers);
    flags.DefineInt(flagRangeLimit, RangesSentThreshold as i64);
    // ignore-stats 默认 true，减少元数据开销。
    flags.DefineBool(flagIgnoreStats, true);
    flags.DefineBool(flagUseBackupMetaV2, true);
    flags.DefineString(flagKeyspaceName, "");
    flags.DefineBool(flagUseCheckpoint, true);
    let _ = flags.MarkHidden(flagUseCheckpoint);
    flags.DefineString(flagReplicaReadLabel, "");
}

impl BackupConfig {
    /// 从旗标填充自身；`skipCommonConfig` 为真时跳过公共 Config 解析（默认配置工厂用）。
    pub fn ParseFromFlags(&mut self, flags: &FlagSet, skipCommonConfig: bool) -> Result<()> {
        let timeAgo = flags.GetDuration(flagBackupTimeago)?;
        // Go 同样禁止 negative timeago。
        if timeAgo.as_secs_f64() < 0.0 {
            return Err(Error::Annotate(
                berrors::ErrInvalidArgument,
                "negative timeago is not allowed",
            ));
        }
        self.TimeAgo = timeAgo;
        self.LastBackupTS = flags.GetUint64(flagLastBackupTS)?;
        let backupTS = flags.GetString(flagBackupTS)?;
        // Parse 阶段 tzCheck=false；严格时区校验留给调用方另传 true。
        self.BackupTS = ParseTSString(&backupTS, false)?;
        self.UseBackupMetaV2 = flags.GetBool(flagUseBackupMetaV2)?;
        self.UseCheckpoint = flags.GetBool(flagUseCheckpoint)?;
        // 增量备份与 checkpoint 互斥：跨备份点续传无意义。
        if self.LastBackupTS > 0 {
            self.UseCheckpoint = false;
        }
        self.GCTTL = flags.GetInt64(flagGCTTL)?;
        self.Config.Concurrency = flags.GetUint32(flagConcurrency)?;
        self.TableConcurrency = flags.GetUint(flagTableConcurrency)?;
        self.CompressionConfig = parseCompressionFlags(flags)?;
        if !skipCommonConfig {
            self.Config.ParseFromFlags(flags)?;
        }
        self.RemoveSchedulers = flags.GetBool(flagRemoveSchedulers)?;
        self.RangeLimit = flags.GetInt(flagRangeLimit)?;
        if self.RangeLimit <= 0 {
            return Err(Error::Errorf(
                "the parameter `--range-limit` should be larger than 0",
            ));
        }
        self.IgnoreStats = flags.GetBool(flagIgnoreStats)?;
        self.Config.KeyspaceName = flags.GetString(flagKeyspaceName)?;
        // 仅当 CLI 注册了 full-backup-type 时才读 EBS/云 API 相关旗标。
        if flags.Lookup(flagFullBackupType).is_some() {
            let fullBackupType = flags.GetString(flagFullBackupType)?;
            let t = FullBackupType(fullBackupType);
            if !t.Valid() {
                return Err(Error::new("invalid full backup type"));
            }
            self.FullBackupType = t;
            self.SkipAWS = flags.GetBool(flagSkipAWS)?;
            self.CloudAPIConcurrency = flags.GetUint(flagCloudAPIConcurrency)?;
            self.VolumeFile = flags.GetString(flagBackupVolumeFile).unwrap_or_default();
            self.ProgressFile = flags.GetString(flagProgressFile).unwrap_or_default();
            self.SkipPauseGCAndScheduler = flags
                .GetBool(flagOperatorPausedGCAndSchedulers)
                .unwrap_or(false);
        }
        self.ReplicaReadLabel = parseReplicaReadLabelFlag(flags)?;
        Ok(())
    }

    /// 校正并发/限速/压缩/云 API 默认值；限速非 unlimited 时强制并发为 1。
    pub fn Adjust(&mut self) {
        self.Config.adjust();
        let mut usingDefaultConcurrency = false;
        if self.Config.Concurrency == 0 {
            self.Config.Concurrency = defaultBackupConcurrency;
            usingDefaultConcurrency = true;
        }
        // 防止用户把并发开得过大打爆集群。
        if self.Config.Concurrency > maxBackupConcurrency {
            self.Config.Concurrency = maxBackupConcurrency;
        }
        // 有速率限制时 Go 侧同样把并发压到 1，保证限速可预期。
        if self.Config.RateLimit != unlimited {
            let _ = usingDefaultConcurrency;
            self.Config.Concurrency = 1;
        }
        if self.GCTTL == 0 {
            self.GCTTL = DefaultBRGCSafePointTTL;
        }
        if self.CompressionConfig.CompressionType == CompressionType::UNKNOWN {
            self.CompressionConfig.CompressionType = CompressionType::ZSTD;
        }
        if self.CloudAPIConcurrency == 0 {
            self.CloudAPIConcurrency = defaultCloudAPIConcurrency;
        }
    }

    /// 对「不可变」子集做 JSON 序列化再 SHA256，供 checkpoint 校验配置未漂移。
    pub fn Hash(&self) -> Result<Vec<u8>> {
        // 字段集与 Go ImmutableBackupConfig 对齐；改集合会破坏续传兼容。
        #[derive(Serialize)]
        struct ImmutableBackupConfig<'a> {
            #[serde(rename = "last-backup-ts")]
            LastBackupTS: u64,
            #[serde(rename = "ignore-stats")]
            IgnoreStats: bool,
            #[serde(rename = "use-checkpoint")]
            UseCheckpoint: bool,
            #[serde(flatten)]
            backend_options: &'a crate::stubs::BackendOptions,
            storage: &'a str,
            pd: &'a [String],
            #[serde(rename = "send-credentials-to-tikv")]
            SendCreds: bool,
            #[serde(rename = "no-credentials")]
            NoCreds: bool,
            #[serde(rename = "filter-strings")]
            FilterStr: &'a [String],
            cipher: &'a CipherInfo,
            #[serde(rename = "keyspace-name")]
            KeyspaceName: &'a str,
        }
        let config = ImmutableBackupConfig {
            LastBackupTS: self.LastBackupTS,
            IgnoreStats: self.IgnoreStats,
            UseCheckpoint: self.UseCheckpoint,
            backend_options: &self.Config.BackendOptions,
            storage: &self.Config.Storage,
            pd: &self.Config.PD,
            SendCreds: self.Config.SendCreds,
            NoCreds: self.Config.NoCreds,
            FilterStr: &self.Config.FilterStr,
            cipher: &self.Config.CipherInfo,
            KeyspaceName: &self.Config.KeyspaceName,
        };
        let data =
            serde_json::to_vec(&config).map_err(|e| Error::Trace(Error::new(e.to_string())))?;
        Ok(Sha256::digest(&data).to_vec())
    }
}

/// 从旗标解析压缩类型与级别。
pub fn parseCompressionFlags(flags: &FlagSet) -> Result<CompressionConfig> {
    let compressionStr = flags.GetString(flagCompressionType)?;
    let compressionType = parseCompressionType(&compressionStr)?;
    let level = flags.GetInt32(flagCompressionLevel)?;
    Ok(CompressionConfig {
        CompressionLevel: level,
        CompressionType: compressionType,
    })
}

/// 是否全量备份命令（摘要文案/分支用）。
pub fn isFullBackup(cmdName: &str) -> bool {
    cmdName == FullBackupCmd
}

/// Injectable backup runner preserving Go `RunBackup` control flow.
/// 可注入 Mgr/Client 的备份入口：先 Adjust+Summary，再按需 RemoveSchedulers 并 defer 恢复。
pub fn RunBackup(
    g: &dyn Glue,
    cmdName: &str,
    cfg: &mut BackupConfig,
    mgr: Arc<dyn Mgr>,
    client: &dyn BackupClient,
) -> Result<()> {
    struct MgrCloseGuard<'a>(&'a dyn Mgr);
    impl Drop for MgrCloseGuard<'_> {
        fn drop(&mut self) {
            self.0.Close();
        }
    }

    // Go owns the manager created by RunBackup and always closes it via defer.
    let _mgr_close = MgrCloseGuard(mgr.as_ref());
    cfg.Adjust();
    // 初始化摘要收集器上下文（与 Go Summary(cmdName) 对齐）。
    Summary(cmdName);
    let _keepalive = GetKeepalive(&cfg.Config);
    let _tls = TLSConfig::default();

    run_backup_body(g, cmdName, cfg, mgr.as_ref(), client)
}

/// Match the Go deferred cancellation/deletion, including checkpoint retries.
struct BackupGCGuard {
    manager: Arc<dyn astersql_br_pkg_gc::Manager>,
    sp: astersql_br_pkg_gc::BRServiceSafePoint,
    cancel: Box<dyn Fn() + Send + Sync>,
    retain_on_failure: bool,
    complete: bool,
}
impl Drop for BackupGCGuard {
    fn drop(&mut self) {
        // RunBackup's outer Go context is cancelled on every return, even
        // when checkpoint recovery retains the barrier until its TTL expires.
        (self.cancel)();
        if self.retain_on_failure && !self.complete {
            return;
        }
        if let Err(error) = self
            .manager
            .DeleteServiceSafePoint(&astersql_br_pkg_gc::Context::Background(), self.sp.clone())
        {
            eprintln!("failed to remove service safe point: {error}");
        }
    }
}

/// 备份主体：取 TS → 构造 BackupRequest → BackupRanges → 写 meta → 标记成功。
fn run_backup_body(
    g: &dyn Glue,
    cmdName: &str,
    cfg: &BackupConfig,
    mgr: &dyn Mgr,
    client: &dyn BackupClient,
) -> Result<()> {
    let clusterVersion = mgr.GetClusterVersion()?;
    let brVersion = g.GetVersion();
    // 用全区间 region 数估进度条总量。
    let approximateRegions = mgr.GetRegionCount(&[], &[])?;
    CollectInt("backup total regions", approximateRegions as i64);
    let updateCh = g.StartProgress(cmdName, approximateRegions as i64, !cfg.Config.LogProgress);
    // BackupTS==0 时实时取 PD/TiKV 当前 TS。
    let backupTS = if cfg.BackupTS != 0 {
        cfg.BackupTS
    } else {
        client.GetCurrentTS()?
    };
    g.Record("BackupTS", backupTS);

    if cfg.LastBackupTS > 0 && backupTS <= cfg.LastBackupTS {
        return Err(Error::Annotate(
            berrors::ErrInvalidArgument,
            "LastBackupTS is larger or equal to current TS",
        ));
    }

    // Register before reading any backup ranges. A keyspace backup must not
    // silently fall back to an unprotected snapshot when its adapter is absent.
    let mut gc_guard = if let Some(manager) = mgr.GetGCManager() {
        let (ctx, cancel) = astersql_br_pkg_gc::Context::WithCancel();
        let sp = astersql_br_pkg_gc::BRServiceSafePoint {
            ID: client.GetSafePointID(),
            TTL: cfg.GCTTL,
            BackupTS: if cfg.LastBackupTS > 0 {
                cfg.LastBackupTS
            } else {
                backupTS
            },
        };
        let guard = BackupGCGuard {
            manager: manager.clone(),
            sp: sp.clone(),
            cancel: Box::new(cancel),
            retain_on_failure: cfg.UseCheckpoint,
            complete: false,
        };
        astersql_br_pkg_gc::StartServiceSafePointKeeper(&ctx, sp, manager)
            .map_err(|error| Error::new(error.to_string()))?;
        Some(guard)
    } else {
        if !cfg.Config.KeyspaceName.is_empty() {
            return Err(Error::new("keyspace backup requires a GC manager"));
        }
        None
    };

    struct SchedulerGuard(crate::stubs::RestoreSchedulers);
    impl Drop for SchedulerGuard {
        fn drop(&mut self) {
            if let Err(error) = (self.0)() {
                eprintln!("failed to restore schedulers: {error}");
            }
        }
    }
    let _schedulers = if cfg.RemoveSchedulers {
        Some(SchedulerGuard(mgr.RemoveSchedulers()?))
    } else {
        None
    };

    if let Some(storage_backend) = client.GetStorageBackend() {
        client.SetStorageAndCheckNotInUse(
            &storage_backend,
            &StorageOptions {
                NoCredentials: cfg.Config.NoCreds,
                SendCredentials: cfg.Config.SendCreds,
                CheckS3ObjectLockOptions: true,
            },
        )?;
    }

    // StartVersion=LastBackupTS（增量起点），EndVersion=本次 backupTS。
    let req = crate::stubs::backuppb::BackupRequest {
        ClusterId: client.GetClusterID(),
        StartVersion: cfg.LastBackupTS,
        EndVersion: backupTS,
        RateLimit: cfg.Config.RateLimit,
        Concurrency: cfg.Config.Concurrency,
        StorageBackend: client.GetStorageBackend(),
        CompressionType: cfg.CompressionConfig.CompressionType,
        CompressionLevel: cfg.CompressionConfig.CompressionLevel,
        CipherInfo: Some(cfg.Config.CipherInfo.clone()),
        ..Default::default()
    };
    let ranges =
        client.BuildBackupRanges(&cfg.Config.FilterStr, backupTS, isFullBackup(cmdName))?;
    if !ranges.is_empty() {
        let _ = client.BackupRanges(&ranges, &req)?;
    }
    updateCh.Close();

    // 异步写 meta 片段，再 Flush 备份元数据并记录归档大小。
    let metaWriter = MetaWriter::new();
    metaWriter.StartWriteMetasAsync();
    metaWriter.Update(|m| {
        m.StartVersion = req.StartVersion;
        m.EndVersion = req.EndVersion;
        m.ClusterId = req.ClusterId;
        m.ClusterVersion = clusterVersion;
        m.BrVersion = brVersion;
        m.ApiVersion = client.GetApiVersion();
    });
    metaWriter.FinishWriteMetas()?;
    metaWriter.FlushBackupMeta()?;
    // 归档字节数写入摘要，供 Summary 成功模板输出。
    g.Record(crate::stubs::BackupDataSize, metaWriter.ArchiveSize());
    if let Some(guard) = gc_guard.as_mut() {
        guard.complete = true;
    }
    // 标记全局成功，供后续 Succeed()/摘要使用。
    SetSuccessStatus(true);
    // 保留 NewMgr 符号引用，避免与 Go 侧依赖漂移时误删导入。
    let _ = NewMgr;
    Ok(())
}

/// 解析备份 TS：空→0；纯数字→TSO；否则按 datetime→毫秒→oracle.GoTimeToTS。
pub fn ParseTSString(ts: &str, tzCheck: bool) -> Result<u64> {
    if ts.is_empty() {
        return Ok(0);
    }
    // 优先按 TSO 数值解析，与 Go 行为一致。
    if let Ok(tso) = ts.parse::<u64>() {
        return Ok(tso);
    }
    if tzCheck {
        // Require timezone offset when datetime format is used.
        // 日期里已有两个 '-'，再出现第三个 '-' 或 '+' 才视为带时区。
        let has_tz = ts.ends_with('Z') || ts.contains('+') || ts.matches('-').count() >= 3;
        if !has_tz {
            return Err(Error::Errorf(
                "must set timezone when using datetime format ts, e.g. '2018-05-11 01:42:23+0800'",
            ));
        }
    }
    // Parse civil time precisely and preserve an explicitly supplied UTC offset.
    let normalized = ts.replace(' ', "T");
    let parsed = parse_datetime_millis(&normalized)
        .ok_or_else(|| Error::new(format!("failed to parse backup ts '{ts}'")))?;
    Ok(oracle::GoTimeToTS(parsed))
}

/// Parse a civil datetime into Unix milliseconds without an extra date-time dependency.
fn parse_datetime_millis(s: &str) -> Option<i64> {
    let (core, offset_seconds) = if let Some(core) = s.strip_suffix('Z') {
        (core, Some(0_i64))
    } else {
        let tz_at = s
            .char_indices()
            .skip_while(|(idx, _)| *idx <= 10)
            .find_map(|(idx, ch)| matches!(ch, '+' | '-').then_some(idx));
        if let Some(idx) = tz_at {
            let sign = if s.as_bytes()[idx] == b'+' {
                1_i64
            } else {
                -1_i64
            };
            let digits = s[idx + 1..].replace(':', "");
            if !matches!(digits.len(), 2 | 4) || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let hour = digits[..2].parse::<i64>().ok()?;
            let minute = if digits.len() == 4 {
                digits[2..].parse::<i64>().ok()?
            } else {
                0
            };
            if hour > 14 || minute >= 60 || (hour == 14 && minute != 0) {
                return None;
            }
            (&s[..idx], Some(sign * (hour * 3600 + minute * 60)))
        } else {
            (s, None)
        }
    };
    let parts: Vec<&str> = core.split('T').collect();
    if parts.len() != 2 {
        return None;
    }
    let d: Vec<i32> = parts[0]
        .split('-')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()
        .ok()?;
    let t: Vec<&str> = parts[1].split(':').collect();
    if d.len() != 3 || t.len() != 3 {
        return None;
    }
    let (second_text, fraction) = t[2].split_once('.').unwrap_or((t[2], ""));
    let hour = t[0].parse::<i32>().ok()?;
    let minute = t[1].parse::<i32>().ok()?;
    let second = second_text.parse::<i32>().ok()?;
    let (year, month, day) = (d[0], d[1], d[2]);
    if !valid_civil_datetime(year, month, day, hour, minute, second)
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > 6
    {
        return None;
    }
    let millis = if fraction.is_empty() {
        0
    } else {
        format!("{fraction:0<3}")[..3].parse::<i64>().ok()?
    };
    let seconds = if let Some(offset) = offset_seconds {
        days_from_civil(year, month, day) * 86_400 + i64::from(hour * 3600 + minute * 60 + second)
            - offset
    } else {
        local_epoch_seconds(year, month, day, hour, minute, second)?
    };
    seconds.checked_mul(1000)?.checked_add(millis)
}

fn valid_civil_datetime(y: i32, m: i32, d: i32, hh: i32, mm: i32, ss: i32) -> bool {
    let leap = y.rem_euclid(4) == 0 && (y.rem_euclid(100) != 0 || y.rem_euclid(400) == 0);
    let max_day = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=max_day).contains(&d)
        && (0..24).contains(&hh)
        && (0..60).contains(&mm)
        && (0..60).contains(&ss)
}

fn days_from_civil(mut y: i32, m: i32, d: i32) -> i64 {
    y -= i32::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}

#[cfg(unix)]
fn local_epoch_seconds(y: i32, m: i32, d: i32, hh: i32, mm: i32, ss: i32) -> Option<i64> {
    use std::ffi::{c_char, c_int, c_long};
    #[repr(C)]
    struct Tm {
        tm_sec: c_int,
        tm_min: c_int,
        tm_hour: c_int,
        tm_mday: c_int,
        tm_mon: c_int,
        tm_year: c_int,
        tm_wday: c_int,
        tm_yday: c_int,
        tm_isdst: c_int,
        tm_gmtoff: c_long,
        tm_zone: *const c_char,
    }
    unsafe extern "C" {
        fn mktime(value: *mut Tm) -> c_long;
    }
    let mut value = Tm {
        tm_sec: ss,
        tm_min: mm,
        tm_hour: hh,
        tm_mday: d,
        tm_mon: m - 1,
        tm_year: y - 1900,
        tm_wday: 0,
        tm_yday: 0,
        tm_isdst: -1,
        tm_gmtoff: 0,
        tm_zone: std::ptr::null(),
    };
    Some(unsafe { mktime(&mut value) } as i64)
}

#[cfg(not(unix))]
fn local_epoch_seconds(y: i32, m: i32, d: i32, hh: i32, mm: i32, ss: i32) -> Option<i64> {
    Some(days_from_civil(y, m, d) * 86_400 + i64::from(hh * 3600 + mm * 60 + ss))
}

/// 用默认旗标生成 BackupConfig，再覆盖公共 Config（测试/默认工厂）。
pub fn DefaultBackupConfig(commonConfig: Config) -> BackupConfig {
    let mut fs = FlagSet::new();
    DefineBackupFlags(&mut fs);
    let mut cfg = BackupConfig::default();
    // skipCommonConfig=true：公共字段稍后用 commonConfig 整体替换。
    cfg.ParseFromFlags(&fs, true)
        .expect("failed to parse backup flags to config");
    cfg.Config = commonConfig;
    cfg
}

/// 映射压缩算法字符串；未知值返回 ErrInvalidArgument。
pub fn parseCompressionType(s: &str) -> Result<CompressionType> {
    match s {
        "lz4" => Ok(CompressionType::LZ4),
        "snappy" => Ok(CompressionType::SNAPPY),
        "zstd" => Ok(CompressionType::ZSTD),
        _ => Err(Error::Annotatef(
            berrors::ErrInvalidArgument,
            format!("invalid compression type '{s}'"),
        )),
    }
}

/// 解析 `key:value` 副本读标签；缺旗标或空串返回空 map。
pub fn parseReplicaReadLabelFlag(flags: &FlagSet) -> Result<HashMap<String, String>> {
    let replicaReadLabelStr = match flags.GetString(flagReplicaReadLabel) {
        Ok(s) => s,
        // 旗标未定义时视为未配置，而非错误。
        Err(_) => return Ok(HashMap::new()),
    };
    if replicaReadLabelStr.is_empty() {
        return Ok(HashMap::new());
    }
    let kv: Vec<&str> = replicaReadLabelStr.split(':').collect();
    // 必须恰好一段冒号分隔的键值。
    if kv.len() != 2 {
        return Err(Error::Annotatef(
            berrors::ErrInvalidArgument,
            format!("invalid replica read label '{replicaReadLabelStr}'"),
        ));
    }
    Ok(HashMap::from([(kv[0].to_string(), kv[1].to_string())]))
}

/// Convenience for tests that don't inject custom clients.
/// 测试便捷入口：默认 NewMgr + MemBackupClient，避免真实集群依赖。
pub fn RunBackupWithDefaults(g: &dyn Glue, cmdName: &str, cfg: &mut BackupConfig) -> Result<()> {
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
    // 固定 cluster_id/current_ts，便于断言 BackupRequest 版本字段。
    let client = MemBackupClient {
        cluster_id: 1,
        current_ts: 100,
        ..Default::default()
    };
    RunBackup(g, cmdName, cfg, mgr, &client)
}
