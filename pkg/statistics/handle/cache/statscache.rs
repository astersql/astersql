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

// 统计缓存对外实现：`StatsCacheImpl` 与批量更新缓冲。
//
// 封装底层 `StatsCache` 的读写、容量与驱逐，并按健康度分桶刷新 Prometheus 仪表；
// `cacheOfBatchUpdate` 将表更新/删除攒批后回调，降低高频写放大。

use crate::{NewStatsCache, StatisticsTable, StatsCache};
use astersql_statistics_handle_metrics::{
    HEALTHY_BUCKET_CONFIGS, StatsHealthyBucket100To100, StatsHealthyBucketCount,
    StatsHealthyBucketPseudo, StatsHealthyBucketTotal, StatsHealthyBucketUnneededAnalyze,
    StatsHealthyGauges,
};
use std::sync::{Arc, RwLock};

/// 租约偏移倍数：用 `lease_ms * LeaseOffset` 回退版本检查起点，避免刚写入的版本被过早扫描。
pub const LeaseOffset: i64 = 5;
/// 批量更新默认批大小（与 Go `batchSizeOfUpdateBatch` 对齐）。
pub const batchSizeOfUpdateBatch: usize = 10;

/// The storage capabilities needed by cache refresh. Full StatsHandle implementations
/// implement this boundary automatically; SQL still runs through the real session pool.
pub trait StatsCacheHandle: Send + Sync {
    fn lease(&self) -> std::time::Duration;
    fn session_pool(&self) -> Arc<dyn stats_util::SessionPool>;
    fn table_stats_from_storage(
        &self,
        table: &model::TableInfo,
        id: i64,
        load_all: bool,
        snapshot: u64,
    ) -> stats_types::Result<Option<Arc<StatisticsTable>>>;
    fn table_info_by_id(
        &self,
        schema: &dyn stats_types::InfoSchema,
        id: i64,
    ) -> stats_types::Result<Option<Arc<model::TableInfo>>> {
        let table = schema
            .TableByID(id)
            .or_else(|| schema.FindTableByPartitionID(id).map(|(table, _, _)| table));
        table
            .map(|table| {
                table
                    .ModelMeta()
                    .map_err(|e| stats_types::Error(e.to_string()))
            })
            .transpose()
    }
}

impl<T: stats_types::StatsHandle + ?Sized> StatsCacheHandle for T {
    fn lease(&self) -> std::time::Duration {
        stats_util::LeaseGetter::lease(self)
    }
    fn session_pool(&self) -> Arc<dyn stats_util::SessionPool> {
        self.s_pool()
    }
    fn table_stats_from_storage(
        &self,
        table: &model::TableInfo,
        id: i64,
        load_all: bool,
        snapshot: u64,
    ) -> stats_types::Result<Option<Arc<StatisticsTable>>> {
        self.TableStatsFromStorage(table, id, load_all, snapshot)
    }
}

pub struct StatsCacheImpl {
    cache: RwLock<Arc<StatsCache>>,
    handle: Option<Arc<dyn StatsCacheHandle>>,
    // Tests exercise both real backends without changing process-global configuration.
    settings: Option<(bool, i64)>,
}

// Keep an extra shared indirection so both concrete handles and Arc<dyn StatsHandle>
// can enter the constructor without requiring an invalid cross-trait-object cast.
struct SharedHandle<H: ?Sized>(Arc<H>);
impl<H: StatsCacheHandle + ?Sized> StatsCacheHandle for SharedHandle<H> {
    fn lease(&self) -> std::time::Duration {
        self.0.lease()
    }
    fn session_pool(&self) -> Arc<dyn stats_util::SessionPool> {
        self.0.session_pool()
    }
    fn table_stats_from_storage(
        &self,
        info: &model::TableInfo,
        id: i64,
        all: bool,
        snapshot: u64,
    ) -> stats_types::Result<Option<Arc<StatisticsTable>>> {
        self.0.table_stats_from_storage(info, id, all, snapshot)
    }
    fn table_info_by_id(
        &self,
        schema: &dyn stats_types::InfoSchema,
        id: i64,
    ) -> stats_types::Result<Option<Arc<model::TableInfo>>> {
        self.0.table_info_by_id(schema, id)
    }
}

pub fn NewStatsCacheImpl<H: StatsCacheHandle + ?Sized + 'static>(
    handle: Arc<H>,
) -> Result<StatsCacheImpl, crate::CacheError> {
    StatsCacheImpl::new(Some(Arc::new(SharedHandle(handle))), None)
}

pub fn NewStatsCacheImplForTest() -> Result<StatsCacheImpl, crate::CacheError> {
    StatsCacheImpl::new(None, None)
}

/// Batch buffer used by Go `cacheOfBatchUpdate` when flushing table cache updates.
///
/// 批量更新缓冲：累计待更新表与待删除表 ID，满批或显式 flush 时调用 `op`。
pub(crate) struct cacheOfBatchUpdate<F>
where
    F: FnMut(&[Arc<StatisticsTable>], &[i64]),
{
    op: F,
    pub(crate) toUpdate: Vec<Arc<StatisticsTable>>,
    pub(crate) toDelete: Vec<i64>,
    batchSize: usize,
}

/// 创建指定批大小与刷新回调的批量更新缓冲。
pub(crate) fn newCacheOfBatchUpdate<F>(batchSize: usize, op: F) -> cacheOfBatchUpdate<F>
where
    F: FnMut(&[Arc<StatisticsTable>], &[i64]),
{
    cacheOfBatchUpdate {
        op,
        toUpdate: Vec::with_capacity(batchSize),
        toDelete: Vec::with_capacity(batchSize),
        batchSize,
    }
}

impl<F> cacheOfBatchUpdate<F>
where
    F: FnMut(&[Arc<StatisticsTable>], &[i64]),
{
    /// 执行回调并清空更新/删除缓冲。
    fn internalFlush(&mut self) {
        (self.op)(&self.toUpdate, &self.toDelete);
        self.toUpdate.clear();
        self.toDelete.clear();
    }

    /// 追加待更新表；若更新缓冲已满则先 flush。
    pub(crate) fn addToUpdate(&mut self, table: Arc<StatisticsTable>) {
        if self.toUpdate.len() == self.batchSize {
            self.internalFlush();
        }
        self.toUpdate.push(table);
    }

    /// 追加待删除表 ID；若删除缓冲已满则先 flush。
    pub(crate) fn addToDelete(&mut self, table_id: i64) {
        if self.toDelete.len() == self.batchSize {
            self.internalFlush();
        }
        self.toDelete.push(table_id);
    }

    /// 若缓冲非空则立即 flush，用于批次结束收尾。
    pub(crate) fn flush(&mut self) {
        if !self.toUpdate.is_empty() || !self.toDelete.is_empty() {
            self.internalFlush();
        }
    }
}

impl StatsCacheImpl {
    /// 克隆当前缓存快照的 `Arc` 引用。
    fn Load(&self) -> Arc<StatsCache> {
        self.cache.read().unwrap().clone()
    }

    pub(crate) fn new(
        handle: Option<Arc<dyn StatsCacheHandle>>,
        settings: Option<(bool, i64)>,
    ) -> Result<Self, crate::CacheError> {
        let cache = match settings {
            Some((quota, capacity)) => crate::NewStatsCacheWithCapacity(quota, capacity)?,
            None => NewStatsCache()?,
        };
        Ok(Self {
            cache: RwLock::new(Arc::new(cache)),
            handle,
            settings,
        })
    }

    fn quota_enabled(&self) -> bool {
        self.settings.map(|(quota, _)| quota).unwrap_or_else(|| {
            config::get_global_config()
                .performance
                .enable_stats_cache_mem_quota
        })
    }

    pub fn GetNextCheckVersionWithOffset(&self) -> u64 {
        let lease = self.handle.as_ref().map(|h| h.lease()).unwrap_or_default();
        // Go time.Duration multiplies signed nanoseconds before truncating to ms.
        let nanos = (lease.as_nanos() as i64).wrapping_mul(LeaseOffset);
        let offset = ((nanos / 1_000_000) as u64).wrapping_shl(18);
        self.MaxTableStatsVersion().saturating_sub(offset)
    }

    fn replace(&self, new: Arc<StatsCache>) {
        let old = std::mem::replace(&mut *self.cache.write().unwrap(), new.clone());
        old.Close();
        crate::statscacheinner::set_cost(new.Cost());
    }

    pub fn Replace(&self, new: &StatsCacheImpl) {
        self.replace(new.Load());
    }

    pub fn UpdateStatsCache(&self, updated: &[Arc<StatisticsTable>], deleted: &[i64], skip: bool) {
        let cache = self.Load();
        if self.quota_enabled() {
            cache.Update(updated, deleted, skip);
        } else {
            self.replace(Arc::new(cache.CopyAndUpdate(updated, deleted)));
        }
    }

    /// Load stats_meta in version order, retaining Go's partial-batch/error semantics.
    pub fn Update(
        &self,
        ctx: &stats_types::ExecutionContext,
        schema: &dyn stats_types::InfoSchema,
        ids: &[i64],
    ) -> stats_types::Result<()> {
        let _timer = LoadTimer(std::time::Instant::now());
        let handle = self
            .handle
            .as_ref()
            .ok_or_else(|| stats_types::Error("statistics handle is not configured".into()))?;
        let mut query = String::from(
            "SELECT version, table_id, modify_count, count, snapshot, last_stats_histograms_version from mysql.stats_meta where version > %? ",
        );
        let mut args = vec![stats_util::SqlValue::Unsigned(
            self.GetNextCheckVersionWithOffset(),
        )];
        let skip = !ids.is_empty();
        if skip {
            let mut ids = ids.to_vec();
            ids.sort_unstable();
            ids.dedup();
            query.push_str("and table_id in (%?) ");
            args.push(stats_util::SqlValue::StringList(
                ids.iter().map(i64::to_string).collect(),
            ));
        }
        query.push_str("order by version");
        let mut rows = Vec::new();
        stats_util::call_with_sctx(
            handle.session_pool().as_ref(),
            |session| {
                rows = stats_util::exec_rows(session, &query, &args)?.0;
                Ok(())
            },
            &[],
        )
        .map_err(|e| stats_types::Error(e.to_string()))?;
        let mut batch = newCacheOfBatchUpdate(batchSizeOfUpdateBatch, |updated, deleted| {
            self.UpdateStatsCache(updated, deleted, skip)
        });
        for row in rows {
            let version = integer(&row, 0)? as u64;
            let id = integer(&row, 1)?;
            let modify_count = integer(&row, 2)?;
            let count = integer(&row, 3)?;
            let snapshot = integer(&row, 4)? as u64;
            let hist_version = if row.values.get(5) == Some(&stats_util::SqlValue::Null) {
                0
            } else {
                integer(&row, 5)? as u64
            };
            if ctx.is_cancelled() {
                return Err(stats_types::Error("context canceled".into()));
            }
            let Some(info) = handle.table_info_by_id(schema, id)? else {
                log::debug!(
                    "unknown physical ID in stats meta table, maybe it has been dropped: {id}"
                );
                batch.addToDelete(id);
                continue;
            };
            let old = self.Get(id);
            if old
                .as_ref()
                .is_some_and(|t| t.Version >= version && t.TblInfoUpdateTS == info.UpdateTS)
            {
                continue;
            }
            let mut table = if let Some(old) =
                old.filter(|t| hist_version > 0 && t.LastStatsHistVersion >= hist_version)
            {
                old.CopyAs(statistics::CopyIntent::MetaOnly)
            } else {
                match handle.table_stats_from_storage(&info, id, false, 0) {
                    Err(error) => {
                        log::warn!(
                            "error occurred when read table stats: table={}, error={error}",
                            info.Name.O
                        );
                        continue;
                    }
                    Ok(None) => {
                        batch.addToDelete(id);
                        continue;
                    }
                    Ok(Some(table)) => table.CopyAs(statistics::CopyIntent::MetaOnly),
                }
            };
            table.Version = version;
            table.LastStatsHistVersion = hist_version;
            table.RealtimeCount = count;
            table.ModifyCount = modify_count;
            table.TblInfoUpdateTS = info.UpdateTS;
            if table.LastAnalyzeVersion == 0 && snapshot != 0 {
                table.LastAnalyzeVersion = snapshot;
            }
            batch.addToUpdate(Arc::new(table));
        }
        batch.flush();
        Ok(())
    }

    /// 关闭底层缓存。
    pub fn Close(&self) {
        self.Load().Close()
    }

    /// 清空缓存：替换为新建的空 `StatsCache`。
    pub fn Clear(&self) {
        let new = match self.settings {
            Some((quota, capacity)) => crate::NewStatsCacheWithCapacity(quota, capacity),
            None => NewStatsCache(),
        };
        match new {
            Ok(new) => self.replace(Arc::new(new)),
            Err(error) => log::warn!("create stats cache failed: {error}"),
        }
    }

    /// 返回当前缓存已消耗的内存代价（cost）。
    pub fn MemConsumed(&self) -> i64 {
        self.Load().Cost()
    }

    /// 按物理表 ID 查找完整统计。
    pub fn Get(&self, id: i64) -> Option<Arc<StatisticsTable>> {
        if fail::eval(
            "github.com/pingcap/tidb/pkg/statistics/handle/cache/StatsCacheGetNil",
            |_| true,
        )
        .unwrap_or(false)
        {
            return None;
        }
        self.Load().Get(id).0
    }

    /// 写入或覆盖指定物理表 ID 的完整统计。
    pub fn Put(&self, id: i64, t: Arc<StatisticsTable>) {
        self.Load().Put(id, t)
    }

    /// 触发一次缓存驱逐（evict）尝试。
    pub fn TriggerEvict(&self) {
        self.Load().TriggerEvict()
    }

    /// 等待异步更新队列排空。
    pub fn WaitForAsyncUpdates(&self) {
        self.Load().WaitForAsyncUpdates()
    }

    /// 返回缓存中表统计的最大版本号。
    pub fn MaxTableStatsVersion(&self) -> u64 {
        self.Load().Version()
    }

    /// 返回缓存中全部表完整统计。
    pub fn Values(&self) -> Vec<Arc<StatisticsTable>> {
        self.Load().Values()
    }

    /// 返回缓存中的表条目数。
    pub fn Len(&self) -> usize {
        self.Load().Len()
    }

    /// 设置缓存容量上限。
    pub fn SetStatsCacheCapacity(&self, c: i64) {
        self.Load().SetCapacity(c)
    }

    /// Refreshes `StatsHealthyGauges` using the same classification rules as Go.
    ///
    /// 遍历缓存中的表，按伪统计、无需 ANALYZE、健康度区间分桶，写回 `StatsHealthyGauges`。
    pub fn UpdateStatsHealthyMetrics(&self) {
        let mut buckets = [0_i64; StatsHealthyBucketCount];
        for tbl in self.Values() {
            buckets[StatsHealthyBucketTotal] += 1;
            // 伪统计单独计数，不再参与健康度分桶。
            if tbl.Pseudo {
                buckets[StatsHealthyBucketPseudo] += 1;
                continue;
            }
            // 行数未达自动 ANALYZE 阈值且从未分析过：记为 unneeded analyze。
            if !tbl.MeetAutoAnalyzeMinCnt() && !tbl.IsAnalyzed() {
                buckets[StatsHealthyBucketUnneededAnalyze] += 1;
                continue;
            }
            let (healthy, ok) = tbl.GetStatsHealthy();
            if !ok {
                continue;
            }
            buckets[statsHealthyBucketIndex(healthy)] += 1;
        }
        for (idx, gauge) in StatsHealthyGauges.iter().enumerate() {
            gauge.set(buckets[idx] as f64);
        }
    }
}

/// 将健康度分数映射到 `HEALTHY_BUCKET_CONFIGS` 中的桶下标；100 落入闭区间桶。
pub fn statsHealthyBucketIndex(healthy: i64) -> usize {
    debug_assert!(
        (0..=100).contains(&healthy),
        "healthy value out of range: {healthy}"
    );
    for cfg in HEALTHY_BUCKET_CONFIGS {
        if cfg.upper_bound <= 0 {
            continue;
        }
        if healthy < cfg.upper_bound {
            return cfg.index;
        }
    }
    StatsHealthyBucket100To100
}

fn integer(row: &stats_util::Row, index: usize) -> stats_types::Result<i64> {
    match row.values.get(index) {
        Some(stats_util::SqlValue::Integer(value)) => Ok(*value),
        Some(stats_util::SqlValue::Unsigned(value)) => Ok(*value as i64),
        _ => Err(stats_types::Error(format!(
            "invalid stats_meta integer at column {index}"
        ))),
    }
}

struct LoadTimer(std::time::Instant);
impl Drop for LoadTimer {
    #[allow(static_mut_refs)]
    fn drop(&mut self) {
        unsafe {
            if let Some(histogram) = &metrics::stats::StatsDeltaLoadHistogram {
                histogram.observe(self.0.elapsed().as_secs_f64());
            }
        }
    }
}

impl stats_types::StatsCache for StatsCacheImpl {
    fn Close(&self) {
        self.Close()
    }
    fn Clear(&self) {
        self.Clear()
    }
    fn Update(
        &self,
        ctx: &stats_types::ExecutionContext,
        schema: &dyn stats_types::InfoSchema,
        ids: &[i64],
    ) -> stats_types::Result<()> {
        self.Update(ctx, schema, ids)
    }
    fn MemConsumed(&self) -> i64 {
        self.MemConsumed()
    }
    fn Get(&self, id: i64) -> Option<Arc<StatisticsTable>> {
        self.Get(id)
    }
    fn Put(&self, id: i64, table: Arc<StatisticsTable>) {
        self.Put(id, table)
    }
    fn UpdateStatsCache(&self, update: stats_types::CacheUpdate) {
        self.UpdateStatsCache(
            &update.Updated,
            &update.Deleted,
            update.Options.SkipMoveForward,
        )
    }
    fn GetNextCheckVersionWithOffset(&self) -> u64 {
        self.GetNextCheckVersionWithOffset()
    }
    fn MaxTableStatsVersion(&self) -> u64 {
        self.MaxTableStatsVersion()
    }
    fn Values(&self) -> Vec<Arc<StatisticsTable>> {
        self.Values()
    }
    fn Len(&self) -> usize {
        self.Len()
    }
    fn SetStatsCacheCapacity(&self, capacity: i64) {
        self.SetStatsCacheCapacity(capacity)
    }
    fn Replace(&self, cache: &dyn stats_types::StatsCache) {
        let cache = (cache as &dyn std::any::Any)
            .downcast_ref::<Self>()
            .expect("Replace requires StatsCacheImpl");
        self.Replace(cache)
    }
    fn UpdateStatsHealthyMetrics(&self) {
        self.UpdateStatsHealthyMetrics()
    }
    fn TriggerEvict(&self) {
        self.TriggerEvict()
    }
    fn WaitForAsyncUpdates(&self) {
        self.WaitForAsyncUpdates()
    }
}
