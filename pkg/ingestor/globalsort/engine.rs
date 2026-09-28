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

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};

use crate::reader::{CancellationToken, MemKvsAndBuffers, read_all_data};
use crate::{ConflictInfo, Error, KeyRange, KvPair, OnDuplicateKey, Result, Storage, encode_kvs};

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
    release_signal: Arc<(Mutex<bool>, Condvar)>,
    active_ingest_data_flags: Vec<Arc<AtomicBool>>,
    worker_pool: Option<Arc<dyn WorkerPoolTuner>>,
    check_hotspot: bool,
    worker_concurrency: AtomicI32,
    timestamp: u64,
    total_kv_size: i64,
    total_kv_count: i64,
    imported_kv_size: Arc<AtomicI64>,
    imported_kv_count: Arc<AtomicI64>,
    memory_limit: usize,
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
        release_signal: Arc::new((Mutex::new(false), Condvar::new())),
        active_ingest_data_flags: Vec::new(),
        worker_pool: None,
        check_hotspot,
        worker_concurrency: AtomicI32::new(worker_concurrency),
        timestamp,
        total_kv_size,
        total_kv_count,
        imported_kv_size: Arc::new(AtomicI64::new(0)),
        imported_kv_count: Arc::new(AtomicI64::new(0)),
        memory_limit: getEngineMemoryLimit(memory_capacity),
        on_duplicate,
        file_prefix,
        recorded_duplicate_count: 0,
        recorded_duplicate_size: 0,
        duplicate_pairs: Vec::new(),
        closed: false,
    })
}

impl Engine {
    /// 按 worker 并发度分批加载 job 键区间数据，返回全部批次；结束后按需写出重复键文件。
    pub fn LoadIngestData(&mut self, token: &CancellationToken) -> Result<Vec<DataAndRanges>> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.job_keys.len() < 2 {
            return Ok(Vec::new());
        }
        let mut outputs = Vec::new();
        let mut start = 0;
        let mut current_batch_size = self.worker_concurrency.load(Ordering::Acquire) as usize;
        while start + 1 < self.job_keys.len() {
            if token.is_cancelled() {
                return Err(Error::Cancelled);
            }
            current_batch_size = self.handle_concurrency_change(current_batch_size);
            let end = (start + current_batch_size + 1).min(self.job_keys.len());
            outputs.push(self.load_range_batch_data(token, &self.job_keys[start..end].to_vec())?);
            start += current_batch_size;
        }
        self.close_duplicate_writer_as_needed()?;
        Ok(outputs)
    }

    /// 加载 `[job_keys.first, job_keys.last)` 范围内的 KV，排序去重后封装为批次。
    fn load_range_batch_data(
        &mut self,
        token: &CancellationToken,
        job_keys: &[Vec<u8>],
    ) -> Result<DataAndRanges> {
        let start_key = job_keys.first().cloned().unwrap_or_default();
        let end_key = job_keys.last().cloned().unwrap_or_default();
        let starts = vec![0; self.data_files.len()];
        let mut ends = Vec::with_capacity(self.data_files.len());
        for file in &self.data_files {
            ends.push(self.storage.read(file)?.len() as u64);
        }
        read_all_data(
            token,
            self.storage.as_ref(),
            &self.data_files,
            &self.stats_files,
            &start_key,
            &end_key,
            &starts,
            &ends,
            self.memory_limit,
            &mut self.loaded,
        )?;
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
                OnDuplicateKey::Remove | OnDuplicateKey::Record => {}
            }
        }
        // Matches simplesst.RemoveDuplicates(kvs, key, recordRemoved) with
        // keptDupCnt=0: any key occurring more than once is dropped in full
        // (not just the repeats after the first), and Record additionally
        // keeps every dropped copy for the duplicate-key writer.
        // 出现次数 >1 的 key 整组丢弃；Record 另将丢弃副本写入 duplicate_pairs。
        let sorted = std::mem::take(&mut self.loaded.kvs);
        let mut deduplicated = Vec::with_capacity(sorted.len());
        let mut run_start = 0;
        while run_start < sorted.len() {
            let mut run_end = run_start + 1;
            while run_end < sorted.len() && sorted[run_end].key == sorted[run_start].key {
                run_end += 1;
            }
            if run_end - run_start == 1 {
                deduplicated.push(sorted[run_start].clone());
            } else if self.on_duplicate == OnDuplicateKey::Record {
                for pair in &sorted[run_start..run_end] {
                    self.recorded_duplicate_count += 1;
                    self.recorded_duplicate_size += pair.encoded_size() as i64;
                    self.duplicate_pairs.push(pair.clone());
                }
            }
            run_start = run_end;
        }
        self.total_loaded_kvs_count
            .fetch_add(deduplicated.len() as i64, Ordering::Relaxed);
        self.loaded.size = 0;
        let in_flight = Arc::clone(&self.in_flight_data_count);
        let signal = Arc::clone(&self.release_signal);
        in_flight.fetch_add(1, Ordering::AcqRel);
        // 释放回调：递增 generation 并唤醒 waitIngestDataReleased。
        let data = MemoryIngestData::new(
            deduplicated,
            self.timestamp,
            Arc::clone(&self.imported_kv_size),
            Arc::clone(&self.imported_kv_count),
            move || {
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

    /// 若并发度变化且无在途批次，则 Reset 缓冲区并采用新批次大小。
    fn handle_concurrency_change(&mut self, current_batch_size: usize) -> usize {
        let new_batch_size = self.worker_concurrency.load(Ordering::Acquire).max(1) as usize;
        self.update_active_ingest_data_flags();
        if new_batch_size != current_batch_size && self.active_ingest_data_flags.is_empty() {
            self.Reset();
            new_batch_size
        } else {
            current_batch_size
        }
    }

    /// 剔除已释放的在途 `MemoryIngestData` 标记。
    fn update_active_ingest_data_flags(&mut self) {
        self.active_ingest_data_flags
            .retain(|released| !released.load(Ordering::Acquire));
    }

    /// 阻塞直至有一批在途数据被释放；若当前无在途则快速返回 OOM 错误。
    pub fn waitIngestDataReleased(&self) -> Result<()> {
        let (lock, condvar) = &*self.release_signal;
        let mut pending = lock.lock().map_err(|_| Error::Poisoned)?;
        if *pending {
            *pending = false;
            return Ok(());
        }
        if self.in_flight_data_count.load(Ordering::Acquire) == 0 {
            return Err(Error::OutOfMemory {
                requested: self.memory_limit.saturating_add(1),
                limit: self.memory_limit,
            });
        }
        pending = condvar
            .wait_while(pending, |value| !*value)
            .map_err(|_| Error::Poisoned)?;
        *pending = false;
        Ok(())
    }

    /// 绑定工作池调谐器。
    pub fn SetWorkerPool(&mut self, worker_pool: Arc<dyn WorkerPoolTuner>) {
        self.worker_pool = Some(worker_pool);
    }

    /// 更新并发度与内存容量，并通知工作池。
    pub fn UpdateResource(&mut self, concurrency: i32, memory_capacity: i64) -> Result<()> {
        let worker_pool = self.worker_pool.as_ref().ok_or_else(|| {
            Error::InvalidArgument("region job worker is not initialized, retry later".into())
        })?;
        // Go treats an unchanged concurrency as a no-op before applying the
        // new memory capacity or tuning the worker pool.
        if self.worker_concurrency.load(Ordering::Acquire) == concurrency {
            return Ok(());
        }
        if concurrency <= 0 || memory_capacity <= 0 {
            return Err(Error::InvalidArgument(
                "concurrency and memory capacity must be positive".into(),
            ));
        }
        self.worker_concurrency
            .store(concurrency, Ordering::Release);
        self.memory_limit = getEngineMemoryLimit(memory_capacity);
        worker_pool.Tune(concurrency as usize);
        Ok(())
    }

    /// Record 策略下将收集到的重复键写出到 `{prefix}/dup`。
    fn close_duplicate_writer_as_needed(&mut self) -> Result<()> {
        if self.on_duplicate != OnDuplicateKey::Record || self.duplicate_pairs.is_empty() {
            return Ok(());
        }
        let path = format!("{}/dup", self.file_prefix.trim_end_matches('/'));
        self.storage.write(&path, encode_kvs(&self.duplicate_pairs))
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
    kvs: RwLock<Vec<KvPair>>,
    timestamp: u64,
    released: Arc<AtomicBool>,
    reference_count: AtomicI64,
    imported_kv_size: Arc<AtomicI64>,
    imported_kv_count: Arc<AtomicI64>,
    on_release: Mutex<Option<Box<dyn FnOnce() + Send>>>,
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
                kvs: RwLock::new(kvs),
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

    /// 构造覆盖范围内 KV 的前向迭代器副本。
    pub fn NewIter(&self, lower_bound: &[u8], upper_bound: &[u8]) -> Result<MemoryDataIter> {
        let Some((first, last)) = self.first_and_last_key_index(lower_bound, upper_bound)? else {
            return Ok(MemoryDataIter::empty());
        };
        let kvs = self.inner.kvs.read().map_err(|_| Error::Poisoned)?;
        Ok(MemoryDataIter {
            kvs: kvs[first..=last].to_vec(),
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
            kvs.clear();
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
    kvs: Vec<KvPair>,
    current: Option<usize>,
}

impl MemoryDataIter {
    fn empty() -> Self {
        Self {
            kvs: Vec::new(),
            current: None,
        }
    }

    /// 定位首元素。
    pub fn First(&mut self) -> bool {
        self.current = (!self.kvs.is_empty()).then_some(0);
        self.Valid()
    }

    /// 当前位置是否有效。
    pub fn Valid(&self) -> bool {
        self.current.is_some_and(|index| index < self.kvs.len())
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
        self.kvs.clear();
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
