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

// 重叠数据文件归并（merge）：按并发度分组后排序去重，写出新的 data/stat 文件。
//
// 对应 Go globalsort merge 路径；`OnDuplicateKey` 控制重复键（同一 key 多值）处理，
// 并可通过 `Collector` 与 `OnWriterClose` 上报进度与写出摘要。

use astersql_resourcemanager_pool_workerpool as pool;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::reader::{CancellationToken, read_stream_pair};
use crate::{ConflictInfo, Error, KvPair, OnDuplicateKey, Result, Storage, WriterSummary};

/// 单线程最多同时归并的文件数上限（可运行时调整）。
pub static MaxMergingFilesPerThread: AtomicUsize = AtomicUsize::new(250);
/// 对象存储分片上传的最小 part 大小（5MiB）。
pub const MinUploadPartSize: i64 = 5 * 1024 * 1024;

/// 归并过程中的字节/行数收集回调。
pub trait Collector: Send + Sync {
    fn Accepted(&self, _bytes: i64) {}
    fn Processed(&self, bytes: i64, row_count: i64);
}

/// 子任务级累计的已处理字节与行数。
#[derive(Default)]
pub struct SubtaskSummary {
    pub processed: AtomicI64,
    pub row_count: AtomicI64,
}

/// 默认收集器：可选写入 `SubtaskSummary`，并本地累计 metric 字节。
pub struct MergeCollector {
    summary: Option<Arc<SubtaskSummary>>,
    metric_bytes: AtomicI64,
}

/// 构造 `MergeCollector`；`summary` 为 `None` 时仅累计本地 metric。
pub fn NewMergeCollector(summary: Option<Arc<SubtaskSummary>>) -> MergeCollector {
    MergeCollector {
        summary,
        metric_bytes: AtomicI64::new(0),
    }
}

impl Collector for MergeCollector {
    fn Processed(&self, bytes: i64, row_count: i64) {
        if let Some(summary) = &self.summary {
            summary.processed.fetch_add(bytes, Ordering::Relaxed);
            summary.row_count.fetch_add(row_count, Ordering::Relaxed);
        }
        self.metric_bytes.fetch_add(bytes, Ordering::Relaxed);
    }
}

/// Writer 关闭时的回调类型。
pub type OnWriterClose = Arc<dyn Fn(&WriterSummary) + Send + Sync>;

/// 归并算子：持有存储、并发度、重复键策略与冲突信息。
pub struct MergeOperator {
    token: CancellationToken,
    store: Arc<dyn Storage>,
    part_size: i64,
    new_file_prefix: String,
    block_size: usize,
    on_writer_close: Option<OnWriterClose>,
    collector: Option<Arc<dyn Collector>>,
    concurrency: usize,
    check_hotspot: bool,
    on_duplicate: OnDuplicateKey,
    next_writer_id: AtomicUsize,
    conflict_info: Arc<Mutex<ConflictInfo>>,
    running_pool: Mutex<Option<Arc<Mutex<pool::WorkerPool<MergeTask, (usize, String)>>>>>,
}

/// 创建归并算子；`concurrency` 必须大于 0，`part_size` 会抬升到至少 `MinUploadPartSize`。
#[allow(clippy::too_many_arguments)]
pub fn NewMergeOperator(
    token: CancellationToken,
    store: Arc<dyn Storage>,
    part_size: i64,
    new_file_prefix: impl Into<String>,
    block_size: usize,
    on_writer_close: Option<OnWriterClose>,
    collector: Option<Arc<dyn Collector>>,
    concurrency: usize,
    check_hotspot: bool,
    on_duplicate: OnDuplicateKey,
) -> Result<MergeOperator> {
    if concurrency == 0 {
        return Err(Error::InvalidArgument(
            "merge concurrency must be greater than zero".into(),
        ));
    }
    Ok(MergeOperator {
        token,
        store,
        // 预留 1MiB 余量后再与最小 part 取 max，对齐 Go 侧上传分片约束。
        part_size: part_size.saturating_add(1024 * 1024).max(MinUploadPartSize),
        new_file_prefix: new_file_prefix.into(),
        block_size,
        on_writer_close,
        collector,
        concurrency,
        check_hotspot,
        on_duplicate,
        next_writer_id: AtomicUsize::new(0),
        conflict_info: Arc::new(Mutex::new(ConflictInfo::default())),
        running_pool: Mutex::new(None),
    })
}

impl MergeOperator {
    /// 算子名称，便于日志标识。
    pub fn String(&self) -> &'static str {
        "mergeOperator"
    }

    /// 返回当前累计的冲突信息快照。
    pub fn conflict_info(&self) -> Result<ConflictInfo> {
        Ok(self
            .conflict_info
            .lock()
            .map_err(|_| Error::Poisoned)?
            .clone())
    }
}

struct MergeTask {
    id: usize,
    files: Vec<String>,
}
impl pool::TaskMayPanic for MergeTask {
    fn RecoverArgs(&self) -> (String, String, Option<pool::Error>) {
        (
            "merge-sort".into(),
            format!("writer {}, {} input files", self.id, self.files.len()),
            None,
        )
    }
}
struct MergeWorker {
    token: CancellationToken,
    store: Arc<dyn Storage>,
    part_size: i64,
    prefix: String,
    block_size: usize,
    writer_closed: Option<OnWriterClose>,
    collector: Option<Arc<dyn Collector>>,
    hotspot: bool,
    duplicate: OnDuplicateKey,
    groups: usize,
    conflicts: Arc<Mutex<ConflictInfo>>,
    error: Arc<Mutex<Option<Error>>>,
}
impl pool::Worker<MergeTask, (usize, String)> for MergeWorker {
    fn HandleTask(
        &mut self,
        task: MergeTask,
        send: &mut dyn FnMut((usize, String)),
    ) -> std::result::Result<(), pool::Error> {
        match merge_overlapping_files_internal(
            &self.token,
            &task.files,
            self.store.as_ref(),
            self.part_size,
            &self.prefix,
            &task.id.to_string(),
            self.block_size,
            self.writer_closed.as_ref(),
            self.collector.as_deref(),
            self.hotspot,
            self.duplicate,
            self.groups,
            &self.conflicts,
        ) {
            Ok(path) => {
                send((task.id, path));
                Ok(())
            }
            Err(error) => {
                let mut first = self.error.lock().unwrap();
                if first.is_none() {
                    *first = Some(error.clone());
                }
                self.token.cancel();
                Err(pool::Error::new(error.to_string()))
            }
        }
    }
    fn Close(&mut self) -> std::result::Result<(), pool::Error> {
        Ok(())
    }
}
struct MergeRun<'a> {
    operator: &'a MergeOperator,
    pool: Arc<Mutex<pool::WorkerPool<MergeTask, (usize, String)>>>,
    input: pool::Channel<MergeTask>,
    stopped: Arc<AtomicBool>,
}
impl Drop for MergeRun<'_> {
    fn drop(&mut self) {
        let mut running = self.operator.running_pool.lock().unwrap();
        if running
            .as_ref()
            .is_some_and(|pool| Arc::ptr_eq(pool, &self.pool))
        {
            *running = None;
        }
        drop(running);
        let mut pool = self.pool.lock().unwrap();
        self.input.close();
        pool.Release();
        self.stopped.store(true, Ordering::Release);
    }
}
impl MergeOperator {
    /// Tune the running file-group workers, joining retired workers before
    /// returning. Initial capacity alone is not a resource-update result.
    pub fn Tune(&self, concurrency: usize) -> Result<()> {
        let concurrency = i32::try_from(concurrency)
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| Error::InvalidArgument("invalid merge concurrency".into()))?;
        let pool = self
            .running_pool
            .lock()
            .map_err(|_| Error::Poisoned)?
            .clone()
            .ok_or_else(|| Error::InvalidArgument("merge sort operator is not started".into()))?;
        pool.lock()
            .map_err(|_| Error::Poisoned)?
            .Tune(concurrency, true);
        Ok(())
    }
}

/// Merge independent file groups through the original worker-pool lifecycle.
/// Readers/uploads stay local to each HandleTask; cancellation joins all of them
/// before returning, and typed duplicate/storage errors retain their identity.
pub fn MergeOverlappingFiles(paths: &[String], op: &MergeOperator) -> Result<Vec<String>> {
    if op.token.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let groups = splitDataFiles(paths, op.concurrency);
    if groups.is_empty() {
        return Ok(Vec::new());
    }
    let group_count = groups.len();
    let context = pool::NewContext(pool::Context::background());
    let input = pool::Channel::bounded(1);
    let error = Arc::new(Mutex::new(None));
    let token = op.token.clone();
    let store = op.store.clone();
    let prefix = op.new_file_prefix.clone();
    let writer_closed = op.on_writer_close.clone();
    let collector = op.collector.clone();
    let conflicts = op.conflict_info.clone();
    let part_size = op.part_size;
    let block_size = op.block_size;
    let hotspot = op.check_hotspot;
    let duplicate = op.on_duplicate;
    let mut workers = pool::WorkerPool::NewWorkerPool("merge-sort", (), op.concurrency as i32, {
        let error = error.clone();
        move || MergeWorker {
            token: token.clone(),
            store: store.clone(),
            part_size,
            prefix: prefix.clone(),
            block_size,
            writer_closed: writer_closed.clone(),
            collector: collector.clone(),
            hotspot,
            duplicate,
            groups: group_count,
            conflicts: conflicts.clone(),
            error: error.clone(),
        }
    });
    workers.SetTaskReceiver(input.clone());
    workers.Start(context.clone());
    let results = workers.GetResultChan().unwrap();
    let workers = Arc::new(Mutex::new(workers));
    *op.running_pool.lock().map_err(|_| Error::Poisoned)? = Some(workers.clone());
    let stopped = Arc::new(AtomicBool::new(false));
    let mut outputs = std::collections::BTreeMap::new();
    std::thread::scope(|scope| {
        let monitor = scope.spawn(|| {
            while !stopped.load(Ordering::Acquire) {
                if op.token.is_cancelled() || context.IsCancelled() {
                    op.token.cancel();
                    context.Cancel();
                    input.close();
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        });
        let producer = scope.spawn(|| {
            for files in groups {
                if op.token.is_cancelled() {
                    break;
                }
                let id = op.next_writer_id.fetch_add(1, Ordering::Relaxed);
                if !input.send(MergeTask { id, files }) {
                    break;
                }
            }
        });
        let running = MergeRun {
            operator: op,
            pool: workers,
            input: input.clone(),
            stopped: stopped.clone(),
        };
        while outputs.len() < group_count {
            match results.recv_timeout(std::time::Duration::from_millis(1)) {
                Ok(Some((id, path))) => {
                    outputs.insert(id, path);
                }
                Ok(None) | Err(pool::RecvTimeoutError::Disconnected) => break,
                Err(pool::RecvTimeoutError::Timeout) => {
                    if context.IsCancelled() || op.token.is_cancelled() {
                        break;
                    }
                }
            }
        }
        drop(running);
        producer.join().unwrap();
        monitor.join().unwrap();
    });
    if let Some(error) = error.lock().map_err(|_| Error::Poisoned)?.take() {
        return Err(error);
    }
    if let Some(error) = context.OperatorErr() {
        return Err(Error::InvalidData(error.to_string()));
    }
    if outputs.len() != group_count || op.token.is_cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(outputs.into_values().collect())
}

/// 计算按当前并发度切分后的分组数。
fn groups_len(paths: &[String], concurrency: usize) -> usize {
    splitDataFiles(paths, concurrency).len()
}

/// Exact number of outputs produced by splitDataFiles.
pub fn getTargetFileCount(file_count: usize, concurrency: usize) -> usize {
    if file_count == 0 {
        return 0;
    }
    let concurrency = concurrency.max(1);
    let maximum = MaxMergingFilesPerThread.load(Ordering::Relaxed).max(1);
    let shares = file_count.div_ceil(maximum).max(concurrency);
    if file_count < 2 * concurrency {
        (file_count / 2).max(1)
    } else {
        shares
    }
}

/// Outputs for groups whose input sizes differ by at most one file.
pub fn getGroupedTargetFileCount(total: usize, groups: usize, concurrency: usize) -> usize {
    let quotient = total / groups;
    let remainder = total % groups;
    remainder * getTargetFileCount(quotient + 1, concurrency)
        + (groups - remainder) * getTargetFileCount(quotient, concurrency)
}

/// 将文件路径列表切成约 `concurrency` 份，并受 `MaxMergingFilesPerThread` 约束。
pub fn splitDataFiles(paths: &[String], concurrency: usize) -> Vec<Vec<String>> {
    let shares = getTargetFileCount(paths.len(), concurrency);
    if shares == 0 {
        return Vec::new();
    }
    let batch_count = paths.len() / shares;
    let mut remainder = paths.len() % shares;
    let mut start = 0;
    let mut groups = Vec::with_capacity(shares);
    while start < paths.len() {
        let size = batch_count + usize::from(remainder > 0);
        remainder = remainder.saturating_sub(1);
        groups.push(paths[start..start + size].to_vec());
        start += size;
    }
    groups
}

/// 读取一组路径、排序去重后写出 `{prefix}/{writer_id}.data/.stat`。
#[allow(clippy::too_many_arguments)]
pub fn merge_overlapping_files_internal(
    token: &CancellationToken,
    paths: &[String],
    store: &dyn Storage,
    _part_size: i64,
    new_file_prefix: &str,
    writer_id: &str,
    _block_size: usize,
    on_writer_close: Option<&OnWriterClose>,
    collector: Option<&dyn Collector>,
    _check_hotspot: bool,
    on_duplicate: OnDuplicateKey,
    _file_group_count: usize,
    conflicts: &Mutex<ConflictInfo>,
) -> Result<String> {
    use std::collections::BinaryHeap;
    if token.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let mut readers = paths
        .iter()
        .map(|path| store.open(path))
        .collect::<Result<Vec<_>>>()?;
    let mut heap = BinaryHeap::new();
    for (source, reader) in readers.iter_mut().enumerate() {
        if let Some(pair) = read_stream_pair(reader.as_mut(), store.record_format())? {
            heap.push(MergeHead { pair, source });
        }
    }
    let prefix = new_file_prefix.trim_end_matches('/');
    let data_file = format!("{prefix}/{writer_id}.data");
    let stat_file = format!("{prefix}/{writer_id}.stat");
    let duplicate_file = format!("{prefix}/{writer_id}.dup");
    let mut output = store.create(&data_file)?;
    let mut duplicates: Option<Box<dyn crate::ObjectWriter + '_>> = None;
    let mut summary = StreamSummary::new(store.record_format(), 1024 * 1024, 8 * 1024);
    let mut previous: Option<Vec<u8>> = None;
    let mut ordinal = 0_u64;
    let mut pending_single: Option<KvPair> = None;
    while let Some(head) = heap.pop() {
        if token.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let pair = head.pair;
        let same = previous.as_ref().is_some_and(|key| *key == pair.key);
        if !same {
            if let Some(single) = pending_single.take() {
                summary.write(output.as_mut(), &single)?;
            }
            previous = Some(pair.key.clone());
            ordinal = 0;
        }
        ordinal = ordinal.wrapping_add(1);
        match on_duplicate {
            OnDuplicateKey::Ignore => summary.write(output.as_mut(), &pair)?,
            OnDuplicateKey::Error => {
                if same {
                    return Err(Error::DuplicateKey {
                        key: pair.key,
                        value: pair.value,
                    });
                }
                summary.write(output.as_mut(), &pair)?;
            }
            OnDuplicateKey::Remove => {
                if same {
                    pending_single = None;
                } else {
                    pending_single = Some(pair.clone());
                }
            }
            OnDuplicateKey::Record => {
                if ordinal <= 2 {
                    summary.write(output.as_mut(), &pair)?;
                } else {
                    if duplicates.is_none() {
                        duplicates = Some(store.create(&duplicate_file)?);
                    }
                    write_stream_pair(
                        duplicates.as_mut().unwrap().as_mut(),
                        &pair,
                        store.record_format(),
                    )?;
                    let mut info = conflicts.lock().map_err(|_| Error::Poisoned)?;
                    info.count = info.count.saturating_add(1);
                }
            }
        }
        collect_processed(collector, &pair);
        if let Some(next) = read_stream_pair(readers[head.source].as_mut(), store.record_format())?
        {
            // Every SST input is individually sorted; retaining one head per
            // reader bounds payload memory by the file count, not total rows.
            if next.key < pair.key {
                return Err(Error::InvalidData("merge input is not sorted".into()));
            }
            heap.push(MergeHead {
                pair: next,
                source: head.source,
            });
        }
    }
    if let Some(single) = pending_single {
        summary.write(output.as_mut(), &single)?;
    }
    output.finish()?;
    summary.write_stats(store, &stat_file)?;
    if let Some(writer) = duplicates {
        writer.finish()?;
        conflicts
            .lock()
            .map_err(|_| Error::Poisoned)?
            .files
            .push(duplicate_file);
    }
    let mut summary = summary.finish(data_file.clone(), stat_file);
    summary.conflict_info = conflicts.lock().map_err(|_| Error::Poisoned)?.clone();
    if let Some(callback) = on_writer_close {
        callback(&summary);
    }
    Ok(data_file)
}

struct MergeHead {
    pair: KvPair,
    source: usize,
}
impl PartialEq for MergeHead {
    fn eq(&self, other: &Self) -> bool {
        self.pair.key == other.pair.key && self.source == other.source
    }
}
impl Eq for MergeHead {}
impl Ord for MergeHead {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .pair
            .key
            .cmp(&self.pair.key)
            .then_with(|| other.source.cmp(&self.source))
    }
}
impl PartialOrd for MergeHead {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

pub(crate) fn write_stream_pair(
    writer: &mut dyn crate::ObjectWriter,
    pair: &KvPair,
    format: crate::RecordFormat,
) -> Result<()> {
    match format {
        crate::RecordFormat::LegacyLittleEndian32 => {
            let key_len = u32::try_from(pair.key.len())
                .map_err(|_| Error::InvalidData("KV key too large".into()))?;
            let value_len = u32::try_from(pair.value.len())
                .map_err(|_| Error::InvalidData("KV value too large".into()))?;
            writer
                .write_all(&key_len.to_le_bytes())
                .map_err(crate::reader::io_error)?;
            writer
                .write_all(&value_len.to_le_bytes())
                .map_err(crate::reader::io_error)?;
        }
        crate::RecordFormat::GoBigEndian64 => {
            writer
                .write_all(&(pair.key.len() as u64).to_be_bytes())
                .map_err(crate::reader::io_error)?;
            writer
                .write_all(&(pair.value.len() as u64).to_be_bytes())
                .map_err(crate::reader::io_error)?;
        }
    }
    writer
        .write_all(&pair.key)
        .map_err(crate::reader::io_error)?;
    writer
        .write_all(&pair.value)
        .map_err(crate::reader::io_error)
}
#[derive(Default)]
pub(crate) struct StreamSummary {
    property_size_distance: u64,
    property_keys_distance: u64,
    format: crate::RecordFormat,
    min: Vec<u8>,
    max: Vec<u8>,
    size: u64,
    count: u64,
    properties: Vec<crate::RangeProperty>,
    current: crate::RangeProperty,
}
impl StreamSummary {
    pub(crate) fn new(
        format: crate::RecordFormat,
        property_size_distance: u64,
        property_keys_distance: u64,
    ) -> Self {
        Self {
            format,
            property_size_distance,
            property_keys_distance,
            ..Default::default()
        }
    }

    pub(crate) fn write(
        &mut self,
        writer: &mut dyn crate::ObjectWriter,
        pair: &KvPair,
    ) -> Result<()> {
        write_stream_pair(writer, pair, self.format)?;
        if self.count == 0 {
            self.min = pair.key.clone();
        }
        self.max = pair.key.clone();
        self.size = self.size.wrapping_add(pair.encoded_size() as u64);
        self.count = self.count.wrapping_add(1);
        if self.current.keys == 0 {
            self.current.first_key = pair.key.clone();
        }
        self.current.last_key = pair.key.clone();
        self.current.size += pair.encoded_size() as u64
            + if self.format == crate::RecordFormat::GoBigEndian64 {
                16
            } else {
                0
            };
        self.current.keys += 1;
        let property_ready = match self.format {
            crate::RecordFormat::LegacyLittleEndian32 => self.current.keys == 4,
            crate::RecordFormat::GoBigEndian64 => {
                self.current.keys >= self.property_keys_distance
                    || self.current.size >= self.property_size_distance
            }
        };
        if property_ready {
            self.properties.push(std::mem::take(&mut self.current));
        }
        Ok(())
    }
    pub(crate) fn write_stats(&mut self, store: &dyn Storage, path: &str) -> Result<()> {
        if self.current.keys != 0 {
            self.properties.push(std::mem::take(&mut self.current));
        }
        if self.format == crate::RecordFormat::LegacyLittleEndian32 {
            return store.write(path, Vec::new());
        }
        let mut writer = store.create(path)?;
        let mut offset = 0_u64;
        for property in &self.properties {
            let size = 32_usize
                .checked_add(property.first_key.len())
                .and_then(|n| n.checked_add(property.last_key.len()))
                .ok_or_else(|| Error::InvalidData("stat property size overflow".into()))?;
            let size = u32::try_from(size)
                .map_err(|_| Error::InvalidData("stat property too large".into()))?;
            writer
                .write_all(&size.to_be_bytes())
                .map_err(crate::reader::io_error)?;
            for key in [&property.first_key, &property.last_key] {
                let size = u32::try_from(key.len())
                    .map_err(|_| Error::InvalidData("stat key too large".into()))?;
                writer
                    .write_all(&size.to_be_bytes())
                    .map_err(crate::reader::io_error)?;
                writer.write_all(key).map_err(crate::reader::io_error)?;
            }
            for value in [property.size, property.keys, offset] {
                writer
                    .write_all(&value.to_be_bytes())
                    .map_err(crate::reader::io_error)?;
            }
            offset = offset.wrapping_add(property.size);
        }
        writer.finish()
    }
    pub(crate) fn finish(mut self, data_file: String, stat_file: String) -> WriterSummary {
        if self.current.keys != 0 {
            self.properties.push(self.current);
        }
        WriterSummary {
            min: self.min,
            max: self.max,
            total_size: self.size,
            total_count: self.count,
            multiple_files_stats: vec![crate::MultipleFilesStat {
                filenames: vec![crate::FilePair {
                    data_file,
                    stat_file,
                    properties: self.properties,
                }],
            }],
            conflict_info: Default::default(),
        }
    }
}

fn collect_processed(collector: Option<&dyn Collector>, pair: &KvPair) {
    if let Some(collector) = collector {
        collector.Processed(pair.encoded_size() as i64, 1);
    }
}
