// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! Advancer 运行环境抽象：聚合 TiKV 元数据、日志备份服务、流任务元数据与锁解决能力。
//! 与 Go `advancer_env.go` 对齐；生产路径用 PD/TiKV，测试用 FakeCluster 实现同一 `Env`。

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use crate::advancer_cliext::{AdvancerExt, TaskEvent};
use crate::client::MetaDataClient;
use crate::regioniter::{RegionWithLeader, Store, TiKVClusterMeta};
use crate::stubs::{LogBackupClient, LogBackupService};

/// 日志备份协调服务在 PD/TiKV 侧的标识。
pub const logBackupServiceID: &str = "log-backup-coordinator";
/// 服务安全点 TTL：阻塞 GC 时使用的保活时长（24h）。
pub const logBackupSafePointTTL: Duration = Duration::from_secs(24 * 3600);
/// 连接 Store 的拨号超时。
pub const dialTimeOut: Duration = Duration::from_secs(8);

/// 从 TiKV 配置读取 `log-backup.max-flush-interval`。
/// Advancer 用该间隔驱动 resolve-lock 与 try-advance 阈值。
pub trait LogBackupFlushIntervalGetter: Send + Sync {
    fn GetLogBackupFlushInterval(&self) -> Result<Duration, String>;
}

/// 流备份任务元数据：枚举任务、上传/读取/清除全局检查点、暂停任务。
/// 生产实现走 etcd；测试实现由 FakeEnv 提供内存表。
pub trait StreamMeta: Send + Sync {
    /// 启动时填充已有任务事件。
    fn Begin(&self, ch: &mut Vec<TaskEvent>) -> Result<(), String>;
    /// 单调上传任务全局检查点。
    fn UploadV3GlobalCheckpointForTask(
        &self,
        taskName: &str,
        checkpoint: u64,
    ) -> Result<(), String>;
    /// 读取任务当前全局检查点。
    fn GetGlobalCheckpointForTask(&self, taskName: &str) -> Result<u64, String>;
    /// 删除任务全局检查点键。
    fn ClearV3GlobalCheckpointForTask(&self, taskName: &str) -> Result<(), String>;
    /// 写入暂停标记。
    fn PauseTask(&self, taskName: &str) -> Result<(), String>;
}

/// 按 key 范围解决锁；`maxVersion` 限制扫描锁的版本上界。
/// ScanLock 遇到 locked 错误时可配合重试降低 maxVersion。
pub trait RegionLockResolver: Send + Sync {
    fn ResolveLocksForRange(
        &self,
        maxVersion: u64,
        startKey: &[u8],
        endKey: &[u8],
    ) -> Result<(), String>;
}

/// Advancer 所需的完整环境接口（Go `Env` 组合）。
/// 由 TiKV 集群元数据 + 日志备份 RPC + 流元数据 + 锁解决 + flush 间隔组成。
pub trait Env:
    TiKVClusterMeta + LogBackupService + StreamMeta + RegionLockResolver + LogBackupFlushIntervalGetter
{
}

impl<T> Env for T where
    T: TiKVClusterMeta
        + LogBackupService
        + StreamMeta
        + RegionLockResolver
        + LogBackupFlushIntervalGetter
{
}

/// 将 `AdvancerExt` 适配为 `StreamMeta`，供绑定 etcd 元数据的环境使用。
#[derive(Clone)]
pub struct AdvancerExtEnv {
    pub ext: AdvancerExt,
}

impl StreamMeta for AdvancerExtEnv {
    fn Begin(&self, ch: &mut Vec<TaskEvent>) -> Result<(), String> {
        self.ext.BeginSnapshot(ch)
    }
    fn UploadV3GlobalCheckpointForTask(
        &self,
        taskName: &str,
        checkpoint: u64,
    ) -> Result<(), String> {
        self.ext
            .UploadV3GlobalCheckpointForTask(taskName, checkpoint)
    }
    fn GetGlobalCheckpointForTask(&self, taskName: &str) -> Result<u64, String> {
        self.ext.GetGlobalCheckpointForTask(taskName)
    }
    fn ClearV3GlobalCheckpointForTask(&self, taskName: &str) -> Result<(), String> {
        self.ext.ClearV3GlobalCheckpointForTask(taskName)
    }
    fn PauseTask(&self, taskName: &str) -> Result<(), String> {
        // 暂停不附带额外 PauseV2 选项；与 Go 默认 PauseTask 一致。
        self.ext.meta.PauseTask(taskName, Vec::new())
    }
}

#[derive(Deserialize)]
struct TikvLogBackupSection {
    #[serde(rename = "max-flush-interval")]
    max_flush_interval: Option<String>,
}

#[derive(Deserialize)]
struct TikvConfigRoot {
    #[serde(rename = "log-backup")]
    log_backup: Option<TikvLogBackupSection>,
}

/// 从单份 TiKV JSON 配置解析 `log-backup.max-flush-interval`。
/// 缺失或解析为零时长视为无效配置。
pub fn parseLogBackupFlushIntervalFromConfig(resp: &[u8]) -> Result<Duration, String> {
    let c: TikvConfigRoot = serde_json::from_slice(resp).map_err(|e| e.to_string())?;
    let section = c
        .log_backup
        .ok_or_else(|| "log-backup.max-flush-interval is not found in TiKV config".to_string())?;
    let raw = section
        .max_flush_interval
        .ok_or_else(|| "log-backup.max-flush-interval is not found in TiKV config".to_string())?;
    let d = parse_go_duration(&raw)?;
    if d.is_zero() {
        return Err(format!("invalid log-backup.max-flush-interval {raw}"));
    }
    Ok(d)
}

/// 解析 Go 风格时长字符串（ns/us/ms/s/m/h），供 TiKV 配置字段使用。
fn parse_go_duration(s: &str) -> Result<Duration, String> {
    // Match Go time.ParseDuration: an optional sign followed by one or more
    // decimal number/unit pairs. The config contract below rejects non-positive
    // results, so a negative value can fail here without a signed duration type.
    if s.is_empty() {
        return Err("empty duration".into());
    }
    let mut rest = s;
    if let Some(sign) = rest.as_bytes().first() {
        if *sign == b'-' {
            return Err(format!("invalid duration {s}"));
        }
        if *sign == b'+' {
            rest = &rest[1..];
        }
    }
    if rest.is_empty() {
        return Err(format!("invalid duration {s}"));
    }
    if rest == "0" {
        return Ok(Duration::ZERO);
    }

    let mut total_nanos = 0u128;
    while !rest.is_empty() {
        let number_end = rest
            .char_indices()
            .take_while(|(_, ch)| ch.is_ascii_digit() || *ch == '.')
            .map(|(index, ch)| index + ch.len_utf8())
            .last()
            .ok_or_else(|| format!("invalid duration {s}"))?;
        let number = &rest[..number_end];
        if number.matches('.').count() > 1 {
            return Err(format!("invalid duration {s}"));
        }
        let (integer, fraction) = number.split_once('.').unwrap_or((number, ""));
        if integer.is_empty() && fraction.is_empty()
            || !integer.chars().all(|ch| ch.is_ascii_digit())
            || !fraction.chars().all(|ch| ch.is_ascii_digit())
        {
            return Err(format!("invalid duration {s}"));
        }

        let units = &rest[number_end..];
        let (unit, nanos_per_unit) = [
            ("ns", 1u128),
            ("us", 1_000),
            ("µs", 1_000),
            ("μs", 1_000),
            ("ms", 1_000_000),
            ("s", 1_000_000_000),
            ("m", 60_000_000_000),
            ("h", 3_600_000_000_000),
        ]
        .into_iter()
        .find(|(unit, _)| units.starts_with(unit))
        .ok_or_else(|| format!("invalid duration {s}"))?;

        let whole = if integer.is_empty() {
            0
        } else {
            integer
                .parse::<u128>()
                .map_err(|_| format!("duration overflow {s}"))?
        };
        let whole_nanos = whole
            .checked_mul(nanos_per_unit)
            .ok_or_else(|| format!("duration overflow {s}"))?;
        // Eighteen decimal places are enough to retain every fraction that can
        // contribute one nanosecond, even for hours; later digits are sub-ns.
        let precision = fraction.len().min(18);
        let fraction_nanos = if precision == 0 {
            0
        } else {
            fraction[..precision]
                .parse::<u128>()
                .map_err(|_| format!("invalid duration {s}"))?
                .checked_mul(nanos_per_unit)
                .and_then(|value| value.checked_div(10u128.pow(precision as u32)))
                .ok_or_else(|| format!("duration overflow {s}"))?
        };
        total_nanos = total_nanos
            .checked_add(whole_nanos)
            .and_then(|value| value.checked_add(fraction_nanos))
            .filter(|value| *value <= i64::MAX as u128)
            .ok_or_else(|| format!("duration overflow {s}"))?;
        rest = &units[unit.len()..];
    }

    Ok(Duration::from_nanos(total_nanos as u64))
}

/// 汇总多 Store 的 TiKV 配置，取最大 flush 间隔作为推进侧 resolve-lock 间隔参考。
/// Go 侧同样聚合各 Store 配置；空列表直接报错。
pub fn GetLogBackupFlushIntervalFromTiKVConfig(configs: &[Vec<u8>]) -> Result<Duration, String> {
    let mut max_flush = Duration::ZERO;
    let mut min_flush = Duration::ZERO;
    let mut store_count = 0usize;
    for resp in configs {
        let flush = parseLogBackupFlushIntervalFromConfig(resp)?;
        if store_count == 0 || flush < min_flush {
            min_flush = flush;
        }
        if flush > max_flush {
            max_flush = flush;
        }
        store_count += 1;
    }
    if store_count == 0 {
        return Err("no TiKV config found for log-backup.max-flush-interval".into());
    }
    // 返回最大值，使推进节奏不比最慢 Store 的 flush 更激进。
    Ok(max_flush)
}

/// PD Region 扫描器替身：包装 `TiKVClusterMeta`，供 BlockGC 等检查使用。
pub struct PDRegionScanner {
    pub meta: Arc<dyn TiKVClusterMeta>,
}

impl PDRegionScanner {
    /// 将服务安全点推进到 `at`，阻止 GC 越过该点。
    pub fn BlockGCUntil(&self, at: u64) -> Result<u64, String> {
        self.meta.BlockGCUntil(at)
    }
    /// 解除服务安全点阻塞。
    pub fn UnblockGC(&self) -> Result<(), String> {
        self.meta.UnblockGC()
    }
    /// 读取集群当前 TSO。
    pub fn FetchCurrentTS(&self) -> Result<u64, String> {
        self.meta.FetchCurrentTS()
    }
    /// 扫描 `[key, endKey)` 内的 Region（带 Leader）。
    pub fn RegionScan(
        &self,
        key: &[u8],
        endKey: &[u8],
        limit: i32,
    ) -> Result<Vec<RegionWithLeader>, String> {
        self.meta.RegionScan(key, endKey, limit)
    }
    /// 列出全部 Store。
    pub fn Stores(&self) -> Result<Vec<Store>, String> {
        self.meta.Stores()
    }
}

/// 由 `MetaDataClient` 构造仅绑定元数据的 `AdvancerExtEnv`。
pub fn NewMetaBoundEnv(meta: MetaDataClient) -> AdvancerExtEnv {
    AdvancerExtEnv {
        ext: AdvancerExt { meta },
    }
}

// 历史 collector 路径使用的服务 trait 别名再导出。
pub use crate::stubs::LogBackupClient as LogBackupClientTrait;
pub use crate::stubs::LogBackupService as LogBackupServiceTrait;

/// 共享日志备份客户端句柄类型别名。
pub type SharedLogBackupClient = Arc<dyn LogBackupClient>;
