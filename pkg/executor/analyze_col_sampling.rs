// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 列统计的行采样（reservoir sampling）构建路径。
//
// 从存储拉取原始样本包，多 worker 合并收集器后并行构建直方图/TopN/FM Sketch；
// 对前缀索引与虚拟列等特殊索引另走 NDV 下推。

#![allow(non_camel_case_types, non_snake_case)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

/// 参与直方图/TopN 构建的样本值最大字节长度。
const MAX_SAMPLE_VALUE_LENGTH: usize = 1024;
/// FM Sketch 哈希集合容量上限。
const MAX_SKETCH_SIZE: usize = 10_000;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
/// 采样行中的单元格值。
pub enum Datum {
    Null,
    Signed(i64),
    Unsigned(u64),
    Bytes(Vec<u8>),
    Text(String),
}

impl Default for Datum {
    fn default() -> Self {
        Self::Null
    }
}

impl Datum {
    /// 将 Datum 编码为字节序列（用于哈希/长度估计）。
    fn bytes(&self) -> Vec<u8> {
        match self {
            Self::Null => Vec::new(),
            Self::Signed(v) => v.to_be_bytes().to_vec(),
            Self::Unsigned(v) => v.to_be_bytes().to_vec(),
            Self::Bytes(v) => v.clone(),
            Self::Text(v) => v.as_bytes().to_vec(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 参与采样的列描述（含虚拟列与唯一性标记）。
pub struct ColumnInfo {
    pub id: i64,
    pub offset: usize,
    pub virtual_generated: bool,
    pub generated_stored: bool,
    pub string_type: bool,
    pub unique_single_column: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
/// 索引列偏移与可选前缀长度。
pub struct IndexColumn {
    pub offset: usize,
    pub prefix_length: Option<usize>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
/// 采样路径上的索引元信息。
pub struct IndexInfo {
    pub id: i64,
    pub columns: Vec<IndexColumn>,
    pub unique: bool,
    pub primary: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 列采样 ANALYZE 错误。
pub enum AnalyzeError {
    Backend(String),
    Cancelled,
    Decode(String),
    WorkerPanic(String),
    InvalidConfig(String),
}
impl Display for AnalyzeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for AnalyzeError {}

#[derive(Clone, Debug, Default)]
/// FM Sketch：估计列/索引 NDV。
pub struct FMSketch {
    pub hashes: BTreeSet<u64>,
}
impl FMSketch {
    /// 合并哈希并裁剪容量。
    fn merge(&mut self, other: &Self) {
        self.hashes.extend(other.hashes.iter().copied());
        while self.hashes.len() > MAX_SKETCH_SIZE {
            let Some(last) = self.hashes.last().copied() else {
                break;
            };
            self.hashes.remove(&last);
        }
    }
    /// 以集合大小估计 NDV。
    pub fn ndv(&self) -> u64 {
        self.hashes.len() as u64
    }
}

#[derive(Clone, Debug)]
/// 水库抽样中的一行样本（含优先级）。
pub struct ReservoirRowSampleItem {
    pub columns: Vec<Datum>,
    pub handle: i64,
    pub priority: u64,
}

#[derive(Clone, Debug)]
/// 行级水库抽样收集器：样本、NULL 计数、尺寸与 FM Sketch。
pub struct RowSampleCollector {
    pub sample_size: usize,
    pub count: i64,
    pub samples: Vec<ReservoirRowSampleItem>,
    pub null_count: Vec<i64>,
    pub total_sizes: Vec<i64>,
    pub fm_sketches: Vec<FMSketch>,
    pub mem_size: i64,
}

impl RowSampleCollector {
    /// 按样本容量与列+索引总长度初始化收集器。
    pub fn new(sample_size: usize, total_len: usize) -> Self {
        Self {
            sample_size,
            count: 0,
            samples: Vec::with_capacity(sample_size),
            null_count: vec![0; total_len],
            total_sizes: vec![0; total_len],
            fm_sketches: vec![FMSketch::default(); total_len],
            mem_size: 0,
        }
    }
    /// 合并另一收集器并按优先级截断到 sample_size。
    pub fn merge(&mut self, mut other: Self) {
        self.count += other.count;
        for (left, right) in self.null_count.iter_mut().zip(&other.null_count) {
            *left += right;
        }
        for (left, right) in self.total_sizes.iter_mut().zip(&other.total_sizes) {
            *left += right;
        }
        for (left, right) in self.fm_sketches.iter_mut().zip(&other.fm_sketches) {
            left.merge(right);
        }
        self.samples.append(&mut other.samples);
        self.samples
            .sort_by_key(|sample| std::cmp::Reverse(sample.priority));
        self.samples.truncate(self.sample_size);
        self.mem_size = collector_memory(&self.samples);
    }
}

#[derive(Clone, Debug, Default)]
/// 采样构建出的直方图桶。
pub struct HistogramBucket {
    pub lower: Datum,
    pub upper: Datum,
    pub count: u64,
}
#[derive(Clone, Debug, Default)]
/// 由样本构建的直方图。
pub struct Histogram {
    pub id: i64,
    pub ndv: u64,
    pub null_count: i64,
    pub buckets: Vec<HistogramBucket>,
}
#[derive(Clone, Debug, Default)]
/// 样本中的高频值列表。
pub struct TopN {
    pub values: Vec<(Datum, u64)>,
}
#[derive(Clone, Debug, Default)]
/// 一组列或索引的直方图/TopN/Sketch。
pub struct AnalyzeResult {
    pub histograms: Vec<Histogram>,
    pub topns: Vec<TopN>,
    pub sketches: Vec<FMSketch>,
    pub is_index: bool,
}
#[derive(Clone, Debug, Default)]
/// 列采样 ANALYZE 的最终输出。
pub struct AnalyzeResults {
    pub table_id: i64,
    pub results: Vec<AnalyzeResult>,
    pub count: i64,
    pub stats_version: i32,
    pub error: Option<AnalyzeError>,
}

/// 采样后端：读写原始样本、解码、虚拟列求值与索引编码。
pub trait AnalyzeSamplingBackend: Send + Sync + 'static {
    fn open_sampling(&self, ranges: &[Range]) -> Result<(), AnalyzeError>;
    fn next_raw(&self) -> Result<Option<Vec<u8>>, AnalyzeError>;
    fn close_sampling(&self) -> Result<(), AnalyzeError>;
    fn decode_collector(
        &self,
        data: &[u8],
        sample_size: usize,
        total_len: usize,
    ) -> Result<RowSampleCollector, AnalyzeError>;
    fn decode_column(&self, column: &ColumnInfo, datum: &Datum) -> Result<Datum, AnalyzeError>;
    fn evaluate_virtual_columns(
        &self,
        columns: &[ColumnInfo],
        row: &mut [Datum],
    ) -> Result<(), AnalyzeError>;
    fn build_handle(&self, row: &[Datum]) -> Result<i64, AnalyzeError>;
    fn analyze_index_ndv(&self, index: &IndexInfo) -> Result<(FMSketch, i64), AnalyzeError>;
    fn collate_key(&self, column: &ColumnInfo, value: &Datum) -> Result<Datum, AnalyzeError>;
    fn encode_index_value(&self, index: &IndexInfo, row: &[Datum]) -> Result<Datum, AnalyzeError>;
    fn killed(&self) -> Result<(), AnalyzeError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 采样扫描范围：有符号 handle / 非空约束。
pub struct Range {
    pub signed: bool,
    pub non_null: bool,
}

#[derive(Default)]
/// 原子计数的采样内存用量追踪。
pub struct MemoryTracker {
    bytes: AtomicI64,
}
impl MemoryTracker {
    /// 增加已用字节。
    pub fn consume(&self, bytes: i64) {
        self.bytes.fetch_add(bytes, Ordering::AcqRel);
    }
    /// 释放已用字节。
    pub fn release(&self, bytes: i64) {
        self.bytes.fetch_sub(bytes, Ordering::AcqRel);
    }
    /// 当前追踪字节数。
    pub fn bytes(&self) -> i64 {
        self.bytes.load(Ordering::Acquire)
    }
}

/// 基于行采样的列（及索引）统计构建执行器。
pub struct AnalyzeColumnsExec<B: AnalyzeSamplingBackend> {
    pub backend: Arc<B>,
    pub table_id: i64,
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
    pub sample_size: usize,
    pub sample_rate: f64,
    pub bucket_count: usize,
    pub topn_count: usize,
    pub samplingStatsConcurrency: usize,
    pub stats_version: i32,
    pub handle_unsigned: bool,
    pub memTracker: Arc<MemoryTracker>,
}

impl<B: AnalyzeSamplingBackend> AnalyzeColumnsExec<B> {
    /// 入口：处理特殊索引 NDV，再构建采样统计并拆成列/索引结果。
    pub fn analyzeColumnsPushDown(&self) -> AnalyzeResults {
        if self.samplingStatsConcurrency == 0 {
            return AnalyzeResults {
                table_id: self.table_id,
                error: Some(AnalyzeError::InvalidConfig(
                    "samplingStatsConcurrency must be positive".into(),
                )),
                ..Default::default()
            };
        }
        let ranges = vec![Range {
            signed: !self.handle_unsigned,
            non_null: true,
        }];
        let special_offsets = self
            .indexes
            .iter()
            .enumerate()
            .filter_map(|(index, info)| {
                info.columns
                    .iter()
                    .any(|column| {
                        column.prefix_length.is_some()
                            || self.columns[column.offset].virtual_generated
                    })
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        let ndv = match self.handleNDVForSpecialIndexes(&special_offsets) {
            Ok(ndv) => ndv,
            Err(error) => {
                return AnalyzeResults {
                    table_id: self.table_id,
                    error: Some(error),
                    ..Default::default()
                };
            }
        };
        match self.buildSamplingStats(&ranges, &special_offsets, ndv) {
            Ok((count, histograms, topns, sketches)) => {
                let column_len = self.columns.len();
                AnalyzeResults {
                    table_id: self.table_id,
                    count,
                    stats_version: self.stats_version,
                    results: vec![
                        AnalyzeResult {
                            histograms: histograms[..column_len].to_vec(),
                            topns: topns[..column_len].to_vec(),
                            sketches: sketches[..column_len].to_vec(),
                            is_index: false,
                        },
                        AnalyzeResult {
                            histograms: histograms[column_len..].to_vec(),
                            topns: topns[column_len..].to_vec(),
                            sketches: sketches[column_len..].to_vec(),
                            is_index: true,
                        },
                    ],
                    error: None,
                }
            }
            Err(error) => {
                let consumed = self.memTracker.bytes();
                self.memTracker.release(consumed);
                AnalyzeResults {
                    table_id: self.table_id,
                    error: Some(error),
                    ..Default::default()
                }
            }
        }
    }

    /// 解码样本列值并求值虚拟生成列。
    pub fn decodeSampleDataWithVirtualColumn(
        &self,
        collector: &mut RowSampleCollector,
    ) -> Result<(), AnalyzeError> {
        for sample in &mut collector.samples {
            for (index, datum) in sample.columns.iter_mut().enumerate() {
                if !self.columns[index].virtual_generated {
                    *datum = self.backend.decode_column(&self.columns[index], datum)?;
                }
            }
            self.backend
                .evaluate_virtual_columns(&self.columns, &mut sample.columns)?;
        }
        Ok(())
    }

    /// 生产者读原始样本，多 worker 合并收集器，再并行构建直方图/TopN。
    pub fn buildSamplingStats(
        &self,
        ranges: &[Range],
        special_offsets: &[usize],
        ndv_results: analyzeIndexNDVTotalResult,
    ) -> Result<(i64, Vec<Histogram>, Vec<TopN>, Vec<FMSketch>), AnalyzeError> {
        self.backend.open_sampling(ranges)?;
        let total_len = self.columns.len() + self.indexes.len();
        let (raw_tx, raw_rx) = mpsc::channel::<Vec<u8>>();
        let raw_rx = Arc::new(Mutex::new(raw_rx));
        let (merge_tx, merge_rx) = mpsc::channel::<samplingMergeResult>();
        let cancelled = Arc::new(AtomicBool::new(false));
        let tracker = Arc::clone(&self.memTracker);
        let backend = Arc::clone(&self.backend);
        let producer_cancel = Arc::clone(&cancelled);

        let result = thread::scope(|scope| -> Result<RowSampleCollector, AnalyzeError> {
            let producer = scope.spawn(move || {
                readDataAndSendTask(
                    backend.as_ref(),
                    raw_tx,
                    tracker.as_ref(),
                    producer_cancel.as_ref(),
                )
            });
            let mut merge_workers = Vec::new();
            for worker_index in 0..self.samplingStatsConcurrency {
                let receiver = Arc::clone(&raw_rx);
                let sender = merge_tx.clone();
                let cancelled = Arc::clone(&cancelled);
                merge_workers.push(scope.spawn(move || {
                    self.subMergeWorker(receiver, sender, total_len, worker_index, cancelled)
                }));
            }
            drop(merge_tx);
            let mut root = RowSampleCollector::new(self.sample_size, total_len);
            let mut first_error = None;
            for result in merge_rx {
                match result.error {
                    Some(error) => {
                        cancelled.store(true, Ordering::Release);
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                    None => {
                        if let Some(collector) = result.collector {
                            let old = root.mem_size;
                            let sub = collector.mem_size;
                            root.merge(collector);
                            self.memTracker.consume(root.mem_size - old - sub);
                        }
                    }
                }
            }
            let producer_result = producer.join().map_err(panic_error)?;
            for worker in merge_workers {
                let worker_result = worker.join().map_err(panic_error)?;
                if let Err(error) = worker_result {
                    if first_error.is_none() && error != AnalyzeError::Cancelled {
                        first_error = Some(error);
                    }
                }
            }
            drainPendingSamplingMergeTasks(
                &raw_rx.lock().expect("merge task receiver poisoned"),
                self.memTracker.as_ref(),
            );
            if let Some(error) = first_error {
                return Err(error);
            }
            producer_result?;
            Ok(root)
        });
        let close_result = self.backend.close_sampling();
        let mut root = result?;
        close_result?;
        self.decodeSampleDataWithVirtualColumn(&mut root)?;
        for sample in &mut root.samples {
            sample.handle = self.backend.build_handle(&sample.columns)?;
        }
        root.samples.sort_by_key(|sample| sample.handle);
        for (index, info) in self.indexes.iter().enumerate() {
            let position = self.columns.len() + index;
            for sample in &root.samples {
                let value = self.backend.encode_index_value(info, &sample.columns)?;
                if value == Datum::Null {
                    root.null_count[position] += 1;
                } else {
                    root.fm_sketches[position]
                        .hashes
                        .insert(stable_hash(&value.bytes()));
                    root.total_sizes[position] += value.bytes().len() as i64;
                }
            }
        }
        for offset in special_offsets {
            if let Some((sketch, null_count)) = ndv_results.results.get(&self.indexes[*offset].id) {
                root.fm_sketches[self.columns.len() + offset] = sketch.clone();
                root.null_count[self.columns.len() + offset] = *null_count;
            }
        }

        let root = Arc::new(root);
        let task_count = total_len;
        let concurrency = self.samplingStatsConcurrency.min(task_count).max(1);
        let (task_tx, task_rx) = mpsc::channel::<samplingBuildTask>();
        let task_rx = Arc::new(Mutex::new(task_rx));
        let (result_tx, result_rx) = mpsc::channel();
        thread::scope(|scope| {
            for _ in 0..concurrency {
                let receiver = Arc::clone(&task_rx);
                let sender = result_tx.clone();
                let collector = Arc::clone(&root);
                scope.spawn(move || self.subBuildWorker(receiver, sender, collector));
            }
            for (index, column) in self.columns.iter().enumerate() {
                task_tx
                    .send(samplingBuildTask {
                        id: column.id,
                        isColumn: true,
                        slicePos: index,
                    })
                    .map_err(|_| AnalyzeError::Cancelled)?;
            }
            for (index, info) in self.indexes.iter().enumerate() {
                task_tx
                    .send(samplingBuildTask {
                        id: info.id,
                        isColumn: false,
                        slicePos: self.columns.len() + index,
                    })
                    .map_err(|_| AnalyzeError::Cancelled)?;
            }
            drop(task_tx);
            drop(result_tx);
            Ok::<(), AnalyzeError>(())
        })?;
        let mut histograms = vec![Histogram::default(); total_len];
        let mut topns = vec![TopN::default(); total_len];
        for result in result_rx {
            let (position, histogram, topn) = result?;
            histograms[position] = histogram;
            topns[position] = topn;
        }
        let count = root.count;
        let sketches = root.fm_sketches.clone();
        self.memTracker.release(root.mem_size);
        Ok((count, histograms, topns, sketches))
    }

    /// 对前缀/虚拟列等特殊索引并行收集 NDV。
    pub fn handleNDVForSpecialIndexes(
        &self,
        offsets: &[usize],
    ) -> Result<analyzeIndexNDVTotalResult, AnalyzeError> {
        let (task_tx, task_rx) = mpsc::channel::<usize>();
        let task_rx = Arc::new(Mutex::new(task_rx));
        let (result_tx, result_rx) = mpsc::channel();
        let concurrency = self.samplingStatsConcurrency.min(offsets.len()).max(1);
        thread::scope(|scope| {
            for _ in 0..concurrency {
                let receiver = Arc::clone(&task_rx);
                let sender = result_tx.clone();
                scope.spawn(move || self.subIndexWorkerForNDV(receiver, sender));
            }
            for offset in offsets {
                task_tx.send(*offset).map_err(|_| AnalyzeError::Cancelled)?;
            }
            drop(task_tx);
            drop(result_tx);
            Ok::<(), AnalyzeError>(())
        })?;
        let mut total = analyzeIndexNDVTotalResult::default();
        for item in result_rx {
            let (id, sketch, nulls) = item?;
            total.results.insert(id, (sketch, nulls));
        }
        Ok(total)
    }

    /// 消费索引偏移任务并调用后端 `analyze_index_ndv`。
    pub fn subIndexWorkerForNDV(
        &self,
        tasks: Arc<Mutex<mpsc::Receiver<usize>>>,
        results: mpsc::Sender<Result<(i64, FMSketch, i64), AnalyzeError>>,
    ) -> Result<(), AnalyzeError> {
        loop {
            let task = tasks.lock().expect("NDV task receiver poisoned").recv();
            let Ok(offset) = task else { return Ok(()) };
            let index = &self.indexes[offset];
            let result = self
                .backend
                .analyze_index_ndv(index)
                .map(|(sketch, nulls)| (index.id, sketch, nulls));
            if results.send(result).is_err() {
                return Err(AnalyzeError::Cancelled);
            }
        }
    }

    /// 为特殊索引列表构造子任务。
    pub fn buildSubIndexJobForSpecialIndex(&self, indexes: &[IndexInfo]) -> Vec<analyzeTask> {
        indexes
            .iter()
            .cloned()
            .map(|index| analyzeTask { index })
            .collect()
    }

    /// 解码原始包并合并进局部 RowSampleCollector。
    pub fn subMergeWorker(
        &self,
        tasks: Arc<Mutex<mpsc::Receiver<Vec<u8>>>>,
        results: mpsc::Sender<samplingMergeResult>,
        total_len: usize,
        _index: usize,
        cancelled: Arc<AtomicBool>,
    ) -> Result<(), AnalyzeError> {
        let mut collector = RowSampleCollector::new(self.sample_size, total_len);
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Err(AnalyzeError::Cancelled);
            }
            let task = tasks.lock().expect("merge task receiver poisoned").recv();
            let Ok(data) = task else {
                results
                    .send(samplingMergeResult {
                        collector: Some(collector),
                        error: None,
                    })
                    .map_err(|_| AnalyzeError::Cancelled)?;
                return Ok(());
            };
            let data_size = data.capacity() as i64;
            let sub = match self
                .backend
                .decode_collector(&data, self.sample_size, total_len)
            {
                Ok(sub) => sub,
                Err(error) => {
                    self.memTracker.release(data_size);
                    let _ = results.send(samplingMergeResult {
                        collector: None,
                        error: Some(error.clone()),
                    });
                    return Err(error);
                }
            };
            let old = collector.mem_size;
            let sub_size = sub.mem_size;
            self.memTracker.consume(sub_size);
            collector.merge(sub);
            self.memTracker.consume(collector.mem_size - old - sub_size);
            self.memTracker.release(data_size);
        }
    }

    /// 按列/索引任务从根收集器构建直方图与 TopN。
    pub fn subBuildWorker(
        &self,
        tasks: Arc<Mutex<mpsc::Receiver<samplingBuildTask>>>,
        results: mpsc::Sender<Result<(usize, Histogram, TopN), AnalyzeError>>,
        root: Arc<RowSampleCollector>,
    ) -> Result<(), AnalyzeError> {
        loop {
            let task = tasks.lock().expect("build task receiver poisoned").recv();
            let Ok(task) = task else { return Ok(()) };
            let result = self
                .build_one_stat(&task, &root)
                .map(|(hist, topn)| (task.slicePos, hist, topn));
            if results.send(result).is_err() {
                return Err(AnalyzeError::Cancelled);
            }
        }
    }

    /// 从样本抽取一列或一索引的值，调用 `build_hist_topn`。
    fn build_one_stat(
        &self,
        task: &samplingBuildTask,
        root: &RowSampleCollector,
    ) -> Result<(Histogram, TopN), AnalyzeError> {
        if task.isColumn
            && self.columns[task.slicePos].virtual_generated
            && !self.columns[task.slicePos].generated_stored
        {
            return Ok((Histogram::default(), TopN::default()));
        }
        let mut values = Vec::new();
        if task.isColumn {
            let column = &self.columns[task.slicePos];
            for (ordinal, sample) in root.samples.iter().enumerate() {
                let value = sample.columns[task.slicePos].clone();
                if value == Datum::Null || value.bytes().len() > MAX_SAMPLE_VALUE_LENGTH {
                    continue;
                }
                let value = if column.string_type {
                    self.backend.collate_key(column, &value)?
                } else {
                    value
                };
                values.push((value, ordinal));
            }
        } else {
            let index = &self.indexes[task.slicePos - self.columns.len()];
            'samples: for sample in &root.samples {
                // Go filters oversized index values per source column before
                // encoding. The encoded composite key itself may legitimately
                // exceed MAX_SAMPLE_VALUE_LENGTH when every component does not.
                for index_column in &index.columns {
                    if sample.columns[index_column.offset].bytes().len() > MAX_SAMPLE_VALUE_LENGTH {
                        continue 'samples;
                    }
                }
                let value = self.backend.encode_index_value(index, &sample.columns)?;
                if value != Datum::Null {
                    values.push((value, 0));
                }
            }
        }
        let num_topn = if task.isColumn && self.columns[task.slicePos].unique_single_column
            || !task.isColumn
                && self.indexes[task.slicePos - self.columns.len()].unique
                && self.indexes[task.slicePos - self.columns.len()]
                    .columns
                    .len()
                    == 1
                && self.indexes[task.slicePos - self.columns.len()].columns[0]
                    .prefix_length
                    .is_none()
        {
            0
        } else {
            self.topn_count
        };
        Ok(build_hist_topn(
            task.id,
            values,
            root.null_count[task.slicePos],
            root.fm_sketches[task.slicePos].ndv(),
            self.bucket_count,
            num_topn,
        ))
    }
}

#[derive(Clone, Debug)]
/// 特殊索引 NDV 子任务。
pub struct analyzeTask {
    pub index: IndexInfo,
}

#[derive(Clone, Debug, Default)]
/// 特殊索引 NDV 汇总：索引 ID → (FMSketch, null 计数)。
pub struct analyzeIndexNDVTotalResult {
    pub results: BTreeMap<i64, (FMSketch, i64)>,
    pub error: Option<AnalyzeError>,
}
#[derive(Clone, Debug)]
/// 合并 worker 输出：收集器或错误。
pub struct samplingMergeResult {
    pub collector: Option<RowSampleCollector>,
    pub error: Option<AnalyzeError>,
}
#[derive(Clone, Debug)]
/// 构建 worker 任务：目标 ID、是否列、在切片中的位置。
pub struct samplingBuildTask {
    pub id: i64,
    pub isColumn: bool,
    pub slicePos: usize,
}

/// 格式化合并收集器过程的诊断日志行。
pub fn printAnalyzeMergeCollectorLog(
    old_root_count: i64,
    new_root_count: i64,
    sub_count: i64,
    table_id: i64,
    partition_id: i64,
    is_partition: bool,
    info: &str,
    index: i32,
) -> String {
    format!(
        "{info}: tableID={table_id}, partitionID={partition_id}, isPartition={is_partition}, oldRootCount={old_root_count}, newRootCount={new_root_count}, subCount={sub_count}, subCollectorIndex={index}"
    )
}

/// 排空未处理原始包并释放其占用的内存计数。
pub fn drainPendingSamplingMergeTasks(receiver: &mpsc::Receiver<Vec<u8>>, tracker: &MemoryTracker) {
    while let Ok(data) = receiver.try_recv() {
        tracker.release(data.capacity() as i64);
    }
}

/// 生产者循环：读取原始样本包并送入合并队列。
pub fn readDataAndSendTask<B: AnalyzeSamplingBackend>(
    backend: &B,
    sender: mpsc::Sender<Vec<u8>>,
    tracker: &MemoryTracker,
    cancelled: &AtomicBool,
) -> Result<(), AnalyzeError> {
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(AnalyzeError::Cancelled);
        }
        backend.killed()?;
        let Some(data) = backend.next_raw()? else {
            return Ok(());
        };
        tracker.consume(data.capacity() as i64);
        if sender.send(data).is_err() {
            return Err(AnalyzeError::Cancelled);
        }
    }
}

// 先抽 TopN，剩余值按桶均分构建直方图。
/// 由样本值频率表构建直方图与 TopN。
fn build_hist_topn(
    id: i64,
    values: Vec<(Datum, usize)>,
    null_count: i64,
    ndv: u64,
    bucket_count: usize,
    topn_count: usize,
) -> (Histogram, TopN) {
    let mut frequencies = BTreeMap::<Datum, u64>::new();
    for (value, _) in values {
        *frequencies.entry(value).or_default() += 1;
    }
    let mut top_values = frequencies
        .iter()
        .map(|(value, count)| (value.clone(), *count))
        .collect::<Vec<_>>();
    top_values.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    top_values.truncate(topn_count);
    let excluded = top_values
        .iter()
        .map(|(value, _)| value.clone())
        .collect::<BTreeSet<_>>();
    let remaining = frequencies
        .into_iter()
        .filter(|(value, _)| !excluded.contains(value))
        .collect::<Vec<_>>();
    let per_bucket = remaining.len().div_ceil(bucket_count.max(1)).max(1);
    let mut buckets = Vec::new();
    for group in remaining.chunks(per_bucket) {
        if let (Some(first), Some(last)) = (group.first(), group.last()) {
            buckets.push(HistogramBucket {
                lower: first.0.clone(),
                upper: last.0.clone(),
                count: group.iter().map(|(_, count)| count).sum(),
            });
        }
    }
    (
        Histogram {
            id,
            ndv,
            null_count,
            buckets,
        },
        TopN { values: top_values },
    )
}

/// 估算样本集合占用的内存字节数。
fn collector_memory(samples: &[ReservoirRowSampleItem]) -> i64 {
    samples
        .iter()
        .map(|sample| {
            sample
                .columns
                .iter()
                .map(|value| value.bytes().capacity() as i64)
                .sum::<i64>()
                + std::mem::size_of::<ReservoirRowSampleItem>() as i64
        })
        .sum()
}
/// FNV-1a 风格稳定哈希，供 FM Sketch 使用。
fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
/// 将 worker panic 载荷转为 `WorkerPanic` 错误。
fn panic_error(payload: Box<dyn std::any::Any + Send>) -> AnalyzeError {
    if let Some(message) = payload.downcast_ref::<String>() {
        AnalyzeError::WorkerPanic(message.clone())
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        AnalyzeError::WorkerPanic((*message).into())
    } else {
        AnalyzeError::WorkerPanic("unknown worker panic".into())
    }
}
