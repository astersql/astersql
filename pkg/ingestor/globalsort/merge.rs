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

use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::reader::CancellationToken;
use crate::util::summary_for_file;
use crate::{
    ConflictInfo, Error, KvPair, OnDuplicateKey, Result, Storage, WriterSummary, decode_kvs,
    encode_kvs,
};

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
    conflict_info: Mutex<ConflictInfo>,
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
        conflict_info: Mutex::new(ConflictInfo::default()),
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

/// 将重叠文件按并发度分组后归并，返回各组写出的 data 文件路径。
pub fn MergeOverlappingFiles(paths: &[String], op: &MergeOperator) -> Result<Vec<String>> {
    let groups = splitDataFiles(paths, op.concurrency);
    let mut outputs = Vec::with_capacity(groups.len());
    for group in groups {
        let id = op.next_writer_id.fetch_add(1, Ordering::Relaxed);
        outputs.push(merge_overlapping_files_internal(
            &op.token,
            &group,
            op.store.as_ref(),
            op.part_size,
            &op.new_file_prefix,
            &id.to_string(),
            op.block_size,
            op.on_writer_close.as_ref(),
            op.collector.as_deref(),
            op.check_hotspot,
            op.on_duplicate,
            groups_len(paths, op.concurrency),
            &op.conflict_info,
        )?);
    }
    Ok(outputs)
}

/// 计算按当前并发度切分后的分组数。
fn groups_len(paths: &[String], concurrency: usize) -> usize {
    splitDataFiles(paths, concurrency).len()
}

/// 将文件路径列表切成约 `concurrency` 份，并受 `MaxMergingFilesPerThread` 约束。
pub fn splitDataFiles(paths: &[String], concurrency: usize) -> Vec<Vec<String>> {
    if paths.is_empty() {
        return Vec::new();
    }
    let concurrency = concurrency.max(1);
    let maximum = MaxMergingFilesPerThread.load(Ordering::Relaxed).max(1);
    // 先按每线程文件上限估算份额，再与并发度取较大者。
    let mut shares = paths.len().div_ceil(maximum).max(concurrency);
    // 文件很少时避免过度切分：份额约为路径数的一半。
    if paths.len() < 2 * concurrency {
        shares = (paths.len() / 2).max(1);
    }
    shares = shares.min(paths.len());
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
    let mut kvs = Vec::new();
    for path in paths {
        if token.is_cancelled() {
            return Err(Error::Cancelled);
        }
        kvs.extend(decode_kvs(&store.read(path)?, 0)?);
    }
    kvs.sort_by(|left, right| left.key.cmp(&right.key));
    let mut recorded_duplicates = Vec::new();
    let kvs = handle_duplicates(
        kvs,
        on_duplicate,
        conflicts,
        &mut recorded_duplicates,
        collector,
    )?;
    let prefix = new_file_prefix.trim_end_matches('/');
    let data_file = format!("{prefix}/{writer_id}.data");
    let stat_file = format!("{prefix}/{writer_id}.stat");
    store.write(&data_file, encode_kvs(&kvs))?;
    store.write(&stat_file, Vec::new())?;
    if !recorded_duplicates.is_empty() {
        let duplicate_file = format!("{prefix}/{writer_id}.dup");
        store.write(&duplicate_file, encode_kvs(&recorded_duplicates))?;
        conflicts
            .lock()
            .map_err(|_| Error::Poisoned)?
            .files
            .push(duplicate_file);
    }
    let mut summary = summary_for_file(data_file.clone(), stat_file, &kvs);
    summary.conflict_info = conflicts.lock().map_err(|_| Error::Poisoned)?.clone();
    if let Some(callback) = on_writer_close {
        callback(&summary);
    }
    Ok(data_file)
}

/// 按 `on_duplicate` 处理已按 key 排序的 KV 流中的相邻重复项。
fn handle_duplicates(
    kvs: Vec<KvPair>,
    on_duplicate: OnDuplicateKey,
    conflicts: &Mutex<ConflictInfo>,
    recorded_duplicates: &mut Vec<KvPair>,
    collector: Option<&dyn Collector>,
) -> Result<Vec<KvPair>> {
    let mut result = Vec::with_capacity(kvs.len());
    let mut run_start = 0;
    while run_start < kvs.len() {
        let mut run_end = run_start + 1;
        while run_end < kvs.len() && kvs[run_end].key == kvs[run_start].key {
            run_end += 1;
        }
        let run = &kvs[run_start..run_end];
        if run.len() == 1 {
            collect_processed(collector, &run[0]);
            result.push(run[0].clone());
            run_start = run_end;
            continue;
        }
        match on_duplicate {
            // Go OneFileWriter bypasses duplicate handling entirely for Ignore.
            OnDuplicateKey::Ignore => {
                collect_all_processed(collector, run);
                result.extend_from_slice(run);
            }
            // Remove omits the complete duplicate group, including its first row.
            OnDuplicateKey::Remove => collect_all_processed(collector, run),
            // Record keeps the first two rows in the data file and records only
            // the third and later rows in its duplicate stream.
            OnDuplicateKey::Record => {
                collect_all_processed(collector, run);
                result.extend_from_slice(&run[..run.len().min(2)]);
                let mut info = conflicts.lock().map_err(|_| Error::Poisoned)?;
                for pair in &run[2..] {
                    info.count = info.count.saturating_add(1);
                    recorded_duplicates.push(pair.clone());
                }
            }
            OnDuplicateKey::Error => {
                // The first row is buffered successfully; the second WriteRow
                // detects the duplicate and returns before mergeCollector.Processed.
                collect_processed(collector, &run[0]);
                let pair = &run[1];
                return Err(Error::DuplicateKey {
                    key: pair.key.clone(),
                    value: pair.value.clone(),
                });
            }
        }
        run_start = run_end;
    }
    Ok(result)
}

fn collect_processed(collector: Option<&dyn Collector>, pair: &KvPair) {
    if let Some(collector) = collector {
        collector.Processed(pair.encoded_size() as i64, 1);
    }
}

fn collect_all_processed(collector: Option<&dyn Collector>, pairs: &[KvPair]) {
    for pair in pairs {
        collect_processed(collector, pair);
    }
}
