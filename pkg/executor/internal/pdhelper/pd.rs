// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 从 PD 或受限 SQL 获取并缓存表的近似行数。
//
// PD（Placement Driver）提供 Region 级存储键估计；小表可能与其它表共享
// Region，故仅在 Region 数 > 2 时采信 PD，否则回退为 `select count(*)`。
// 结果带 TTL 的 LRU 缓存，并由后台清理协程淘汰过期项。
#![allow(non_snake_case, non_upper_case_globals)]

use std::error::Error;
use std::fmt;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use lru::LruCache;

/// 默认缓存 TTL（存活时间），与 Go ttlcache 默认一致。
/// Go's default ttlcache TTL.
pub const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(30);
/// 默认缓存容量（条目数上限）。
/// Go's default ttlcache entry capacity.
pub const DEFAULT_CACHE_CAPACITY: usize = 1024 * 1024;

/// 对应 Go 包级 `globalPDHelperOnce`，所有 PDHelper 实例共享一次启动机会。
static GLOBAL_PD_HELPER_ONCE: AtomicBool = AtomicBool::new(false);

/// 本辅助逻辑用到的 PD Region 统计子集。
/// Region：TiKV/TiFlash 中按键范围划分的数据分片。
/// The subset of PD region statistics used by this helper.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RegionStats {
    /// Region 数量。
    pub count: i64,
    /// 存储层估计的键数量（近似行数来源之一）。
    pub storage_keys: i64,
}

/// 存储或受限 SQL 错误。Go 在包边界有意将这些错误转换为 `(0, false)`。
/// A storage or restricted-SQL error. The Go implementation intentionally
/// converts these errors to `(0, false)` at the package boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PdHelperError {
    /// 错误消息文本。
    message: String,
}

impl PdHelperError {
    /// 由消息构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for PdHelperError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for PdHelperError {}

/// 对应 `kv.WithInternalSourceType` 的上下文行为。
/// Context behavior corresponding to `kv.WithInternalSourceType`.
pub trait InternalSourceContext: Clone {
    /// 标记后续操作为内部统计前台来源。
    fn with_internal_stats_foreground(self) -> Self;
}

/// 对 Go `sessionctx.Context` 中本逻辑用到的两个操作的适配。
/// `get_pd_region_stats` 返回 `Ok(None)` 表示 store 未实现 `helper.Storage`；
/// `exec_restricted_count` 返回 `Ok(None)` 表示 COUNT 结果为空或非法。
/// Adapter over the two `sessionctx.Context` operations used by the Go code.
/// Returning `Ok(None)` from `get_pd_region_stats` means that the session's
/// store does not implement `helper.Storage`; returning `Ok(None)` from
/// `exec_restricted_count` represents an empty or malformed COUNT result.
pub trait SessionContext<C: InternalSourceContext>: Send + Sync {
    /// 向 PD 查询指定物理表 ID 的 Region 统计。
    fn get_pd_region_stats(
        &self,
        ctx: &C,
        physical_id: i64,
        include_stats: bool,
    ) -> Result<Option<RegionStats>, PdHelperError>;

    /// 以受限权限执行 COUNT SQL，返回计数值。
    fn exec_restricted_count(&self, ctx: C, sql: &str) -> Result<Option<i64>, PdHelperError>;
}

#[derive(Clone, Copy)]
/// 缓存条目：数值及其过期时刻。
struct CacheEntry {
    /// 缓存的近似行数。
    value: f64,
    /// 过期时间点。
    expires_at: Instant,
}

/// 后台清理协程的启停状态。
struct CleanupState {
    /// 是否请求停止清理循环。
    stop: bool,
    /// 清理线程句柄。
    worker: Option<JoinHandle<()>>,
}

/// PDHelper 的共享内部状态。
struct PDHelperInner {
    /// 带互斥锁的 LRU 缓存。
    cache: Mutex<LruCache<String, CacheEntry>>,
    /// 条目存活时间。
    ttl: Duration,
    /// 清理协程状态。
    cleanup: Mutex<CleanupState>,
    /// 唤醒清理循环的条件变量。
    cleanup_wakeup: Condvar,
}

impl PDHelperInner {
    /// 获取缓存锁；若锁被 poison 则吞掉 poison 继续使用。
    fn cache(&self) -> MutexGuard<'_, LruCache<String, CacheEntry>> {
        self.cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 获取清理状态锁。
    fn cleanup(&self) -> MutexGuard<'_, CleanupState> {
        self.cleanup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 扫描并删除所有已过期的缓存键。
    fn remove_expired(&self, now: Instant) {
        let mut cache = self.cache();
        let expired = cache
            .iter()
            .filter_map(|(key, entry)| (entry.expires_at <= now).then(|| key.clone()))
            .collect::<Vec<_>>();
        for key in expired {
            cache.pop(&key);
        }
    }
}

/// 从 PD 或受限 SQL 获取并缓存表的近似行数。
/// Fetches and caches approximate table counts from PD or restricted SQL.
pub struct PDHelper {
    /// 共享内部状态，便于清理线程持有弱引用。
    inner: Arc<PDHelperInner>,
}

impl Default for PDHelper {
    fn default() -> Self {
        Self::with_cache_config(DEFAULT_CACHE_TTL, DEFAULT_CACHE_CAPACITY)
    }
}

impl PDHelper {
    /// 按指定 TTL 与容量构造 helper；二者均不可为 0。
    pub fn with_cache_config(ttl: Duration, capacity: usize) -> Self {
        assert!(!ttl.is_zero(), "PDHelper cache TTL must be non-zero");
        let capacity =
            NonZeroUsize::new(capacity).expect("PDHelper cache capacity must be non-zero");
        Self {
            inner: Arc::new(PDHelperInner {
                cache: Mutex::new(LruCache::new(capacity)),
                ttl,
                cleanup: Mutex::new(CleanupState {
                    stop: false,
                    worker: None,
                }),
                cleanup_wakeup: Condvar::new(),
            }),
        }
    }

    /// 启动缓存清理 worker（仅一次），对应 Go `globalPDHelperOnce`。
    /// Starts the cache cleanup worker once, matching `globalPDHelperOnce`.
    pub fn Start(&self) {
        // Go 的 sync.Once 是包级变量，而非 PDHelper 实例字段。
        if GLOBAL_PD_HELPER_ONCE.swap(true, Ordering::AcqRel) {
            return;
        }

        let mut cleanup = self.inner.cleanup();
        let weak = Arc::downgrade(&self.inner);
        cleanup.worker = Some(thread::spawn(move || cleanup_loop(weak)));
    }

    /// 停止清理 worker 并等待其退出。
    /// Stops the cleanup worker and waits for it to exit.
    pub fn Stop(&self) {
        let worker = {
            let mut cleanup = self.inner.cleanup();
            cleanup.stop = true;
            self.inner.cleanup_wakeup.notify_all();
            cleanup.worker.take()
        };
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }

    /// 优先读缓存；未命中则走与 Go 相同的 PD→受限 SQL 决策树，
    /// 并将数值结果写入缓存（即便 `has_pd == false`）。
    /// Returns a cached value when present. On a miss it executes the same PD
    /// then restricted-SQL decision tree as the Go implementation and caches
    /// the numeric result even when the lookup reports `has_pd == false`.
    pub fn GetApproximateTableCountFromStorage<C, S>(
        &self,
        ctx: C,
        sctx: &S,
        tid: i64,
        db_name: &str,
        table_name: &str,
        partition_name: &str,
    ) -> (f64, bool)
    where
        C: InternalSourceContext,
        S: SessionContext<C> + ?Sized,
    {
        let key = approximate_table_count_key(tid, db_name, table_name, partition_name);
        // Go 缓存只存 float，命中时 has_pd 恒为 true。
        if let Some(value) = self.get_cached(&key) {
            // Go's cache stores only the float, so a cache hit always reports true.
            return (value, true);
        }

        let (result, has_pd) = get_approximate_table_count_from_storage(
            ctx,
            sctx,
            tid,
            db_name,
            table_name,
            partition_name,
        );
        self.insert_cached(key, result);
        (result, has_pd)
    }

    /// 读取未过期缓存；遇过期条目则弹出并视为未命中。
    fn get_cached(&self, key: &str) -> Option<f64> {
        let now = Instant::now();
        let mut cache = self.inner.cache();
        let entry = cache.get(key).copied();
        match entry {
            Some(entry) if entry.expires_at > now => Some(entry.value),
            Some(_) => {
                cache.pop(key);
                None
            }
            None => None,
        }
    }

    /// 写入/更新缓存条目，过期时间 = now + ttl。
    fn insert_cached(&self, key: String, value: f64) {
        self.inner.cache().put(
            key,
            CacheEntry {
                value,
                expires_at: Instant::now() + self.inner.ttl,
            },
        );
    }

    #[cfg(test)]
    /// 返回当前 helper 是否持有清理 worker，仅供核对 Go 包级 once 语义。
    pub(crate) fn cleanup_worker_started(&self) -> bool {
        self.inner.cleanup().worker.is_some()
    }
}

/// 析构时停止清理线程，避免泄漏。
impl Drop for PDHelper {
    fn drop(&mut self) {
        self.Stop();
    }
}

/// 清理循环：按 TTL 休眠，醒来后删除过期项；`stop` 或强引用消失时退出。
fn cleanup_loop(inner: Weak<PDHelperInner>) {
    loop {
        let Some(inner) = inner.upgrade() else {
            return;
        };
        let cleanup = inner.cleanup();
        let (cleanup, _) = inner
            .cleanup_wakeup
            .wait_timeout_while(cleanup, inner.ttl, |state| !state.stop)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if cleanup.stop {
            return;
        }
        drop(cleanup);
        inner.remove_expired(Instant::now());
    }
}

/// 对应 Go `GlobalPDHelper` 的全局单例。
/// Global helper corresponding to Go's `GlobalPDHelper`.
pub static GlobalPDHelper: LazyLock<PDHelper> = LazyLock::new(defaultPDHelper);

/// 使用默认 TTL/容量构造 PDHelper。
pub fn defaultPDHelper() -> PDHelper {
    PDHelper::default()
}

/// 生成缓存键：`{tid}_{db}_{table}_{partition}`。
pub fn approximate_table_count_key(
    tid: i64,
    db_name: &str,
    table_name: &str,
    partition_name: &str,
) -> String {
    [
        tid.to_string(),
        db_name.to_owned(),
        table_name.to_owned(),
        partition_name.to_owned(),
    ]
    .join("_")
}

/// Go 风格驼峰别名，转发到 `approximate_table_count_key`。
pub fn approximateTableCountKey(
    tid: i64,
    db_name: &str,
    table_name: &str,
    partition_name: &str,
) -> String {
    approximate_table_count_key(tid, db_name, table_name, partition_name)
}

/// 无缓存的一次近似行数查询：先 PD，大表用 storage_keys，小表跑 COUNT。
pub fn get_approximate_table_count_from_storage<C, S>(
    ctx: C,
    sctx: &S,
    tid: i64,
    db_name: &str,
    table_name: &str,
    partition_name: &str,
) -> (f64, bool)
where
    C: InternalSourceContext,
    S: SessionContext<C> + ?Sized,
{
    // PD 不可用或 store 不支持时直接失败关闭。
    let region_stats =
        match pd_region_stats_with_failpoint(sctx.get_pd_region_stats(&ctx, tid, true)) {
            Ok(Some(stats)) => stats,
            Ok(None) | Err(_) => return (0.0, false),
        };

    // Small tables may share regions with other large tables, so PD's storage
    // key estimate is used only when the table occupies more than two regions.
    if region_stats.count > 2 {
        return (region_stats.storage_keys as f64, true);
    }

    let sql = count_sql(db_name, table_name, partition_name);
    let ctx = ctx.with_internal_stats_foreground();
    match sctx.exec_restricted_count(ctx, &sql) {
        Ok(Some(count)) => (count as f64, true),
        Ok(None) | Err(_) => (0.0, false),
    }
}

/// Go 风格驼峰别名，转发到 `get_approximate_table_count_from_storage`。
pub fn getApproximateTableCountFromStorage<C, S>(
    ctx: C,
    sctx: &S,
    tid: i64,
    db_name: &str,
    table_name: &str,
    partition_name: &str,
) -> (f64, bool)
where
    C: InternalSourceContext,
    S: SessionContext<C> + ?Sized,
{
    get_approximate_table_count_from_storage(ctx, sctx, tid, db_name, table_name, partition_name)
}

/// 可选 failpoint：注入固定 RegionStats，便于采样率相关测试。
fn pd_region_stats_with_failpoint(
    result: Result<Option<RegionStats>, PdHelperError>,
) -> Result<Option<RegionStats>, PdHelperError> {
    fail::fail_point!("calcSampleRateByStorageCount", |_| {
        Ok(Some(RegionStats {
            count: 1,
            storage_keys: 1_000_000,
        }))
    });
    result
}

/// 构造 `select count(*) from ... [partition(...)]`，标识符经反引号转义。
fn count_sql(db_name: &str, table_name: &str, partition_name: &str) -> String {
    let mut sql = format!(
        "select count(*) from {}.{}",
        quote_identifier(db_name),
        quote_identifier(table_name)
    );
    if !partition_name.is_empty() {
        sql.push_str(" partition(");
        sql.push_str(&quote_identifier(partition_name));
        sql.push(')');
    }
    sql
}

/// MySQL 风格标识符引用：用反引号包裹，内部反引号加倍。
fn quote_identifier(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}
