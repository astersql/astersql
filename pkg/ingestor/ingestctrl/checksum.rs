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

// 导入后校验：本地 KVChecksum 与远端 ADMIN CHECKSUM / TiKV 扫描校验。
//
// 提供 TiDBChecksumExecutor（SQL ADMIN CHECKSUM）与 TiKVChecksumManager（按 TSO
// 扫描并维护 PD Service GC Safe Point）。GC：垃圾回收；Safe Point：不可再回收的时间戳下界；
// TSO：PD 分配的时间戳。

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::{CancellationToken, Error, KvPair, Result};

/// TiKV 校验获取 TSO / 扫描失败时的最大重试次数。
const MAX_ERROR_RETRY_COUNT: usize = 3;
/// DistSQL 扫描并发度下限。
pub const MinDistSQLScanConcurrency: usize = 4;
/// 默认 backoff 权重。
pub const DefaultBackoffWeight: i32 = 15;
/// 默认 tidb_gc_life_time 提升目标（100 小时）。
pub const DefaultGCLifeTime: Duration = Duration::from_secs(100 * 60 * 60);
/// Service GC Safe Point 的基础 TTL（秒），更新时会再乘 PRE_UPDATE 因子。
pub static serviceSafePointTTL: AtomicI64 = AtomicI64::new(10 * 60);

/// 本地累积的 KV 校验和：CRC64 异或、键值条数与字节数。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KVChecksum {
    pub checksum: u64,
    pub total_kvs: u64,
    pub total_bytes: u64,
}

impl KVChecksum {
    /// 用一对 KV 更新校验和与计数。
    pub fn update(&mut self, pair: &KvPair) {
        self.checksum ^= crc64(&pair.key, crc64(&pair.value, 0));
        self.total_kvs += 1;
        self.total_bytes += pair.size() as u64;
    }
}

/// 远端校验结果（Schema/Table + Checksum/TotalKVs/TotalBytes）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RemoteChecksum {
    pub Schema: String,
    pub Table: String,
    pub Checksum: u64,
    pub TotalKVs: u64,
    pub TotalBytes: u64,
}

impl RemoteChecksum {
    /// 与本地 KVChecksum 三项是否全部相等。
    pub fn IsEqual(&self, other: &KVChecksum) -> bool {
        self.Checksum == other.checksum
            && self.TotalKVs == other.total_kvs
            && self.TotalBytes == other.total_bytes
    }
}

/// 校验目标表信息：库名、表名、table_id 与索引 id 列表。
#[derive(Clone, Debug, Default)]
pub struct TableInfo {
    pub schema: String,
    pub table: String,
    pub table_id: i64,
    pub index_ids: Vec<i64>,
}

/// 远端校验管理器：对单表执行 Checksum，并支持 Close 释放资源。
pub trait ChecksumManager: Send + Sync {
    fn Checksum(&self, token: &CancellationToken, table: &TableInfo) -> Result<RemoteChecksum>;
    fn Close(&self);
}

/// 通过 SQL 查询校验和与读写 tidb_gc_life_time 的客户端。
pub trait SqlChecksumClient: Send + Sync {
    fn query_checksum(&self, sql: &str) -> Result<RemoteChecksum>;
    fn obtain_gc_lifetime(&self) -> Result<String>;
    fn update_gc_lifetime(&self, value: &str) -> Result<()>;
}

/// 基于 ADMIN CHECKSUM TABLE 的 TiDB 侧校验执行器，并在任务期间延长 GC lifetime。
pub struct TiDBChecksumExecutor {
    client: Arc<dyn SqlChecksumClient>,
    gc_lifetime: Arc<GCLifeTimeManager>,
}

/// 构造绑定 SqlChecksumClient 的 TiDBChecksumExecutor。
pub fn NewTiDBChecksumExecutor(client: Arc<dyn SqlChecksumClient>) -> TiDBChecksumExecutor {
    TiDBChecksumExecutor {
        client,
        gc_lifetime: Arc::new(GCLifeTimeManager::default()),
    }
}

impl ChecksumManager for TiDBChecksumExecutor {
    fn Checksum(&self, token: &CancellationToken, table: &TableInfo) -> Result<RemoteChecksum> {
        token.check()?;
        self.gc_lifetime.addOneJob(self.client.as_ref())?;
        // 反引号转义后拼 ADMIN CHECKSUM TABLE。
        let sql = format!(
            "ADMIN CHECKSUM TABLE `{}`.`{}`",
            table.schema.replace('`', "``"),
            table.table.replace('`', "``")
        );
        let result = self.client.query_checksum(&sql);
        self.gc_lifetime.removeOneJob(self.client.as_ref());
        result
    }

    fn Close(&self) {}
}

/// 引用计数式管理 tidb_gc_life_time：首个任务可能抬高，末个任务恢复原值。
#[derive(Default)]
pub struct GCLifeTimeManager {
    state: Mutex<GCLifeTimeState>,
}

#[derive(Default)]
struct GCLifeTimeState {
    running_jobs: usize,
    original_lifetime: Option<String>,
}

impl GCLifeTimeManager {
    /// 登记一个校验任务；若为首个任务则按需抬高 GC lifetime。
    pub fn addOneJob(&self, client: &dyn SqlChecksumClient) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| Error::Poisoned)?;
        if state.running_jobs == 0 {
            let original = client.obtain_gc_lifetime()?;
            let original_duration = parse_duration(&original)?;
            state.original_lifetime = Some(original);
            if original_duration < DefaultGCLifeTime {
                client.update_gc_lifetime(&format_duration(DefaultGCLifeTime))?;
            }
        }
        state.running_jobs += 1;
        Ok(())
    }

    /// 结束一个校验任务；任务归零时恢复原始 GC lifetime。
    pub fn removeOneJob(&self, client: &dyn SqlChecksumClient) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.running_jobs = state.running_jobs.saturating_sub(1);
        if state.running_jobs == 0 {
            if let Some(original) = state.original_lifetime.take() {
                let _ = client.update_gc_lifetime(&original);
            }
        }
    }
}

/// 按表与时间戳扫描并返回本地 KVChecksum 的数据源。
pub trait ChecksumSource: Send + Sync {
    fn checksum_table(
        &self,
        token: &CancellationToken,
        table: &TableInfo,
        ts: u64,
    ) -> Result<KVChecksum>;
}

/// PD 客户端：取 TSO、更新 Service GC Safe Point。
pub trait PDClient: Send + Sync {
    fn GetTS(&self, token: &CancellationToken) -> Result<(i64, i64)>;
    fn UpdateServiceGCSafePoint(
        &self,
        token: &CancellationToken,
        service_id: &str,
        ttl: i64,
        safe_point: u64,
    ) -> Result<u64>;
}

/// 基于 TiKV 扫描的校验管理器：取 TSO、挂 GC TTL、扫描表并返回 RemoteChecksum。
pub struct TiKVChecksumManager {
    source: Arc<dyn ChecksumSource>,
    pd_client: Arc<dyn PDClient>,
    gc_ttl_manager: Arc<GCTTLManager>,
    pub dist_sql_scan_concurrency: usize,
    pub backoff_weight: i32,
    pub resource_group_name: String,
    closed: AtomicBool,
}

/// 构造 TiKVChecksumManager，保留调用方指定的初始扫描并发度。
pub fn NewTiKVChecksumManager(
    source: Arc<dyn ChecksumSource>,
    pd_client: Arc<dyn PDClient>,
    dist_sql_scan_concurrency: usize,
    backoff_weight: i32,
    resource_group_name: String,
    service_prefix: &str,
) -> TiKVChecksumManager {
    let gc_ttl_manager = Arc::new(GCTTLManager::new(
        Arc::clone(&pd_client),
        format!("{service_prefix}-{}", std::process::id()),
    ));
    TiKVChecksumManager {
        source,
        pd_client,
        gc_ttl_manager,
        dist_sql_scan_concurrency,
        backoff_weight,
        resource_group_name,
        closed: AtomicBool::new(false),
    }
}

impl ChecksumManager for TiKVChecksumManager {
    fn Checksum(&self, token: &CancellationToken, table: &TableInfo) -> Result<RemoteChecksum> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        // Go 实现只获取一次成功的 TSO；可重试的 PD leader-change 错误不占用扫描预算。
        let (physical, logical) = loop {
            token.check()?;
            match self.pd_client.GetTS(token) {
                Ok(ts) => break ts,
                Err(error) => {
                    if !isRetryableChecksumError(&error) {
                        return Err(error);
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        };
        let timestamp = compose_ts(physical, logical)?;
        self.gc_ttl_manager
            .addOneJob(token, &table.table, timestamp)?;

        let mut last_error = None;
        for _ in 0..MAX_ERROR_RETRY_COUNT {
            if let Err(error) = token.check() {
                last_error = Some(error);
                break;
            }
            let result = self.source.checksum_table(token, table, timestamp);
            match result {
                Ok(checksum) => {
                    self.gc_ttl_manager.removeOneJob(token, &table.table);
                    return Ok(RemoteChecksum {
                        Schema: table.schema.clone(),
                        Table: table.table.clone(),
                        Checksum: checksum.checksum,
                        TotalKVs: checksum.total_kvs,
                        TotalBytes: checksum.total_bytes,
                    });
                }
                Err(error) => {
                    let retryable = isRetryableChecksumError(&error);
                    last_error = Some(error);
                    if !retryable {
                        break;
                    }
                }
            }
        }
        self.gc_ttl_manager.removeOneJob(token, &table.table);
        Err(last_error
            .unwrap_or_else(|| Error::Retryable("cannot obtain checksum timestamp".into())))
    }

    fn Close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.gc_ttl_manager.close(&CancellationToken::default());
        }
    }
}

/// 按表登记扫描用时间戳，并向 PD 更新 Service GC Safe Point（取各 job 最小 ts）。
pub struct GCTTLManager {
    pd_client: Arc<dyn PDClient>,
    service_id: String,
    jobs: Mutex<Vec<(String, u64)>>,
    last_updated_safe_point: AtomicI64,
    closed: AtomicBool,
}

impl GCTTLManager {
    /// 绑定 PD 客户端与 service_id。
    pub fn new(pd_client: Arc<dyn PDClient>, service_id: String) -> Self {
        Self {
            pd_client,
            service_id,
            jobs: Mutex::new(Vec::new()),
            last_updated_safe_point: AtomicI64::new(-1),
            closed: AtomicBool::new(false),
        }
    }

    /// 登记表扫描时间戳并刷新 Safe Point。
    pub fn addOneJob(&self, token: &CancellationToken, table: &str, ts: u64) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        let mut jobs = self.jobs.lock().map_err(|_| Error::Poisoned)?;
        jobs.push((table.to_owned(), ts));
        let safe_point = jobs
            .iter()
            .map(|(_, timestamp)| *timestamp)
            .min()
            .unwrap_or(0);
        let previous = self.last_updated_safe_point.load(Ordering::Relaxed);
        if previous >= 0 && safe_point >= previous as u64 {
            return Ok(());
        }
        drop(jobs);

        let ttl = serviceSafePointTTL.load(Ordering::Relaxed);
        self.pd_client
            .UpdateServiceGCSafePoint(token, &self.service_id, ttl, safe_point)?;
        self.last_updated_safe_point
            .store(safe_point as i64, Ordering::Relaxed);
        Ok(())
    }

    /// 移除一个表任务；Safe Point 由后续刷新或关闭动作更新。
    pub fn removeOneJob(&self, token: &CancellationToken, table: &str) {
        if let Ok(mut jobs) = self.jobs.lock()
            && let Some(index) = jobs.iter().position(|(name, _)| name == table)
        {
            jobs.remove(index);
        }
    }

    /// 关闭时用 ttl=0 撤销 Service GC Safe Point。
    pub fn close(&self, token: &CancellationToken) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            let _ = self.pd_client.UpdateServiceGCSafePoint(
                token,
                &self.service_id,
                0,
                self.last_updated_safe_point.load(Ordering::Relaxed).max(0) as u64,
            );
        }
    }
}

fn isRetryableChecksumError(error: &Error) -> bool {
    matches!(error, Error::Retryable(_) | Error::Timeout) || error.to_string().contains("EOF")
}

/// 将 PD 物理/逻辑时间戳合成为 u64 TSO（物理左移 18 位）。
fn compose_ts(physical: i64, logical: i64) -> Result<u64> {
    if physical < 0 || logical < 0 {
        return Err(Error::InvalidData("negative TSO component".into()));
    }
    Ok(((physical as u64) << 18) | logical as u64)
}

/// 解析形如 `100h` / `10m` / `60s` 的 GC lifetime 字符串。
fn parse_duration(value: &str) -> Result<Duration> {
    let value = value.trim();
    let split = value
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(value.len());
    let amount: u64 = value[..split]
        .parse()
        .map_err(|_| Error::InvalidData(format!("invalid GC lifetime: {value}")))?;
    match &value[split..] {
        "h" => Ok(Duration::from_secs(amount.saturating_mul(3600))),
        "m" => Ok(Duration::from_secs(amount.saturating_mul(60))),
        "s" | "" => Ok(Duration::from_secs(amount)),
        unit => Err(Error::InvalidData(format!(
            "unsupported GC lifetime unit: {unit}"
        ))),
    }
}

/// 将 Duration 格式化为 `Nh` / `Nm` / `Ns`（优先更大单位）。
fn format_duration(duration: Duration) -> String {
    if duration.as_secs() % 3600 == 0 {
        format!("{}h", duration.as_secs() / 3600)
    } else if duration.as_secs() % 60 == 0 {
        format!("{}m", duration.as_secs() / 60)
    } else {
        format!("{}s", duration.as_secs())
    }
}

/// CRC-64（多项式 0x42F0E1EBA9EA3693）增量计算，用于 KVChecksum。
fn crc64(data: &[u8], mut crc: u64) -> u64 {
    const POLYNOMIAL: u64 = 0x42f0_e1eb_a9ea_3693;
    for byte in data {
        crc ^= (*byte as u64) << 56;
        for _ in 0..8 {
            crc = if crc & (1 << 63) != 0 {
                (crc << 1) ^ POLYNOMIAL
            } else {
                crc << 1
            };
        }
    }
    crc
}
