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

// DDL 回填（backfill）算子模块。
//
// “回填”指在执行 `ADD INDEX` 等在线 DDL 时，为表中已存在的历史数据补建索引条目。
// 本模块把回填流程拆分为一组类似流水线（pipeline）的算子（operator）：
// - `TableScanTaskSource`：根据 Region（数据分片的键区间）划分表扫描任务；
// - `TableScanWorker`：扫描表记录并按批（chunk）产出待写入的索引记录；
// - `IndexIngestWorker`：把索引记录写入底层 `IndexWriter`（例如本地导入引擎）；
// - `IndexWriteResultSink`：汇总写入结果、触发配额检查与最终 flush；
// - `MergeTemporaryIndexWorker`：将临时索引（增量 DML 写入的暂存索引）合并回正式索引。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use crate::backfilling::{Key, KeyRange};
use crate::backfilling_txn_executor::TaskIdAllocator;

/// 回填算子执行过程中可能出现的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperatorError {
    /// 任务被取消。
    Cancelled,
    /// 检查点（checkpoint，用于断点续传的进度键）不在 [start, end] 区间内。
    InvalidCheckpoint {
        checkpoint: Key,
        start: Key,
        end: Key,
    },
    /// 扫描区间非法（起始键不小于结束键，或批大小为 0）。
    InvalidRange,
    /// 写入索引数据失败。
    Write(String),
    /// 刷盘（flush）失败。
    Flush(String),
    /// 可重试错误重试次数耗尽。
    RetryExhausted,
}

impl std::fmt::Display for OperatorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("context canceled"),
            Self::InvalidCheckpoint {
                checkpoint,
                start,
                end,
            } => write!(
                formatter,
                "invalid checkpoint {checkpoint:02x?} outside [{start:02x?}, {end:02x?}]"
            ),
            Self::InvalidRange => formatter.write_str("invalid scan range"),
            Self::Write(error) | Self::Flush(error) => formatter.write_str(error),
            Self::RetryExhausted => formatter.write_str("retry attempts exhausted"),
        }
    }
}

impl std::error::Error for OperatorError {}

/// 一个表扫描任务，对应一段左闭右开的键区间 [start, end)。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableScanTask {
    /// 任务编号，由 `TaskIdAllocator` 分配。
    pub id: usize,
    /// 区间起始键（包含）。
    pub start: Key,
    /// 区间结束键（不包含）。
    pub end: Key,
}

impl std::fmt::Display for TableScanTask {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "TableScanTask: id={}, startKey={:02x?}, endKey={:02x?}",
            self.id, self.start, self.end
        )
    }
}

/// 一条待写入索引的记录：由表行数据编码出的索引键值对。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexRecord {
    /// 原始行的记录键（row key）。
    pub row_key: Key,
    /// 编码后的索引键。
    pub index_key: Key,
    /// 索引值。
    pub value: Vec<u8>,
    /// 该行是否满足部分索引（partial index，带 WHERE 条件的索引）的过滤条件。
    pub matches_partial_index: bool,
}

/// 一批扫描出的索引记录，是表扫描算子与写入算子之间传递的单元。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexRecordChunk {
    /// 所属扫描任务编号。
    pub task_id: usize,
    /// 本批记录。
    pub records: Vec<IndexRecord>,
    /// 是否为该任务的最后一批。
    pub done: bool,
    /// 本批实际扫描到的表行数（过滤前）。
    pub table_scan_row_count: i64,
    /// 部分索引条件是否已在扫描阶段下推过滤。
    pub condition_pushed: bool,
    /// 扫描阶段产生的错误（若有则整批失败）。
    pub error: Option<OperatorError>,
}

/// 表扫描任务的来源：负责根据 Region 区间与检查点切分出具体的扫描任务。
///
/// Region 是分布式存储中按键区间划分的数据分片；回填按 Region 边界切分任务
/// 以便并行处理并利用数据本地性。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableScanTaskSource {
    /// 物理表 ID（分区表时为具体分区的 ID）。
    pub physical_table_id: i64,
    /// 整个回填范围的起始键。
    pub start_key: Key,
    /// 整个回填范围的结束键。
    pub end_key: Key,
    /// 表记录键的公共前缀（用于把空区间补全为整表范围）。
    pub record_prefix: Key,
    /// 断点续传的检查点键：若存在则从该键继续扫描。
    pub checkpoint_key: Option<Key>,
}

impl TableScanTaskSource {
    /// 根据检查点调整扫描起始键。
    ///
    /// 返回 `(调整后的起始键, 是否已全部完成)`；若检查点落在 [start, end] 之外，
    /// 与 Go 实现一致忽略该检查点并从原始起点继续。
    pub fn adjust_start_key(&self, start: Key, end: &[u8]) -> Result<(Key, bool), OperatorError> {
        // 没有检查点：从原始起点开始。
        let Some(checkpoint) = self.checkpoint_key.as_ref() else {
            return Ok((start, false));
        };
        if checkpoint.as_slice() < start.as_slice() || checkpoint.as_slice() > end {
            return Ok((start, false));
        }
        // 检查点恰好等于结束键时表示扫描已经全部完成。
        Ok((checkpoint.clone(), checkpoint.as_slice() == end))
    }

    /// 把一组 Region 键区间与回填范围求交集，生成表扫描任务列表。
    ///
    /// 空的区间边界会被补全为整表的记录键范围；与回填范围无交集的区间被跳过。
    pub fn generate_tasks(
        &self,
        ranges: &[KeyRange],
        allocator: &mut TaskIdAllocator,
    ) -> Result<Vec<TableScanTask>, OperatorError> {
        let (adjusted_start, done) =
            self.adjust_start_key(self.start_key.clone(), &self.end_key)?;
        if done {
            return Ok(Vec::new());
        }
        let prefix_end = prefix_next(&self.record_prefix);
        ranges
            .iter()
            .filter_map(|range| {
                // 空边界表示无界，用表记录前缀补全为整表范围。
                let mut start = if range.start_key.is_empty() {
                    self.record_prefix.clone()
                } else {
                    range.start_key.clone()
                };
                let end = if range.end_key.is_empty() {
                    prefix_end.clone()
                } else {
                    range.end_key.clone()
                };
                // 与 [adjusted_start, end_key) 无交集的 Region 直接跳过。
                if end <= adjusted_start || start >= self.end_key {
                    return None;
                }
                // 把起点收敛到检查点之后，避免重复回填。
                if start < adjusted_start {
                    start = adjusted_start.clone();
                }
                Some(TableScanTask {
                    id: allocator.alloc(),
                    start,
                    end: end.min(self.end_key.clone()),
                })
            })
            .map(|task| {
                // 求交集后区间必须仍然有效（起点严格小于终点）。
                if task.start >= task.end {
                    Err(OperatorError::InvalidRange)
                } else {
                    Ok(task)
                }
            })
            .collect()
    }
}

/// 表扫描工作者：按任务区间扫描记录并切分为批（chunk）输出。
#[derive(Clone)]
pub struct TableScanWorker {
    /// 默认每批记录数上限。
    pub chunk_capacity: usize,
    /// 是否将部分索引条件下推到扫描阶段过滤。
    pub condition_pushed: bool,
    /// 可动态调整的 reorg 批大小（对应系统变量 tidb_ddl_reorg_batch_size），
    /// 存在时优先于 `chunk_capacity`。
    pub reorg_batch_size: Option<Arc<AtomicUsize>>,
}

impl TableScanWorker {
    /// 扫描 `rows` 中落在任务区间内的记录，按批大小切分为多个 chunk。
    ///
    /// 最后一个 chunk 的 `done` 为 true；若无匹配记录则返回单个空的 done chunk。
    pub fn scan_records(
        &self,
        task: &TableScanTask,
        rows: &[IndexRecord],
    ) -> Result<Vec<IndexRecordChunk>, OperatorError> {
        // 优先读取动态批大小（Acquire 保证读到其他线程最新写入的值）。
        let chunk_capacity = self
            .reorg_batch_size
            .as_ref()
            .map_or(self.chunk_capacity, |batch_size| {
                batch_size.load(Ordering::Acquire)
            });
        if task.start >= task.end || chunk_capacity == 0 {
            return Err(OperatorError::InvalidRange);
        }
        // 选出行键落在 [start, end) 内的记录。
        let selected: Vec<IndexRecord> = rows
            .iter()
            .filter(|record| record.row_key >= task.start && record.row_key < task.end)
            .cloned()
            .collect();
        // 区间内无记录：仍需输出一个空的完成批，让下游知道该任务已结束。
        if selected.is_empty() {
            return Ok(vec![IndexRecordChunk {
                task_id: task.id,
                done: true,
                condition_pushed: self.condition_pushed,
                ..IndexRecordChunk::default()
            }]);
        }
        let total_chunks = selected.len().div_ceil(chunk_capacity);
        Ok(selected
            .chunks(chunk_capacity)
            .enumerate()
            .map(|(index, records)| {
                let scanned = records.len() as i64;
                // 条件已下推时在扫描阶段就过滤掉不满足部分索引条件的行。
                let records = if self.condition_pushed {
                    records
                        .iter()
                        .filter(|record| record.matches_partial_index)
                        .cloned()
                        .collect()
                } else {
                    records.to_vec()
                };
                IndexRecordChunk {
                    task_id: task.id,
                    records,
                    done: index + 1 == total_chunks,
                    table_scan_row_count: scanned,
                    condition_pushed: self.condition_pushed,
                    error: None,
                }
            })
            .collect())
    }
}

/// 索引写入器抽象：屏蔽底层存储（如本地导入引擎或事务写入）的差异。
pub trait IndexWriter {
    /// 写入一条索引键值对，返回写入的字节数。
    fn write(&mut self, key: &[u8], value: &[u8]) -> Result<usize, String>;
    /// 将缓冲的数据刷入持久化存储。
    fn flush(&mut self) -> Result<(), String>;
    /// 当本地缓存超过磁盘配额时触发 ingest（把已排序数据导入存储引擎）。
    fn ingest_if_quota_exceeded(&mut self, task_id: usize, row_count: i64) -> Result<(), String>;
    /// 已写入的索引键总数。
    fn total_key_count(&self) -> i64;
}

/// 单批索引写入的结果统计。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexWriteResult {
    /// 所属扫描任务编号。
    pub task_id: usize,
    /// 实际写入的行数。
    pub row_count: i64,
    /// 写入的字节数。
    pub written_bytes: usize,
    /// 是否为该任务的最后一批。
    pub done: bool,
}

/// 索引摄入（ingest）工作者：把扫描出的索引记录写入 `IndexWriter`。
pub struct IndexIngestWorker<W> {
    /// 底层索引写入器。
    pub writer: W,
    /// 本次 DDL 同时创建的索引个数。
    pub index_count: usize,
}

impl<W: IndexWriter> IndexIngestWorker<W> {
    /// 把一批索引记录写入底层写入器，返回写入统计。
    pub fn write_chunk(
        &mut self,
        chunk: &IndexRecordChunk,
    ) -> Result<IndexWriteResult, OperatorError> {
        if let Some(error) = &chunk.error {
            return Err(error.clone());
        }
        // 只有单索引且条件已在扫描端下推时，才能跳过部分索引条件复查；
        // 多索引时各索引条件不同，仍需逐行检查。
        let skip_checker = chunk.condition_pushed && self.index_count == 1;
        let mut bytes = 0_usize;
        for record in &chunk.records {
            // 不满足部分索引条件的行直接跳过，不写入索引。
            if !skip_checker && !record.matches_partial_index {
                continue;
            }
            bytes += self
                .writer
                .write(&record.index_key, &record.value)
                .map_err(OperatorError::Write)?;
        }
        Ok(IndexWriteResult {
            task_id: chunk.task_id,
            // Go forwards tableScanRowCount to quota/progress accounting even
            // when partial-index filtering writes fewer index entries.
            row_count: chunk.table_scan_row_count,
            written_bytes: bytes,
            done: chunk.done,
        })
    }
}

/// 写入结果汇聚端（sink）：流水线的终点，累计行数、触发配额检查并做最终 flush。
pub struct IndexWriteResultSink<W> {
    /// 可选的底层写入器；为 None 时（如分布式执行路径）仅做行数统计。
    pub writer: Option<W>,
    /// 已处理（写入）的行数累计。
    pub processed_rows: i64,
    /// 写入器报告的索引键总数（用于最终校验行数一致性）。
    pub total_rows: i64,
}

impl<W: IndexWriter> IndexWriteResultSink<W> {
    /// 汇总所有写入结果：逐批检查磁盘配额，最后 flush 并更新总行数。
    pub fn collect_results(&mut self, results: &[IndexWriteResult]) -> Result<(), OperatorError> {
        for result in results {
            self.processed_rows += result.row_count;
            if let Some(writer) = self.writer.as_mut() {
                writer
                    .ingest_if_quota_exceeded(result.task_id, result.row_count)
                    .map_err(OperatorError::Write)?;
            }
        }
        self.flush()?;
        // flush 后从写入器读取权威的键总数，覆盖本地累计值。
        if let Some(writer) = self.writer.as_ref() {
            let total = writer.total_key_count();
            if total > 0 {
                self.total_rows = total;
            }
        }
        Ok(())
    }

    /// 触发底层写入器刷盘；无写入器时为空操作。
    pub fn flush(&mut self) -> Result<(), OperatorError> {
        match self.writer.as_mut() {
            Some(writer) => writer.flush().map_err(OperatorError::Flush),
            None => Ok(()),
        }
    }
}

/// 串联执行 ADD INDEX 回填流水线：切分任务 -> 扫描记录 -> 写入索引。
///
/// 这是各算子的同步组合版本，真实系统中各阶段通常由并发算子异步衔接。
pub fn run_add_index_pipeline<W: IndexWriter>(
    source: &TableScanTaskSource,
    ranges: &[KeyRange],
    rows: &[IndexRecord],
    scanner: &TableScanWorker,
    ingest: &mut IndexIngestWorker<W>,
) -> Result<Vec<IndexWriteResult>, OperatorError> {
    let mut allocator = TaskIdAllocator::new();
    let tasks = source.generate_tasks(ranges, &mut allocator)?;
    let mut results = Vec::new();
    for task in tasks {
        for chunk in scanner.scan_records(&task, rows)? {
            results.push(ingest.write_chunk(&chunk)?);
        }
    }
    Ok(results)
}

/// Go-compatible failpoint paths reached by the asynchronous ADD INDEX pipeline.
pub const MOCK_SCAN_RECORD_ERROR: &str = "github.com/pingcap/tidb/pkg/ddl/mockScanRecordError";
pub const SCAN_RECORD_EXEC: &str = "github.com/pingcap/tidb/pkg/ddl/scanRecordExec";
pub const MOCK_WRITE_LOCAL_ERROR: &str = "github.com/pingcap/tidb/pkg/ddl/mockWriteLocalError";
pub const WRITE_LOCAL_EXEC: &str = "github.com/pingcap/tidb/pkg/ddl/writeLocalExec";
pub const MOCK_FLUSH_ERROR: &str = "github.com/pingcap/tidb/pkg/ddl/mockFlushError";

/// The set of fault-injection boundaries enabled for one pipeline execution.
///
/// The configuration belongs to the pipeline rather than global process state,
/// so concurrently running DDL jobs cannot consume one another's one-shot
/// injections. Every supported path is evaluated inside the corresponding
/// production scan/write/flush boundary.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AddIndexPipelineFailpoints {
    enabled: BTreeSet<&'static str>,
}

impl AddIndexPipelineFailpoints {
    /// Enable one of the five Go ADD INDEX operator failpoints.
    pub fn enable(&mut self, path: &str) -> Result<(), String> {
        let canonical = match path {
            MOCK_SCAN_RECORD_ERROR => MOCK_SCAN_RECORD_ERROR,
            SCAN_RECORD_EXEC => SCAN_RECORD_EXEC,
            MOCK_WRITE_LOCAL_ERROR => MOCK_WRITE_LOCAL_ERROR,
            WRITE_LOCAL_EXEC => WRITE_LOCAL_EXEC,
            MOCK_FLUSH_ERROR => MOCK_FLUSH_ERROR,
            _ => return Err(format!("unknown ADD INDEX pipeline failpoint {path}")),
        };
        self.enabled.insert(canonical);
        Ok(())
    }

    fn contains(&self, path: &'static str) -> bool {
        self.enabled.contains(path)
    }
}

/// Cancellation and first-error state shared by every stage of one pipeline.
#[derive(Clone, Default)]
pub struct BackfillOperatorContext {
    inner: Arc<BackfillOperatorContextInner>,
}

#[derive(Default)]
struct BackfillOperatorContextInner {
    cancelled: AtomicBool,
    operator_error: Mutex<Option<OperatorError>>,
}

impl BackfillOperatorContext {
    /// Construct an independent ADD INDEX operator context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancel the pipeline without recording an operator failure.
    #[allow(non_snake_case)]
    pub fn Cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Release);
    }

    /// Whether cancellation has been broadcast to the pipeline.
    #[allow(non_snake_case)]
    pub fn IsCancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    /// Return the first scan/write/flush failure.
    #[allow(non_snake_case)]
    pub fn OperatorErr(&self) -> Option<OperatorError> {
        self.inner
            .operator_error
            .lock()
            .expect("backfill operator error mutex")
            .clone()
    }

    fn on_error(&self, error: OperatorError) {
        let mut first = self
            .inner
            .operator_error
            .lock()
            .expect("backfill operator error mutex");
        if first.is_none() {
            *first = Some(error);
        }
        drop(first);
        self.Cancel();
    }
}

/// Construct the cancellable context used by local ADD INDEX workers.
#[allow(non_snake_case)]
pub fn NewLocalWorkerCtx() -> BackfillOperatorContext {
    BackfillOperatorContext::new()
}

/// Asynchronous table-scan stage backed by the production [`TableScanWorker`].
pub struct TableScanOperator {
    context: BackfillOperatorContext,
    scanner: Arc<TableScanWorker>,
    rows: Arc<Vec<IndexRecord>>,
    worker_count: usize,
    opened: bool,
}

impl TableScanOperator {
    /// Construct a table-scan operator with the requested concurrency.
    #[allow(non_snake_case)]
    pub fn New(
        context: BackfillOperatorContext,
        scanner: TableScanWorker,
        rows: Vec<IndexRecord>,
        worker_count: usize,
    ) -> Self {
        Self {
            context,
            scanner: Arc::new(scanner),
            rows: Arc::new(rows),
            worker_count: worker_count.max(1),
            opened: false,
        }
    }

    /// Open the stage before pipeline execution.
    #[allow(non_snake_case)]
    pub fn Open(&mut self) -> Result<(), OperatorError> {
        if self.opened {
            return Err(OperatorError::Write(
                "table scan operator is already open".to_owned(),
            ));
        }
        self.opened = true;
        Ok(())
    }

    /// Close the stage after all workers have joined.
    #[allow(non_snake_case)]
    pub fn Close(&mut self) -> Result<(), OperatorError> {
        self.opened = false;
        Ok(())
    }

    /// Dynamically configure the number of scan workers used by the next run.
    #[allow(non_snake_case)]
    pub fn TuneWorkerPoolSize(&mut self, worker_count: i32, _wait: bool) {
        self.worker_count = worker_count.max(1) as usize;
    }

    /// Return the configured scan-worker count.
    #[allow(non_snake_case)]
    pub fn GetWorkerPoolSize(&self) -> i32 {
        self.worker_count as i32
    }
}

/// A writer handle shared by every ingest worker and the result sink.
struct LockedIndexWriter<W> {
    inner: Arc<Mutex<W>>,
}

impl<W> Clone for LockedIndexWriter<W> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<W: IndexWriter> IndexWriter for LockedIndexWriter<W> {
    fn write(&mut self, key: &[u8], value: &[u8]) -> Result<usize, String> {
        self.inner
            .lock()
            .expect("index writer mutex")
            .write(key, value)
    }

    fn flush(&mut self) -> Result<(), String> {
        self.inner.lock().expect("index writer mutex").flush()
    }

    fn ingest_if_quota_exceeded(&mut self, task_id: usize, row_count: i64) -> Result<(), String> {
        self.inner
            .lock()
            .expect("index writer mutex")
            .ingest_if_quota_exceeded(task_id, row_count)
    }

    fn total_key_count(&self) -> i64 {
        self.inner
            .lock()
            .expect("index writer mutex")
            .total_key_count()
    }
}

/// Asynchronous index-ingest stage backed by [`IndexIngestWorker`].
pub struct IndexIngestOperator<W> {
    context: BackfillOperatorContext,
    writer: Arc<Mutex<W>>,
    index_count: usize,
    worker_count: usize,
    opened: bool,
}

impl<W> IndexIngestOperator<W> {
    /// Construct an index-ingest operator with a shared writer.
    #[allow(non_snake_case)]
    pub fn New(
        context: BackfillOperatorContext,
        writer: W,
        index_count: usize,
        worker_count: usize,
    ) -> Self {
        Self {
            context,
            writer: Arc::new(Mutex::new(writer)),
            index_count,
            worker_count: worker_count.max(1),
            opened: false,
        }
    }

    /// Open the stage before pipeline execution.
    #[allow(non_snake_case)]
    pub fn Open(&mut self) -> Result<(), OperatorError> {
        if self.opened {
            return Err(OperatorError::Write(
                "index ingest operator is already open".to_owned(),
            ));
        }
        self.opened = true;
        Ok(())
    }

    /// Close the stage after all workers have joined.
    #[allow(non_snake_case)]
    pub fn Close(&mut self) -> Result<(), OperatorError> {
        self.opened = false;
        Ok(())
    }

    /// Dynamically configure the number of ingest workers used by the next run.
    #[allow(non_snake_case)]
    pub fn TuneWorkerPoolSize(&mut self, worker_count: i32, _wait: bool) {
        self.worker_count = worker_count.max(1) as usize;
    }

    /// Return the configured ingest-worker count.
    #[allow(non_snake_case)]
    pub fn GetWorkerPoolSize(&self) -> i32 {
        self.worker_count as i32
    }
}

/// Final row-count and write statistics produced by an ADD INDEX pipeline.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AddIndexPipelineSummary {
    pub task_count: usize,
    pub chunk_count: usize,
    pub processed_rows: i64,
    pub total_rows: i64,
    pub written_bytes: usize,
}

/// A multi-producer/multi-consumer queue with explicit close semantics.
struct StageQueue<T> {
    state: Mutex<StageQueueState<T>>,
    changed: Condvar,
}

struct StageQueueState<T> {
    values: VecDeque<T>,
    closed: bool,
}

impl<T> StageQueue<T> {
    fn new() -> Self {
        Self {
            state: Mutex::new(StageQueueState {
                values: VecDeque::new(),
                closed: false,
            }),
            changed: Condvar::new(),
        }
    }

    fn send(&self, value: T) -> bool {
        let mut state = self.state.lock().expect("pipeline queue mutex");
        if state.closed {
            return false;
        }
        state.values.push_back(value);
        self.changed.notify_one();
        true
    }

    fn recv(&self) -> Option<T> {
        let mut state = self.state.lock().expect("pipeline queue mutex");
        loop {
            if let Some(value) = state.values.pop_front() {
                return Some(value);
            }
            if state.closed {
                return None;
            }
            state = self.changed.wait(state).expect("pipeline queue mutex");
        }
    }

    fn close(&self) {
        let mut state = self.state.lock().expect("pipeline queue mutex");
        state.closed = true;
        self.changed.notify_all();
    }
}

struct PipelineFaultRuntime {
    configured: AddIndexPipelineFailpoints,
    scan_error_fired: AtomicBool,
    scan_cancel_fired: AtomicBool,
    write_error_fired: AtomicBool,
    completed_scan_tasks: AtomicUsize,
}

impl PipelineFaultRuntime {
    fn new(configured: AddIndexPipelineFailpoints) -> Self {
        Self {
            configured,
            scan_error_fired: AtomicBool::new(false),
            scan_cancel_fired: AtomicBool::new(false),
            write_error_fired: AtomicBool::new(false),
            completed_scan_tasks: AtomicUsize::new(0),
        }
    }

    fn fire_once(&self, path: &'static str, fired: &AtomicBool) -> bool {
        self.configured.contains(path) && !fired.swap(true, Ordering::AcqRel)
    }
}

/// The production source → scan → ingest → sink asynchronous ADD INDEX pipeline.
pub struct AddIndexIngestPipeline<W>
where
    W: IndexWriter + Send + 'static,
{
    context: BackfillOperatorContext,
    source: TableScanTaskSource,
    ranges: Vec<KeyRange>,
    scan: TableScanOperator,
    ingest: IndexIngestOperator<W>,
    failpoints: AddIndexPipelineFailpoints,
    summary: Arc<Mutex<AddIndexPipelineSummary>>,
    handle: Option<JoinHandle<Result<(), OperatorError>>>,
    started: bool,
}

/// Build a production asynchronous ADD INDEX pipeline.
#[allow(non_snake_case)]
pub fn NewAddIndexIngestPipeline<W>(
    context: BackfillOperatorContext,
    source: TableScanTaskSource,
    ranges: Vec<KeyRange>,
    rows: Vec<IndexRecord>,
    writer: W,
    index_count: usize,
    scan_workers: usize,
    ingest_workers: usize,
    scanner: TableScanWorker,
) -> AddIndexIngestPipeline<W>
where
    W: IndexWriter + Send + 'static,
{
    let scan = TableScanOperator::New(context.clone(), scanner, rows, scan_workers);
    let ingest = IndexIngestOperator::New(context.clone(), writer, index_count, ingest_workers);
    AddIndexIngestPipeline {
        context,
        source,
        ranges,
        scan,
        ingest,
        failpoints: AddIndexPipelineFailpoints::default(),
        summary: Arc::new(Mutex::new(AddIndexPipelineSummary::default())),
        handle: None,
        started: false,
    }
}

impl<W> AddIndexIngestPipeline<W>
where
    W: IndexWriter + Send + 'static,
{
    /// Enable a one-shot production failpoint for this pipeline run.
    #[allow(non_snake_case)]
    pub fn EnableFailpoint(&mut self, path: &str) -> Result<(), String> {
        if self.started {
            return Err("cannot enable a failpoint after pipeline start".to_owned());
        }
        self.failpoints.enable(path)
    }

    /// Return the reader/writer stages for pre-execution concurrency tuning.
    #[allow(non_snake_case)]
    pub fn GetReaderAndWriter(&mut self) -> (&mut TableScanOperator, &mut IndexIngestOperator<W>) {
        (&mut self.scan, &mut self.ingest)
    }

    /// Start the asynchronous pipeline and return after every stage is open.
    #[allow(non_snake_case)]
    pub fn Execute(&mut self) -> Result<(), OperatorError> {
        if self.started {
            return Err(OperatorError::Write(
                "ADD INDEX pipeline is already running".to_owned(),
            ));
        }
        self.scan.Open()?;
        if let Err(error) = self.ingest.Open() {
            let _ = self.scan.Close();
            return Err(error);
        }

        let context = self.context.clone();
        let source = self.source.clone();
        let ranges = self.ranges.clone();
        let scanner = Arc::clone(&self.scan.scanner);
        let rows = Arc::clone(&self.scan.rows);
        let scan_workers = self.scan.worker_count;
        let writer = Arc::clone(&self.ingest.writer);
        let index_count = self.ingest.index_count;
        let ingest_workers = self.ingest.worker_count;
        let failpoints = self.failpoints.clone();
        let summary = Arc::clone(&self.summary);
        self.handle = Some(thread::spawn(move || {
            execute_add_index_pipeline(
                context,
                source,
                ranges,
                scanner,
                rows,
                scan_workers,
                writer,
                index_count,
                ingest_workers,
                failpoints,
                summary,
            )
        }));
        self.started = true;
        Ok(())
    }

    /// Wait for every stage, close resources, and return the pipeline close error.
    #[allow(non_snake_case)]
    pub fn Close(&mut self) -> Result<(), OperatorError> {
        let result = match self.handle.take() {
            Some(handle) => match handle.join() {
                Ok(result) => result,
                Err(_) => {
                    self.context.on_error(OperatorError::Write(
                        "ADD INDEX pipeline worker panicked".to_owned(),
                    ));
                    Err(OperatorError::Cancelled)
                }
            },
            None if self.started => Err(OperatorError::Write(
                "ADD INDEX pipeline has no execution handle".to_owned(),
            )),
            None => Ok(()),
        };
        let scan_close = self.scan.Close();
        let ingest_close = self.ingest.Close();
        self.started = false;
        result.and(scan_close).and(ingest_close)
    }

    /// Return whether the asynchronous execution has started.
    #[allow(non_snake_case)]
    pub fn IsStarted(&self) -> bool {
        self.started
    }

    /// Snapshot the final pipeline statistics.
    #[allow(non_snake_case)]
    pub fn Summary(&self) -> AddIndexPipelineSummary {
        self.summary.lock().expect("pipeline summary mutex").clone()
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_add_index_pipeline<W>(
    context: BackfillOperatorContext,
    source: TableScanTaskSource,
    ranges: Vec<KeyRange>,
    scanner: Arc<TableScanWorker>,
    rows: Arc<Vec<IndexRecord>>,
    scan_workers: usize,
    writer: Arc<Mutex<W>>,
    index_count: usize,
    ingest_workers: usize,
    failpoints: AddIndexPipelineFailpoints,
    summary: Arc<Mutex<AddIndexPipelineSummary>>,
) -> Result<(), OperatorError>
where
    W: IndexWriter + Send + 'static,
{
    let tasks = source.generate_tasks(&ranges, &mut TaskIdAllocator::new())?;
    let task_count = tasks.len();
    let task_queue = Arc::new(StageQueue::new());
    let chunk_queue = Arc::new(StageQueue::new());
    let result_queue = Arc::new(StageQueue::new());
    let faults = Arc::new(PipelineFaultRuntime::new(failpoints));

    for task in tasks {
        let _ = task_queue.send(task);
    }
    task_queue.close();

    let mut ingest_handles = Vec::with_capacity(ingest_workers);
    for _ in 0..ingest_workers {
        let context = context.clone();
        let chunks = Arc::clone(&chunk_queue);
        let results = Arc::clone(&result_queue);
        let faults = Arc::clone(&faults);
        let writer = LockedIndexWriter {
            inner: Arc::clone(&writer),
        };
        ingest_handles.push(thread::spawn(move || {
            let mut worker = IndexIngestWorker {
                writer,
                index_count,
            };
            while let Some(chunk) = chunks.recv() {
                if context.IsCancelled() {
                    break;
                }
                if faults.fire_once(MOCK_WRITE_LOCAL_ERROR, &faults.write_error_fired) {
                    context.on_error(OperatorError::Write("mock write local error".to_owned()));
                    chunks.close();
                    results.close();
                    break;
                }
                match worker.write_chunk(&chunk) {
                    Ok(result) => {
                        if !results.send(result) {
                            break;
                        }
                    }
                    Err(error) => {
                        context.on_error(error);
                        chunks.close();
                        results.close();
                        break;
                    }
                }
            }
        }));
    }

    let mut scan_handles = Vec::with_capacity(scan_workers);
    for _ in 0..scan_workers {
        let context = context.clone();
        let tasks = Arc::clone(&task_queue);
        let chunks = Arc::clone(&chunk_queue);
        let scanner = Arc::clone(&scanner);
        let rows = Arc::clone(&rows);
        let faults = Arc::clone(&faults);
        scan_handles.push(thread::spawn(move || {
            while let Some(task) = tasks.recv() {
                if context.IsCancelled() {
                    break;
                }
                if faults.fire_once(MOCK_SCAN_RECORD_ERROR, &faults.scan_error_fired) {
                    context.on_error(OperatorError::Write("mock scan record error".to_owned()));
                    chunks.close();
                    break;
                }
                if faults.fire_once(SCAN_RECORD_EXEC, &faults.scan_cancel_fired) {
                    context.on_error(OperatorError::Cancelled);
                    chunks.close();
                    break;
                }
                match scanner.scan_records(&task, &rows) {
                    Ok(produced) => {
                        for chunk in produced {
                            if !chunks.send(chunk) {
                                break;
                            }
                        }
                    }
                    Err(error) => {
                        context.on_error(error);
                        chunks.close();
                        break;
                    }
                }
                let completed = faults.completed_scan_tasks.fetch_add(1, Ordering::AcqRel) + 1;
                if faults.configured.contains(WRITE_LOCAL_EXEC) && completed == task_count {
                    context.Cancel();
                    chunks.close();
                    break;
                }
            }
        }));
    }

    for handle in scan_handles {
        if handle.join().is_err() {
            context.on_error(OperatorError::Write(
                "table scan worker panicked".to_owned(),
            ));
        }
    }
    chunk_queue.close();
    for handle in ingest_handles {
        if handle.join().is_err() {
            context.on_error(OperatorError::Write(
                "index ingest worker panicked".to_owned(),
            ));
        }
    }
    result_queue.close();

    let mut results = Vec::new();
    while let Some(result) = result_queue.recv() {
        results.push(result);
    }
    if context.IsCancelled() {
        return Err(OperatorError::Cancelled);
    }

    let written_bytes = results.iter().map(|result| result.written_bytes).sum();
    let chunk_count = results.len();
    let mut sink = IndexWriteResultSink {
        writer: Some(LockedIndexWriter { inner: writer }),
        processed_rows: 0,
        total_rows: 0,
    };
    if faults.configured.contains(MOCK_FLUSH_ERROR) {
        let error = OperatorError::Flush("mock flush error".to_owned());
        context.on_error(error.clone());
        return Err(error);
    }
    if let Err(error) = sink.collect_results(&results) {
        context.on_error(error.clone());
        return Err(error);
    }
    *summary.lock().expect("pipeline summary mutex") = AddIndexPipelineSummary {
        task_count,
        chunk_count,
        processed_rows: sink.processed_rows,
        total_rows: sink.total_rows,
        written_bytes,
    };
    Ok(())
}

/// 临时索引扫描任务：合并阶段按键区间扫描临时索引。
///
/// 临时索引是在线加索引期间并发 DML 写入的暂存区（temp index），
/// 回填完成后需把其中的增量变更合并回正式索引。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TemporaryIndexScanTask {
    /// 任务编号。
    pub id: usize,
    /// 区间起始键（包含）。
    pub start: Key,
    /// 区间结束键（不包含）。
    pub end: Key,
}

/// 根据 Region 区间生成临时索引合并任务；空边界用索引前缀补全为整个索引范围。
pub fn generate_temporary_index_tasks(
    ranges: &[KeyRange],
    table_index_prefix: &[u8],
    allocator: &mut TaskIdAllocator,
) -> Vec<TemporaryIndexScanTask> {
    let prefix_end = prefix_next(table_index_prefix);
    ranges
        .iter()
        .map(|range| TemporaryIndexScanTask {
            id: allocator.alloc(),
            start: if range.start_key.is_empty() {
                table_index_prefix.to_vec()
            } else {
                range.start_key.clone()
            },
            end: if range.end_key.is_empty() {
                prefix_end.clone()
            } else {
                range.end_key.clone()
            },
        })
        .collect()
}

/// 临时索引中的一条记录，描述一次增量索引变更。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TemporaryIndexRecord {
    /// 临时索引中的键。
    pub temporary_key: Key,
    /// 对应的正式索引键。
    pub original_key: Key,
    /// 索引值。
    pub value: Vec<u8>,
    /// true 表示这是一次删除（需从正式索引删除该键）。
    pub delete: bool,
    /// true 表示该记录无需合并（例如已被后续变更覆盖）。
    pub skip: bool,
}

/// 一个临时索引合并任务（或其中一批）的处理结果。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TemporaryIndexResult {
    /// 所属任务编号。
    pub task_id: usize,
    /// 下一批扫描的起始键（最后一条已处理键的后继）。
    pub next_key: Key,
    /// 本批扫描到的记录数。
    pub scan_count: usize,
    /// 实际合并（写入/删除正式索引）的记录数。
    pub add_count: usize,
    /// 是否已处理完该区间（扫描数量不足一整批即为结束）。
    pub done: bool,
}

/// 临时索引合并的目标存储抽象：以原子批（事务）方式应用一组变更。
pub trait TemporaryIndexStore {
    fn apply_batch(
        &mut self,
        mutations: &[TemporaryIndexMutation],
    ) -> Result<(), TemporaryStoreError>;
}

/// 临时索引存储写入错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TemporaryStoreError {
    /// 可重试错误（如写冲突），调用方可缩小批大小后重试。
    Retryable,
    /// 不可恢复错误。
    Fatal(String),
}

/// 合并临时索引时对存储施加的单条变更。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TemporaryIndexMutation {
    /// 写入键值。
    Set { key: Key, value: Vec<u8> },
    /// 删除键。
    Delete { key: Key },
}

/// 临时索引合并工作者：把临时索引区间内的增量变更批量应用到正式索引。
pub struct MergeTemporaryIndexWorker<S> {
    /// 目标存储。
    pub store: S,
    /// 当前批大小（遇到可重试错误时会减半退避）。
    pub batch_count: usize,
    /// 单批最大重试次数。
    pub maximum_attempts: usize,
    /// 累计扫描的记录总数。
    pub total_scan_count: usize,
}

impl<S: TemporaryIndexStore> MergeTemporaryIndexWorker<S> {
    /// 处理一个区间内的一批临时索引记录：
    /// 每条记录先把变更写到正式索引键，再删除临时索引键；整批原子提交。
    /// 遇到可重试错误时把批大小减半重试，直至耗尽重试次数。
    pub fn handle_one_range(
        &mut self,
        task: &TemporaryIndexScanTask,
        records: &[TemporaryIndexRecord],
    ) -> Result<TemporaryIndexResult, OperatorError> {
        // 记住原始批大小，无论成功失败退出前都要恢复。
        let original_batch_count = self.batch_count.max(1);
        let mut attempts = 0;
        // 重试循环：成功时 break 出统计信息。
        let (selected_count, successful_batch_count, add_count, next_key) = loop {
            attempts += 1;
            // 从区间内最多取一批记录。
            let selected: Vec<&TemporaryIndexRecord> = records
                .iter()
                .filter(|record| {
                    record.temporary_key >= task.start && record.temporary_key < task.end
                })
                .take(self.batch_count.max(1))
                .collect();
            let mut mutations = Vec::new();
            let mut add_count = 0;
            for record in &selected {
                if record.skip {
                    continue;
                }
                // 先在正式索引上执行写入或删除。
                mutations.push(if record.delete {
                    TemporaryIndexMutation::Delete {
                        key: record.original_key.clone(),
                    }
                } else {
                    TemporaryIndexMutation::Set {
                        key: record.original_key.clone(),
                        value: record.value.clone(),
                    }
                });
                // 合并完成后删除对应的临时索引键。
                mutations.push(TemporaryIndexMutation::Delete {
                    key: record.temporary_key.clone(),
                });
                add_count += 1;
            }
            match self.store.apply_batch(&mutations) {
                Ok(()) => {
                    // 下一批从最后一条已处理键的字节序后继开始；本批为空则整个区间结束。
                    let next_key = selected
                        .last()
                        .map(|record| prefix_next(&record.temporary_key))
                        .unwrap_or_else(|| task.end.clone());
                    break (selected.len(), self.batch_count.max(1), add_count, next_key);
                }
                // 可重试错误：批大小减半（至少为 1）后重试，减少写冲突概率。
                Err(TemporaryStoreError::Retryable) if attempts < self.maximum_attempts.max(1) => {
                    self.batch_count = (self.batch_count / 2).max(1);
                }
                Err(TemporaryStoreError::Retryable) => {
                    self.batch_count = original_batch_count;
                    return Err(OperatorError::RetryExhausted);
                }
                Err(TemporaryStoreError::Fatal(error)) => {
                    self.batch_count = original_batch_count;
                    return Err(OperatorError::Write(error));
                }
            }
        };
        self.batch_count = original_batch_count;
        self.total_scan_count += selected_count;
        Ok(TemporaryIndexResult {
            task_id: task.id,
            next_key,
            scan_count: selected_count,
            add_count,
            done: selected_count < successful_batch_count,
        })
    }
}

/// 汇总所有临时索引合并结果，返回累计合并的记录数。
pub fn collect_temporary_index_results(results: &[TemporaryIndexResult]) -> i64 {
    results.iter().map(|result| result.add_count as i64).sum()
}

/// 计算键前缀的字节序后继（即大于所有以该前缀开头的键的最小键）。
///
/// 从末尾字节起进位加一；若全部字节为 0xFF，则追加一个 0 字节。
fn prefix_next(prefix: &[u8]) -> Key {
    let mut next = prefix.to_vec();
    for byte in next.iter_mut().rev() {
        if *byte != u8::MAX {
            *byte += 1;
            return next;
        }
        *byte = 0;
    }
    next.push(0);
    next
}

/// 基于内存 BTreeMap 的 `IndexWriter` 实现，用于测试与演示。
#[derive(Clone, Debug, Default)]
pub struct MemoryIndexWriter {
    /// 已写入的索引键值对（有序）。
    pub data: BTreeMap<Key, Vec<u8>>,
    /// flush 被调用的次数。
    pub flush_count: usize,
    /// 记录每次配额检查的 (task_id, row_count) 参数。
    pub quota_checks: Vec<(usize, i64)>,
}

impl IndexWriter for MemoryIndexWriter {
    fn write(&mut self, key: &[u8], value: &[u8]) -> Result<usize, String> {
        self.data.insert(key.to_vec(), value.to_vec());
        Ok(key.len() + value.len())
    }

    fn flush(&mut self) -> Result<(), String> {
        self.flush_count += 1;
        Ok(())
    }

    fn ingest_if_quota_exceeded(&mut self, task_id: usize, row_count: i64) -> Result<(), String> {
        self.quota_checks.push((task_id, row_count));
        Ok(())
    }

    fn total_key_count(&self) -> i64 {
        self.data.len() as i64
    }
}
