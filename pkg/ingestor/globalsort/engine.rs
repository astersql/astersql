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

// 外部存储导入引擎（External Engine）：按 job 键批量加载有序 KV，处理重复键并输出可 ingest 批次。
//
// `Engine` 从对象存储读取 data/stat 文件，在内存中排序去重后封装为 `MemoryIngestData`，
// 供下游 regionJob（按 Region 范围导入 SST）消费；支持并发度调节与内存配额等待。

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};

#[path = "engine_api.rs"]
mod api;
pub use api::ExternalEngineAdapter;

#[cfg(test)]
#[path = "engine_test.rs"]
mod engine_test;

use crate::reader::{CancellationToken, MemKvsAndBuffers, read_all_data};
use crate::{ConflictInfo, Error, KeyRange, KvPair, OnDuplicateKey, Result, Storage};

/// 写步骤与引擎共享内存容量时的份额系数（与 Go `writeStepMemShareCount` 一致）。
const writeStepMemShareCount: f64 = 6.5;

/// 由总内存容量推算引擎可用内存上限；容量 ≤0 时返回 0。
pub fn getEngineMemoryLimit(mem_capacity: i64) -> usize {
    if mem_capacity <= 0 {
        return 0;
    }
    (mem_capacity as f64 / writeStepMemShareCount * 3.0) as usize
}

/// 工作池并发度调谐接口，供 `UpdateResource` 回调。
pub trait WorkerPoolTuner: Send + Sync {
    fn Tune(&self, concurrency: usize);
}

/// 一批可导入数据及其有序键范围列表。
#[derive(Clone)]
pub struct DataAndRanges {
    pub data: MemoryIngestData,
    pub sorted_ranges: Vec<KeyRange>,
}

/// 基于外部存储文件的导入引擎状态机。
/// Shared live controls can be updated by the framework while the loader holds
/// mutable ownership of its buffers. The actual downstream pool performs Tune.
#[derive(Default)]
struct LoadState {
    loading: bool,
    finished: bool,
    applied: i32,
    pending: bool,
}
struct LoadGuard(Arc<EngineResource>);
impl Drop for LoadGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0.ready.0.lock() {
            state.loading = false;
            state.finished = true;
            self.0.ready.1.notify_all();
        }
    }
}
pub struct EngineResource {
    worker_pool: Mutex<Option<Arc<dyn WorkerPoolTuner>>>,
    concurrency: AtomicI32,
    memory_limit: AtomicUsize,
    ready: (Mutex<LoadState>, Condvar),
}
impl EngineResource {
    pub fn SetWorkerPool(&self, pool: Arc<dyn WorkerPoolTuner>) {
        *self.worker_pool.lock().unwrap() = Some(pool);
    }
    pub fn UpdateResource(&self, concurrency: i32, memory_capacity: i64) -> Result<()> {
        self.UpdateResourceWith(&CancellationToken::default(), concurrency, memory_capacity)
    }
    pub fn WorkerConcurrency(&self) -> i32 {
        self.concurrency.load(Ordering::Acquire)
    }
    pub fn UpdateResourceWith(
        &self,
        token: &CancellationToken,
        concurrency: i32,
        memory_capacity: i64,
    ) -> Result<()> {
        let worker_pool = self
            .worker_pool
            .lock()
            .map_err(|_| Error::Poisoned)?
            .clone()
            .ok_or_else(|| {
                Error::InvalidArgument("region job worker is not initialized, retry later".into())
            })?;
        if self.concurrency.load(Ordering::Acquire) == concurrency {
            return Ok(());
        }
        if concurrency <= 0 || memory_capacity <= 0 {
            return Err(Error::InvalidArgument(
                "concurrency and memory capacity must be positive".into(),
            ));
        }
        let mut state = self.ready.0.lock().map_err(|_| Error::Poisoned)?;
        state.pending = state.loading;
        self.memory_limit
            .store(getEngineMemoryLimit(memory_capacity), Ordering::Release);
        self.concurrency.store(concurrency, Ordering::Release);
        while state.loading && state.applied != concurrency {
            if token.is_cancelled() {
                state.pending = false;
                self.ready.1.notify_all();
                return Err(Error::Cancelled);
            }
            state = self
                .ready
                .1
                .wait_timeout(state, std::time::Duration::from_millis(10))
                .map_err(|_| Error::Poisoned)?
                .0;
        }
        state.pending = false;
        self.ready.1.notify_all();
        if token.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let finished = state.finished;
        drop(state);
        if !finished {
            worker_pool.Tune(concurrency as usize);
        }
        Ok(())
    }
}
pub struct Engine {
    storage: Arc<dyn Storage>,
    data_files: Vec<String>,
    stats_files: Vec<String>,
    start_key: Vec<u8>,
    end_key: Vec<u8>,
    /// 作业切分键序列；相邻键构成一批加载的半开区间。
    job_keys: Vec<Vec<u8>>,
    /// Region 分裂键（Region：TiKV 数据分片）。
    split_keys: Vec<Vec<u8>>,
    loaded: MemKvsAndBuffers,
    total_loaded_kvs_count: AtomicI64,
    in_flight_data_count: Arc<AtomicI64>,
    in_flight_bytes: Arc<AtomicUsize>,
    release_signal: Arc<(Mutex<bool>, Condvar)>,
    active_ingest_data_flags: Vec<Arc<AtomicBool>>,
    resource: Arc<EngineResource>,
    check_hotspot: bool,
    timestamp: u64,
    total_kv_size: i64,
    total_kv_count: i64,
    imported_kv_size: Arc<AtomicI64>,
    imported_kv_count: Arc<AtomicI64>,
    on_duplicate: OnDuplicateKey,
    file_prefix: String,
    recorded_duplicate_count: usize,
    recorded_duplicate_size: i64,
    duplicate_pairs: Vec<KvPair>,
    closed: bool,
}

/// 构造外部引擎；校验 data/stat 数量一致、并发度 >0、job_keys 有序。
#[allow(clippy::too_many_arguments)]
pub fn NewExternalEngine(
    storage: Arc<dyn Storage>,
    data_files: Vec<String>,
    stats_files: Vec<String>,
    start_key: Vec<u8>,
    end_key: Vec<u8>,
    job_keys: Vec<Vec<u8>>,
    split_keys: Vec<Vec<u8>>,
    worker_concurrency: i32,
    timestamp: u64,
    total_kv_size: i64,
    total_kv_count: i64,
    check_hotspot: bool,
    memory_capacity: i64,
    on_duplicate: OnDuplicateKey,
    file_prefix: String,
) -> Result<Engine> {
    if data_files.len() != stats_files.len() {
        return Err(Error::InvalidArgument(
            "data and stat file counts differ".into(),
        ));
    }
    if worker_concurrency <= 0 {
        return Err(Error::InvalidArgument(
            "worker concurrency must be greater than zero".into(),
        ));
    }
    if !job_keys.windows(2).all(|keys| keys[0] <= keys[1]) {
        return Err(Error::InvalidArgument("job keys must be sorted".into()));
    }
    Ok(Engine {
        storage,
        data_files,
        stats_files,
        start_key,
        end_key,
        job_keys,
        split_keys,
        loaded: MemKvsAndBuffers::default(),
        total_loaded_kvs_count: AtomicI64::new(0),
        in_flight_data_count: Arc::new(AtomicI64::new(0)),
        in_flight_bytes: Arc::new(AtomicUsize::new(0)),
        release_signal: Arc::new((Mutex::new(false), Condvar::new())),
        active_ingest_data_flags: Vec::new(),
        resource: Arc::new(EngineResource {
            worker_pool: Mutex::new(None),
            concurrency: AtomicI32::new(worker_concurrency),
            memory_limit: AtomicUsize::new(getEngineMemoryLimit(memory_capacity)),
            ready: (Mutex::new(LoadState::default()), Condvar::new()),
        }),
        check_hotspot,
        timestamp,
        total_kv_size,
        total_kv_count,
        imported_kv_size: Arc::new(AtomicI64::new(0)),
        imported_kv_count: Arc::new(AtomicI64::new(0)),
        on_duplicate,
        file_prefix,
        recorded_duplicate_count: 0,
        recorded_duplicate_size: 0,
        duplicate_pairs: Vec::new(),
        closed: false,
    })
}

impl Engine {
    /// Transfer loaded payloads without doubling the batch's live allocation.
    fn take_deduplicated_pairs(&mut self) -> Vec<KvPair> {
        let sorted = std::mem::take(&mut self.loaded.kvs);
        // Classify adjacent keys before consuming the buffer; both retained and
        // recorded rows keep their original payload allocations and order.
        let duplicated = (0..sorted.len())
            .map(|index| {
                (index > 0 && sorted[index - 1].key == sorted[index].key)
                    || (index + 1 < sorted.len() && sorted[index + 1].key == sorted[index].key)
            })
            .collect::<Vec<_>>();
        let mut deduplicated = Vec::with_capacity(sorted.len());
        for (pair, duplicated) in sorted.into_iter().zip(duplicated) {
            if !duplicated {
                deduplicated.push(pair);
            } else if self.on_duplicate == OnDuplicateKey::Record {
                self.recorded_duplicate_count += 1;
                self.recorded_duplicate_size += pair.encoded_size() as i64;
                self.duplicate_pairs.push(pair);
            }
        }
        deduplicated
    }

    /// 按 worker 并发度分批加载 job 键区间数据，返回全部批次；结束后按需写出重复键文件。
    pub fn LoadIngestData(&mut self, token: &CancellationToken) -> Result<Vec<DataAndRanges>> {
        let mut outputs = Vec::new();
        self.load_ingest_data_with(token, false, |batch| {
            outputs.push(batch);
            Ok(())
        })?;
        Ok(outputs)
    }

    /// Native consumers send each loaded batch through their bounded operator
    /// channel and release it after ingest. The collecting wrapper remains for
    /// existing callers whose fixtures deliberately retain all returned batches.
    pub fn LoadIngestDataWith(
        &mut self,
        token: &CancellationToken,
        consume: impl FnMut(DataAndRanges) -> Result<()>,
    ) -> Result<()> {
        self.load_ingest_data_with(token, true, consume)
    }

    fn load_ingest_data_with(
        &mut self,
        token: &CancellationToken,
        wait_for_release: bool,
        mut consume: impl FnMut(DataAndRanges) -> Result<()>,
    ) -> Result<()> {
        if self.closed {
            return Err(Error::Closed);
        }
        {
            let mut state = self.resource.ready.0.lock().map_err(|_| Error::Poisoned)?;
            state.loading = true;
            state.finished = false;
            state.applied = self.resource.WorkerConcurrency();
        }
        let _loading = LoadGuard(self.resource.clone());
        if self.job_keys.len() < 2 {
            return Ok(());
        }
        let offsets = crate::reader::get_read_ranges_from_props(
            token,
            self.storage.as_ref(),
            &self.job_keys,
            &self.stats_files,
        )?;
        let duplicate_storage = self.storage.clone();
        let duplicate_path = format!("{}/dup", self.file_prefix.trim_end_matches('/'));
        let mut duplicate_writer = None;
        let mut start = 0;
        let mut batch_size = self.resource.concurrency.load(Ordering::Acquire).max(1) as usize;
        let result = (|| {
            while start + 1 < self.job_keys.len() {
                if token.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                batch_size = self.handle_concurrency_change(token, batch_size)?;
                let end = (start + batch_size + 1).min(self.job_keys.len());
                let keys = self.job_keys[start..end].to_vec();
                let batch = self.load_range_batch_data(
                    token,
                    &keys,
                    &offsets[start],
                    &offsets[end - 1],
                    wait_for_release,
                    &mut |pairs| {
                        if duplicate_writer.is_none() {
                            duplicate_writer = Some(duplicate_storage.create(&duplicate_path)?);
                        }
                        let writer = duplicate_writer
                            .as_mut()
                            .expect("duplicate writer initialized");
                        for pair in pairs {
                            if token.is_cancelled() {
                                return Err(Error::Cancelled);
                            }
                            crate::merge::write_stream_pair(
                                writer.as_mut(),
                                pair,
                                duplicate_storage.record_format(),
                            )?;
                        }
                        Ok(())
                    },
                )?;
                consume(batch)?;
                start += batch_size;
            }
            Ok(())
        })();
        // Go closes the duplicate writer on both success and error, preserving
        // the original load/consumer error when close also fails.
        let close = duplicate_writer.map_or(Ok(()), |writer| writer.finish());
        result.and(close)
    }

    /// 加载 `[job_keys.first, job_keys.last)` 范围内的 KV，排序去重后封装为批次。
    fn load_range_batch_data(
        &mut self,
        token: &CancellationToken,
        job_keys: &[Vec<u8>],
        starts: &[u64],
        ends: &[u64],
        wait_for_release: bool,
        record: &mut dyn FnMut(&[KvPair]) -> Result<()>,
    ) -> Result<DataAndRanges> {
        let start_key = job_keys.first().cloned().unwrap_or_default();
        let end_key = job_keys.last().cloned().unwrap_or_default();
        loop {
            match read_all_data(
                token,
                self.storage.as_ref(),
                &self.data_files,
                &self.stats_files,
                &start_key,
                &end_key,
                starts,
                ends,
                self.resource
                    .memory_limit
                    .load(Ordering::Acquire)
                    .saturating_sub(self.in_flight_bytes.load(Ordering::Acquire)),
                &mut self.loaded,
            ) {
                Ok(()) => break,
                Err(Error::OutOfMemory { .. }) if wait_for_release => {
                    self.wait_ingest_data_released(token)?
                }
                Err(error) => return Err(error),
            }
        }
        self.loaded.build();
        self.loaded
            .kvs
            .sort_by(|left, right| left.key.cmp(&right.key));
        // 先探测是否存在任一重复键，决定 Ignore/Error 是否立即失败。
        let duplicate = self
            .loaded
            .kvs
            .windows(2)
            .find(|pairs| pairs[0].key == pairs[1].key)
            .map(|pairs| pairs[1].clone());
        if let Some(pair) = duplicate.as_ref() {
            match self.on_duplicate {
                // Preserve the Go compatibility branch: Ignore still reports a duplicate.
                // 兼容 Go：Ignore 与 Error 一样在发现重复时直接报错。
                OnDuplicateKey::Ignore | OnDuplicateKey::Error => {
                    self.loaded.clear();
                    return Err(Error::DuplicateKey {
                        key: pair.key.clone(),
                        value: pair.value.clone(),
                    });
                }
                OnDuplicateKey::Record => record(&[])?,
                OnDuplicateKey::Remove => {}
            }
        }
        // Matches simplesst.RemoveDuplicates(kvs, key, recordRemoved) with
        // keptDupCnt=0: any key occurring more than once is dropped in full
        // (not just the repeats after the first), and Record additionally
        // keeps every dropped copy for the duplicate-key writer.
        // 出现次数 >1 的 key 整组丢弃；Record 另将丢弃副本写入 duplicate_pairs。
        let deduplicated = self.take_deduplicated_pairs();
        if !self.duplicate_pairs.is_empty() {
            let written = record(&self.duplicate_pairs);
            self.duplicate_pairs.clear();
            written?;
        }
        self.total_loaded_kvs_count
            .fetch_add(deduplicated.len() as i64, Ordering::Relaxed);
        self.loaded.size = 0;
        let in_flight = Arc::clone(&self.in_flight_data_count);
        let signal = Arc::clone(&self.release_signal);
        let in_flight_bytes = self.in_flight_bytes.clone();
        let retained_bytes = deduplicated.iter().map(KvPair::encoded_size).sum::<usize>();
        in_flight_bytes.fetch_add(retained_bytes, Ordering::AcqRel);
        in_flight.fetch_add(1, Ordering::AcqRel);
        // 释放回调：递增 generation 并唤醒 waitIngestDataReleased。
        let data = MemoryIngestData::new(
            deduplicated,
            self.timestamp,
            Arc::clone(&self.imported_kv_size),
            Arc::clone(&self.imported_kv_count),
            move || {
                in_flight_bytes.fetch_sub(retained_bytes, Ordering::AcqRel);
                let (lock, condvar) = &*signal;
                if let Ok(mut pending) = lock.lock() {
                    // Go uses a capacity-one channel: multiple releases may
                    // coalesce, but one signal remains observable by a later
                    // waiter even after the in-flight count reaches zero.
                    *pending = true;
                    condvar.notify_all();
                }
                in_flight.fetch_sub(1, Ordering::AcqRel);
            },
        );
        self.active_ingest_data_flags
            .push(Arc::clone(&data.inner.released));
        let sorted_ranges = job_keys
            .windows(2)
            .map(|keys| KeyRange {
                start: keys[0].clone(),
                end: keys[1].clone(),
            })
            .collect();
        Ok(DataAndRanges {
            data,
            sorted_ranges,
        })
    }

    /// Resource changes recreate the load buffers only after downstream has
    /// released every previously generated batch, as in Go's external engine.
    fn handle_concurrency_change(
        &mut self,
        token: &CancellationToken,
        current_batch_size: usize,
    ) -> Result<usize> {
        let new_batch_size = self.resource.concurrency.load(Ordering::Acquire).max(1) as usize;
        self.update_active_ingest_data_flags();
        if new_batch_size == current_batch_size {
            return Ok(current_batch_size);
        }
        while !self.active_ingest_data_flags.is_empty() {
            self.wait_ingest_data_released(token)?;
            self.update_active_ingest_data_flags();
        }
        self.Reset();
        let mut state = self.resource.ready.0.lock().map_err(|_| Error::Poisoned)?;
        state.applied = new_batch_size as i32;
        self.resource.ready.1.notify_all();
        while state.pending {
            if token.is_cancelled() {
                return Err(Error::Cancelled);
            }
            state = self
                .resource
                .ready
                .1
                .wait_timeout(state, std::time::Duration::from_millis(10))
                .map_err(|_| Error::Poisoned)?
                .0;
        }
        Ok(new_batch_size)
    }

    /// 剔除已释放的在途 `MemoryIngestData` 标记。
    fn update_active_ingest_data_flags(&mut self) {
        self.active_ingest_data_flags
            .retain(|released| !released.load(Ordering::Acquire));
    }

    /// 阻塞直至有一批在途数据被释放；若当前无在途则快速返回 OOM 错误。
    pub fn waitIngestDataReleased(&self) -> Result<()> {
        self.wait_ingest_data_released(&CancellationToken::default())
    }

    fn wait_ingest_data_released(&self, token: &CancellationToken) -> Result<()> {
        if token.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let (lock, condvar) = &*self.release_signal;
        let mut pending = lock.lock().map_err(|_| Error::Poisoned)?;
        if *pending {
            *pending = false;
            return Ok(());
        }
        if self.in_flight_data_count.load(Ordering::Acquire) == 0 {
            return Err(Error::OutOfMemory {
                requested: self
                    .resource
                    .memory_limit
                    .load(Ordering::Acquire)
                    .saturating_add(1),
                limit: self.resource.memory_limit.load(Ordering::Acquire),
            });
        }
        while !*pending {
            if token.is_cancelled() {
                return Err(Error::Cancelled);
            }
            pending = condvar
                .wait_timeout(pending, std::time::Duration::from_millis(10))
                .map_err(|_| Error::Poisoned)?
                .0;
        }
        *pending = false;
        Ok(())
    }

    /// 绑定工作池调谐器。
    pub fn SetWorkerPool(&mut self, worker_pool: Arc<dyn WorkerPoolTuner>) {
        *self.resource.worker_pool.lock().unwrap() = Some(worker_pool);
    }
    pub fn ResourceHandle(&self) -> Arc<EngineResource> {
        self.resource.clone()
    }
    /// Keep the existing entrypoint; native loaders share the same atomic
    /// controls through ResourceHandle rather than locking their entire load.
    pub fn UpdateResource(&mut self, concurrency: i32, memory_capacity: i64) -> Result<()> {
        self.resource.UpdateResource(concurrency, memory_capacity)
    }

    /// 返回计划导入的总 KV 大小与条数。
    pub fn KVStatistics(&self) -> (i64, i64) {
        (self.total_kv_size, self.total_kv_count)
    }

    /// 返回下游 `Finish` 累计的已导入大小与条数。
    pub fn ImportedStatistics(&self) -> (i64, i64) {
        (
            self.imported_kv_size.load(Ordering::Relaxed),
            self.imported_kv_count.load(Ordering::Relaxed),
        )
    }

    /// 累计已加载（去重后）的 KV 条数。
    pub fn GetTotalLoadedKVsCount(&self) -> i64 {
        self.total_loaded_kvs_count.load(Ordering::Relaxed)
    }

    /// 返回 Record 策略下记录的冲突计数与重复键文件。
    pub fn ConflictInfo(&self) -> ConflictInfo {
        if self.recorded_duplicate_count == 0 {
            return ConflictInfo::default();
        }
        ConflictInfo {
            count: self.recorded_duplicate_count as u64,
            files: vec![format!("{}/dup", self.file_prefix.trim_end_matches('/'))],
        }
    }

    /// 返回与 Go external engine 一致的固定引擎标识。
    pub fn ID(&self) -> &str {
        "external"
    }

    /// 当前重复键处理策略。
    pub fn GetOnDup(&self) -> OnDuplicateKey {
        self.on_duplicate
    }

    /// 引擎覆盖的全局键范围。
    pub fn GetKeyRange(&self) -> (Vec<u8>, Vec<u8>) {
        (self.start_key.clone(), self.end_key.clone())
    }

    /// Region 分裂建议键列表。
    pub fn GetRegionSplitKeys(&self) -> Vec<Vec<u8>> {
        self.split_keys.clone()
    }

    /// 清空已加载缓冲并标记关闭。
    pub fn Close(&mut self) -> Result<()> {
        self.loaded.clear();
        self.closed = true;
        Ok(())
    }

    /// 清空已加载缓冲，保留引擎其它状态。
    pub fn Reset(&mut self) {
        self.loaded.clear();
    }

    /// 是否启用热点检测。
    pub fn check_hotspot(&self) -> bool {
        self.check_hotspot
    }
}

/// `MemoryIngestData` 的共享内部状态。
struct MemoryIngestDataInner {
    kvs: RwLock<Arc<Vec<KvPair>>>,
    timestamp: u64,
    released: Arc<AtomicBool>,
    reference_count: AtomicI64,
    imported_kv_size: Arc<AtomicI64>,
    imported_kv_count: Arc<AtomicI64>,
    on_release: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}
impl Drop for MemoryIngestDataInner {
    fn drop(&mut self) {
        // A cancelled generator can discard a batch before any job Ref. Return
        // its allocation before publishing the release signal, just as DecRef.
        if !self.released.swap(true, Ordering::AcqRel) {
            if let Ok(kvs) = self.kvs.get_mut() {
                *kvs = Arc::new(Vec::new());
            }
            if let Ok(callback) = self.on_release.get_mut()
                && let Some(callback) = callback.take()
            {
                callback();
            }
        }
    }
}

/// 内存中的可导入数据批次：支持范围查询、迭代与引用计数释放。
#[derive(Clone)]
pub struct MemoryIngestData {
    inner: Arc<MemoryIngestDataInner>,
}

impl MemoryIngestData {
    /// 构造批次；`on_release` 在引用归零时最多调用一次。
    pub(crate) fn new(
        kvs: Vec<KvPair>,
        timestamp: u64,
        imported_kv_size: Arc<AtomicI64>,
        imported_kv_count: Arc<AtomicI64>,
        on_release: impl FnOnce() + Send + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(MemoryIngestDataInner {
                kvs: RwLock::new(Arc::new(kvs)),
                timestamp,
                released: Arc::new(AtomicBool::new(false)),
                reference_count: AtomicI64::new(0),
                imported_kv_size,
                imported_kv_count,
                on_release: Mutex::new(Some(Box::new(on_release))),
            }),
        }
    }

    /// 二分定位半开区间 `[lower, upper)` 内首尾下标；空区间返回 `None`。
    fn first_and_last_key_index(
        &self,
        lower_bound: &[u8],
        upper_bound: &[u8],
    ) -> Result<Option<(usize, usize)>> {
        let kvs = self.inner.kvs.read().map_err(|_| Error::Poisoned)?;
        if kvs.is_empty() {
            return Ok(None);
        }
        let first = if lower_bound.is_empty() {
            0
        } else {
            kvs.partition_point(|pair| pair.key.as_slice() < lower_bound)
        };
        let exclusive_last = if upper_bound.is_empty() {
            kvs.len()
        } else {
            kvs.partition_point(|pair| pair.key.as_slice() < upper_bound)
        };
        if first >= exclusive_last {
            Ok(None)
        } else {
            Ok(Some((first, exclusive_last - 1)))
        }
    }

    /// 返回范围内首尾键；空范围返回空向量对。
    pub fn GetFirstAndLastKey(
        &self,
        lower_bound: &[u8],
        upper_bound: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        let Some((first, last)) = self.first_and_last_key_index(lower_bound, upper_bound)? else {
            return Ok((Vec::new(), Vec::new()));
        };
        let kvs = self.inner.kvs.read().map_err(|_| Error::Poisoned)?;
        Ok((kvs[first].key.clone(), kvs[last].key.clone()))
    }

    /// Share the sorted ingest buffer; independent range cursors allocate no
    /// additional KV payload outside the engine's memory budget.
    pub fn NewIter(&self, lower_bound: &[u8], upper_bound: &[u8]) -> Result<MemoryDataIter> {
        let Some((first, last)) = self.first_and_last_key_index(lower_bound, upper_bound)? else {
            return Ok(MemoryDataIter::empty());
        };
        let kvs = self.inner.kvs.read().map_err(|_| Error::Poisoned)?;
        Ok(MemoryDataIter {
            kvs: Arc::clone(&kvs),
            start: first,
            end: last + 1,
            current: None,
        })
    }

    /// 批次时间戳（TS）。
    pub fn GetTS(&self) -> u64 {
        self.inner.timestamp
    }

    /// 增加引用；已释放后调用会 panic。
    pub fn IncRef(&self) {
        assert!(
            !self.inner.released.load(Ordering::Acquire),
            "data shouldn't be released when IncRef"
        );
        self.inner.reference_count.fetch_add(1, Ordering::AcqRel);
    }

    /// 减少引用；归零时触发 `release`。
    pub fn DecRef(&self) {
        let previous = self.inner.reference_count.fetch_sub(1, Ordering::AcqRel);
        assert!(previous > 0, "DecRef without a matching IncRef");
        if previous == 1 {
            self.release();
        }
    }

    /// 清空 KV 并执行一次性释放回调（幂等）。
    pub fn release(&self) {
        if self
            .inner
            .released
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        if let Ok(mut kvs) = self.inner.kvs.write() {
            *kvs = Arc::new(Vec::new());
        }
        if let Ok(mut callback) = self.inner.on_release.lock()
            && let Some(callback) = callback.take()
        {
            callback();
        }
    }

    /// 累加已导入字节与条数到引擎共享计数器。
    pub fn Finish(&self, total_bytes: i64, total_count: i64) {
        self.inner
            .imported_kv_size
            .fetch_add(total_bytes, Ordering::Relaxed);
        self.inner
            .imported_kv_count
            .fetch_add(total_count, Ordering::Relaxed);
    }
}

/// 内存 KV 前向迭代器。
pub struct MemoryDataIter {
    kvs: Arc<Vec<KvPair>>,
    start: usize,
    end: usize,
    current: Option<usize>,
}

impl MemoryDataIter {
    fn empty() -> Self {
        Self {
            kvs: Arc::new(Vec::new()),
            start: 0,
            end: 0,
            current: None,
        }
    }

    /// 定位首元素。
    pub fn First(&mut self) -> bool {
        self.current = (self.start < self.end).then_some(self.start);
        self.Valid()
    }

    /// 当前位置是否有效。
    pub fn Valid(&self) -> bool {
        self.current.is_some_and(|index| index < self.end)
    }

    /// 前进一位。
    pub fn Next(&mut self) -> bool {
        if let Some(index) = self.current.as_mut() {
            *index += 1;
        }
        self.Valid()
    }

    /// 当前键；须先 `Valid()`。
    pub fn Key(&self) -> &[u8] {
        &self.kvs[self.current.expect("iterator is not valid")].key
    }

    /// 当前值；须先 `Valid()`。
    pub fn Value(&self) -> &[u8] {
        &self.kvs[self.current.expect("iterator is not valid")].value
    }

    /// 关闭并清空内部缓冲。
    pub fn Close(&mut self) -> Result<()> {
        self.kvs = Arc::new(Vec::new());
        self.start = 0;
        self.end = 0;
        self.current = None;
        Ok(())
    }

    /// 内存实现无错误。
    pub fn Error(&self) -> Option<&Error> {
        None
    }

    /// 无额外缓冲可释放（占位以对齐 Go 接口）。
    pub fn ReleaseBuf(&mut self) {}
}
