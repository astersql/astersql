// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// DistSQL（分布式 SQL）执行路径：IndexReader / IndexLookUp。
//
// DistSQL 把读请求下推到 TiKV/TiFlash 等存储节点：按 key range（键范围）
// 发起 coprocessor / 扫描，再在 TiDB 侧汇总结果。
//
// - [`IndexReaderExecutor`]：只扫索引，结果即为查询输出；
// - [`IndexLookUpExecutor`]：先扫索引取 handle（行定位键），再回表取完整行；
//   索引 worker 与 table worker 通过 channel 流水线并行。
//
// Handle 是行在表中的唯一标识（整型主键、common handle 或带分区前缀）。

#![allow(non_camel_case_types, non_snake_case)]

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::{Display, Formatter};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use astersql_util_memory::tracker::Tracker;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
/// 行定位键：整型、编码后的 common handle，或「分区 id + 内层 handle」。
pub enum Handle {
    Int(i64),
    Common(Vec<u8>),
    Partition(i64, Box<Handle>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// DistSQL 路径上简化的列值表示。
pub enum Datum {
    Null,
    Signed(i64),
    Unsigned(u64),
    Bytes(Vec<u8>),
    Text(String),
}
/// 一行数据：按列偏移排列的 Datum 列表。
pub type Row = Vec<Datum>;

#[derive(Clone, Debug, PartialEq, Eq)]
/// 半开键区间 [start, end)，用于下推扫描范围。
pub struct KeyRange {
    pub start: Vec<u8>,
    pub end: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 排序项：列偏移与是否降序，用于 merge sort。
pub struct ByItem {
    pub offset: usize,
    pub descending: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 下推到索引扫描的 LIMIT（offset + count）。
pub struct Limit {
    pub offset: usize,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// DistSQL 读路径错误，含取消、关闭、解码与索引/表不一致等。
pub enum DistSqlError {
    Backend(String),
    Cancelled,
    Closed,
    Decode(String),
    Inconsistent {
        expected: usize,
        obtained: usize,
        missing: Vec<Handle>,
    },
    InvalidPlan(String),
    WorkerPanic(String),
}
impl Display for DistSqlError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for DistSqlError {}

#[derive(Clone, Debug)]
/// 发给存储层的一次选择/扫描请求参数。
pub struct Request {
    pub ranges: Vec<KeyRange>,
    pub descending: bool,
    pub keep_order: bool,
    pub concurrency: usize,
    pub plan_ids: Vec<i32>,
    pub collect_stats: bool,
}

/// 流式选择结果：按容量拉取行，并报告 in-flight 代价。
pub trait SelectResult: Send {
    fn next(&mut self, capacity: usize) -> Result<Vec<Row>, DistSqlError>;
    fn close(&mut self) -> Result<(), DistSqlError>;
    fn in_flight_cost(&self) -> usize;
}

/// 存储访问与行比较/校验的后端抽象。
///
/// 生产环境对接 TiKV client；测试可注入 mock。
pub trait DistSqlBackend: Send + Sync + 'static {
    fn select(&self, request: Request) -> Result<Box<dyn SelectResult>, DistSqlError>;
    fn rebuild_index_ranges(
        &self,
        access_conditions: &[String],
        index_columns: &[i64],
        column_lengths: &[i32],
    ) -> Result<Vec<KeyRange>, DistSqlError>;
    fn index_ranges(
        &self,
        table_ids: &[i64],
        index_id: i64,
        ranges: &[KeyRange],
    ) -> Result<Vec<KeyRange>, DistSqlError>;
    fn table_ranges(
        &self,
        table_id: i64,
        handles: &[Handle],
    ) -> Result<Vec<KeyRange>, DistSqlError>;
    fn index_scan(
        &self,
        ranges: &[kvRangesWithPhysicalTblID],
        request: &Request,
    ) -> Result<Vec<Row>, DistSqlError>;
    fn table_scan(
        &self,
        table_id: i64,
        handles: &[Handle],
        request: &Request,
    ) -> Result<Vec<Row>, DistSqlError>;
    fn decode_handle(
        &self,
        row: &[Datum],
        offsets: &[usize],
        common: bool,
        partition: bool,
    ) -> Result<Handle, DistSqlError>;
    fn row_handle(
        &self,
        row: &[Datum],
        offsets: &[usize],
        common: bool,
    ) -> Result<Handle, DistSqlError>;
    fn compare_rows(
        &self,
        left: &[Datum],
        right: &[Datum],
        by: &[ByItem],
    ) -> Result<Ordering, DistSqlError>;
    fn checksum(&self, row: &[Datum]) -> Result<u64, DistSqlError>;
    fn report_inconsistency(
        &self,
        expected: &[Handle],
        obtained: &[Handle],
        missing: &[Handle],
    ) -> Result<(), DistSqlError>;
}

/// 可关闭资源，供 `closeAll` 统一收尾。
pub trait Closeable {
    fn Close(&mut self) -> Result<(), DistSqlError>;
}
/// 依次 Close，返回第一个错误（仍继续关闭其余对象）。
pub fn closeAll(objects: &mut [&mut dyn Closeable]) -> Result<(), DistSqlError> {
    let mut first = None;
    for object in objects {
        if let Err(error) = object.Close() {
            if first.is_none() {
                first = Some(error);
            }
        }
    }
    first.map_or(Ok(()), Err)
}

#[derive(Clone, Debug, Default)]
/// IndexLookUp 流水线中的一个回表任务（handles → 表行）。
pub struct lookupTableTask {
    pub id: usize,
    pub handles: Vec<Handle>,
    pub rows: Vec<Row>,
    pub index_rows: Vec<Row>,
    pub row_idx: Vec<usize>,
    pub error: Option<DistSqlError>,
    pub build_done_time: Option<Instant>,
    pub mem_usage: usize,
}
impl lookupTableTask {
    /// 排序辅助：当前任务行数。
    pub fn Len(&self) -> usize {
        self.row_idx.len()
    }
    /// 按原始索引序比较两行（保持索引扫描顺序）。
    pub fn Less(&self, i: usize, j: usize) -> bool {
        self.row_idx[i] < self.row_idx[j]
    }
    /// 交换两行及其 row_idx。
    pub fn Swap(&mut self, i: usize, j: usize) {
        self.row_idx.swap(i, j);
        self.rows.swap(i, j);
    }
}

/// 根据访问条件重建索引 key ranges。
pub fn rebuildIndexRanges<B: DistSqlBackend>(
    backend: &B,
    access: &[String],
    columns: &[i64],
    lengths: &[i32],
) -> Result<Vec<KeyRange>, DistSqlError> {
    backend.rebuild_index_ranges(access, columns, lengths)
}

#[derive(Clone)]
/// IndexReader 共享上下文：后端、并发度与弱一致读标记。
pub struct indexReaderExecutorContext<B: DistSqlBackend> {
    pub backend: Arc<B>,
    pub dist_sql_concurrency: usize,
    pub weak_consistency: bool,
}
/// 构造 IndexReader 上下文；concurrency 至少为 1。
pub fn newIndexReaderExecutorContext<B: DistSqlBackend>(
    backend: Arc<B>,
    concurrency: usize,
    weak: bool,
) -> indexReaderExecutorContext<B> {
    indexReaderExecutorContext {
        backend,
        dist_sql_concurrency: concurrency.max(1),
        weak_consistency: weak,
    }
}

/// 索引读执行器：Open 时建 KV 请求，Next 拉行；多 range 有序时可能本地 merge sort。
pub struct IndexReaderExecutor<B: DistSqlBackend> {
    pub context: indexReaderExecutorContext<B>,
    pub table_id: i64,
    pub index_id: i64,
    pub plans: Vec<i32>,
    pub ranges: Vec<KeyRange>,
    pub access_conditions: Vec<String>,
    pub index_columns: Vec<i64>,
    pub column_lengths: Vec<i32>,
    pub table_ids: Vec<i64>,
    pub by_items: Vec<ByItem>,
    pub descending: bool,
    pub keep_order: bool,
    pub dummy: bool,
    pub result: Option<Box<dyn SelectResult>>,
    pub merged_rows: VecDeque<Row>,
    pub runtime_rows: u64,
    /// Index Join inner tasks charge range construction to their task tracker.
    pub range_mem_tracker: Option<Arc<Tracker>>,
}
impl<B: DistSqlBackend> IndexReaderExecutor<B> {
    /// 返回逻辑表 id。
    pub fn Table(&self) -> i64 {
        self.table_id
    }
    /// 标记为 dummy：Open 不真正发请求（用于某些计划短路）。
    pub fn setDummy(&mut self) {
        self.dummy = true;
    }
    /// 关闭 SelectResult 并清空 merge 缓冲。
    pub fn Close(&mut self) -> Result<(), DistSqlError> {
        if let Some(result) = self.result.as_mut() {
            result.close()?;
        }
        self.result = None;
        self.merged_rows.clear();
        Ok(())
    }
    /// 先消费 merge 缓冲，再从 SelectResult 拉取至多 capacity 行。
    pub fn Next(&mut self, capacity: usize) -> Result<Vec<Row>, DistSqlError> {
        if !self.merged_rows.is_empty() {
            let count = capacity.min(self.merged_rows.len());
            let rows = self.merged_rows.drain(..count).collect::<Vec<_>>();
            self.runtime_rows += rows.len() as u64;
            return Ok(rows);
        }
        let rows = match self.result.as_mut() {
            Some(result) => result.next(capacity)?,
            None => Vec::new(),
        };
        self.runtime_rows += rows.len() as u64;
        Ok(rows)
    }
    /// 按需重建 ranges，构建 KV 范围并 open。
    pub fn Open(&mut self) -> Result<(), DistSqlError> {
        if !self.access_conditions.is_empty() {
            self.ranges = rebuildIndexRanges(
                self.context.backend.as_ref(),
                &self.access_conditions,
                &self.index_columns,
                &self.column_lengths,
            )?;
        }
        let ranges = self.buildKVRangesForIndexReader()?;
        self.open(ranges)
    }
    /// 把逻辑索引 ranges 映射为物理 KV ranges。
    pub fn buildKVRangesForIndexReader(&self) -> Result<Vec<KeyRange>, DistSqlError> {
        let ranges =
            self.context
                .backend
                .index_ranges(&self.table_ids, self.index_id, &self.ranges)?;
        consume_key_range_memory(self.range_mem_tracker.as_deref(), &ranges);
        Ok(ranges)
    }
    /// 组装下推 Request。
    pub fn buildKVReq(&self, ranges: Vec<KeyRange>) -> Result<Request, DistSqlError> {
        Ok(Request {
            ranges,
            descending: self.descending,
            keep_order: self.keep_order,
            concurrency: self.context.dist_sql_concurrency,
            plan_ids: self.plans.clone(),
            collect_stats: true,
        })
    }
    /// 排序/反转 ranges；多 range 有序时本地 merge sort，否则直接 select。
    pub fn open(&mut self, mut ranges: Vec<KeyRange>) -> Result<(), DistSqlError> {
        if self.dummy {
            return Ok(());
        }
        ranges.sort_by(|a, b| a.start.cmp(&b.start));
        if self.descending {
            ranges.reverse();
        }
        // 多段 range 且需要保序时，逐段拉取后统一排序，结果放入 merged_rows。
        if needMergeSort(&self.by_items, ranges.len()) {
            let mut all = Vec::new();
            for range in ranges {
                let mut result = self.context.backend.select(self.buildKVReq(vec![range])?)?;
                loop {
                    let rows = result.next(1024)?;
                    if rows.is_empty() {
                        break;
                    }
                    all.extend(rows);
                }
                result.close()?;
            }
            sort_rows(self.context.backend.as_ref(), &mut all, &self.by_items)?;
            self.merged_rows = all.into();
        } else {
            self.result = Some(self.context.backend.select(self.buildKVReq(ranges)?)?);
        }
        Ok(())
    }
}
impl<B: DistSqlBackend> Closeable for IndexReaderExecutor<B> {
    fn Close(&mut self) -> Result<(), DistSqlError> {
        IndexReaderExecutor::Close(self)
    }
}

#[derive(Clone)]
/// IndexLookUp 共享上下文。
pub struct indexLookUpExecutorContext<B: DistSqlBackend> {
    pub backend: Arc<B>,
    pub dist_sql_concurrency: usize,
    pub weak_consistency: bool,
}
/// 构造 IndexLookUp 上下文；concurrency 至少为 1。
pub fn newIndexLookUpExecutorContext<B: DistSqlBackend>(
    backend: Arc<B>,
    concurrency: usize,
    weak: bool,
) -> indexLookUpExecutorContext<B> {
    indexLookUpExecutorContext {
        backend,
        dist_sql_concurrency: concurrency.max(1),
        weak_consistency: weak,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 绑定到某个物理表 id 的一组 KV ranges（分区表场景）。
pub struct kvRangesWithPhysicalTblID {
    pub physicalTblID: i64,
    pub keyRanges: Vec<KeyRange>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 从索引行或表行解码 handle 的来源。
pub enum getHandleType {
    GetHandleFromIndex,
    GetHandleFromTable,
}
#[derive(Clone, Debug)]
/// 是否对索引行与表行做 checksum 一致性检查。
pub struct checkIndexValue {
    pub checksum: bool,
}

/// 索引回表执行器：索引 worker 抽 handle，table worker 并发回表，主线程按序输出。
pub struct IndexLookUpExecutor<B: DistSqlBackend> {
    pub context: indexLookUpExecutorContext<B>,
    pub table_id: i64,
    pub index_id: i64,
    pub idx_plans: Vec<i32>,
    pub tbl_plans: Vec<i32>,
    pub ranges: Vec<KeyRange>,
    pub grouped_kv_ranges: Vec<kvRangesWithPhysicalTblID>,
    pub grouped_ranges: Vec<kvRangesWithPhysicalTblID>,
    pub partition_range_map: BTreeMap<i64, Vec<KeyRange>>,
    pub handle_offsets: Vec<usize>,
    pub common_handle: bool,
    pub partition_mode: bool,
    pub keep_order: bool,
    pub descending: bool,
    pub pushed_limit: Option<Limit>,
    pub batch_size: usize,
    pub max_batch_size: usize,
    pub check_index_value: Option<checkIndexValue>,
    pub dummy: bool,
    pub cancelled: Arc<AtomicBool>,
    pub result_tx: Option<mpsc::SyncSender<lookupTableTask>>,
    pub result_rx: Option<mpsc::Receiver<lookupTableTask>>,
    pub table_tx: Option<mpsc::SyncSender<lookupTableTask>>,
    pub index_join: Option<JoinHandle<()>>,
    pub table_joins: Vec<JoinHandle<()>>,
    pub pending: BTreeMap<usize, lookupTableTask>,
    pub next_task_id: usize,
    pub current: VecDeque<Row>,
    pub stats: Arc<Mutex<IndexLookUpRunTimeStats>>,
    /// Regular readers charge ranges to this executor-owned tracker.
    pub mem_tracker: Option<Arc<Tracker>>,
    /// Index Join inner tasks override `mem_tracker` with their task tracker.
    pub range_mem_tracker: Option<Arc<Tracker>>,
}

impl<B: DistSqlBackend> IndexLookUpExecutor<B> {
    /// 返回逻辑表 id。
    pub fn Table(&self) -> i64 {
        self.table_id
    }
    /// 标记 dummy：不启动 worker。
    pub fn setDummy(&mut self) {
        self.dummy = true;
    }
    /// 构建表侧 key ranges，初始化 channel，并启动流水线 worker。
    pub fn Open(&mut self) -> Result<(), DistSqlError> {
        self.buildTableKeyRanges()?;
        self.open()?;
        if !self.dummy {
            self.startWorkers(self.batch_size.max(1))?;
        }
        Ok(())
    }
    /// 按分区映射构建 grouped_kv_ranges。
    pub fn buildTableKeyRanges(&mut self) -> Result<(), DistSqlError> {
        self.grouped_kv_ranges = buildKeyRanges(
            self.context.backend.as_ref(),
            &self.ranges,
            &self.partition_range_map,
            self.table_id,
            self.index_id,
            self.range_mem_tracker
                .as_deref()
                .or(self.mem_tracker.as_deref()),
        )?;
        Ok(())
    }
    /// 初始化运行时统计与结果/回表 channel。
    pub fn open(&mut self) -> Result<(), DistSqlError> {
        self.initRuntimeStats();
        let (result_tx, result_rx) = mpsc::sync_channel(32);
        let (table_tx, _) = mpsc::sync_channel(32);
        self.result_tx = Some(result_tx);
        self.result_rx = Some(result_rx);
        self.table_tx = Some(table_tx);
        Ok(())
    }
    /// 启动 concurrency 个 table worker 与 1 个 index worker（fetchHandles）。
    pub fn startWorkers(&mut self, init_batch_size: usize) -> Result<(), DistSqlError> {
        let (table_tx, table_rx) = mpsc::sync_channel::<lookupTableTask>(32);
        self.table_tx = Some(table_tx.clone());
        let shared_rx = Arc::new(Mutex::new(table_rx));
        let result_tx = self.result_tx.as_ref().ok_or(DistSqlError::Closed)?.clone();
        for _ in 0..self.context.dist_sql_concurrency {
            let backend = Arc::clone(&self.context.backend);
            let rx = Arc::clone(&shared_rx);
            let tx = result_tx.clone();
            let cancelled = Arc::clone(&self.cancelled);
            let weak = self.context.weak_consistency;
            let table_id = self.table_id;
            let request = self.table_request();
            let offsets = self.handle_offsets.clone();
            let common = self.common_handle;
            let keep = self.keep_order;
            let stats = Arc::clone(&self.stats);
            let checker = self.check_index_value.is_some();
            // table worker：从共享队列取 lookup 任务，回表后送入 result channel。
            self.table_joins.push(thread::spawn(move || {
                let mut worker = tableWorker {
                    backend,
                    table_id,
                    request,
                    handle_idx: offsets,
                    common_handle: common,
                    keep_order: keep,
                    weak_consistency: weak,
                    check_index_value: checker,
                    stats,
                };
                loop {
                    if cancelled.load(AtomicOrdering::Acquire) {
                        break;
                    }
                    let task = rx.lock().expect("table task receiver poisoned").recv();
                    let Ok(mut task) = task else {
                        break;
                    };
                    if let Err(error) = worker.executeTask(&mut task) {
                        task.error = Some(error);
                        cancelled.store(true, AtomicOrdering::Release);
                    }
                    if tx.send(task).is_err() {
                        break;
                    }
                }
            }));
        }
        let backend = Arc::clone(&self.context.backend);
        let ranges = self.grouped_kv_ranges.clone();
        let request = self.index_request();
        let offsets = self.handle_offsets.clone();
        let common = self.common_handle;
        let partition = self.partition_mode;
        let keep = self.keep_order;
        let limit = self.pushed_limit.clone();
        let cancelled = Arc::clone(&self.cancelled);
        let stats = Arc::clone(&self.stats);
        let index_error_tx = result_tx.clone();
        // index worker：扫索引抽 handle，按批 dispatch 到 table_tx。
        self.index_join = Some(thread::spawn(move || {
            let mut worker = indexWorker {
                backend,
                table_tx,
                ranges,
                request,
                handle_offsets: offsets,
                common_handle: common,
                partition_mode: partition,
                keep_order: keep,
                pushed_limit: limit,
                batch_size: init_batch_size.max(1),
                max_batch_size: 1024,
                scanned_keys: 0,
                stats,
            };
            if let Err(error) = worker.fetchHandles() {
                let _ = index_error_tx.send(lookupTableTask {
                    error: Some(error),
                    ..Default::default()
                });
                cancelled.store(true, AtomicOrdering::Release);
            }
        }));
        self.table_tx.take();
        self.result_tx.take();
        Ok(())
    }
    /// 分区模式下 handle 需带分区前缀。
    pub fn needPartitionHandle(&self, _kind: getHandleType) -> Result<bool, DistSqlError> {
        Ok(self.partition_mode)
    }
    /// 是否为 clustered index / common handle 表。
    pub fn isCommonHandle(&self) -> bool {
        self.common_handle
    }
    /// 返回 handle 列偏移（兼容 Go 命名）。
    pub fn getRetTpsForIndexReader(&self) -> Vec<usize> {
        self.handle_offsets.clone()
    }
    /// 启动索引侧 worker（等价于 startWorkers）。
    pub fn startIndexWorker(&mut self, init_batch_size: usize) -> Result<(), DistSqlError> {
        self.startWorkers(init_batch_size)
    }
    /// 对给定物理 ranges 执行索引扫描。
    pub fn buildIndexSelectResultForRange(
        &self,
        ranges: &[kvRangesWithPhysicalTblID],
    ) -> Result<Vec<Row>, DistSqlError> {
        self.context
            .backend
            .index_scan(ranges, &self.index_request())
    }
    /// 结合 pushed LIMIT 估计批大小。
    pub fn calculateBatchSize(&self, init: usize, max: usize) -> usize {
        CalculateBatchSize(
            self.pushed_limit
                .as_ref()
                .map_or(max, |limit| limit.offset.saturating_add(limit.count)),
            init,
            max,
        )
    }
    /// 按任务 handles 回表扫描。
    pub fn buildTableReader(&self, task: &lookupTableTask) -> Result<Vec<Row>, DistSqlError> {
        self.context
            .backend
            .table_scan(self.table_id, &task.handles, &self.table_request())
    }
    /// 取消并 join 所有 worker，清空缓冲。
    pub fn Close(&mut self) -> Result<(), DistSqlError> {
        self.cancelled.store(true, AtomicOrdering::Release);
        self.table_tx.take();
        if let Some(join) = self.index_join.take() {
            join.join().map_err(panic_error)?;
        }
        for join in self.table_joins.drain(..) {
            join.join().map_err(panic_error)?;
        }
        self.result_tx.take();
        self.current.clear();
        Ok(())
    }
    /// 从结果队列攒够 capacity 行后返回。
    pub fn Next(&mut self, capacity: usize) -> Result<Vec<Row>, DistSqlError> {
        while self.current.len() < capacity {
            let Some(task) = self.getResultTask()? else {
                break;
            };
            if let Some(error) = task.error {
                return Err(error);
            }
            self.current.extend(task.rows);
        }
        Ok(self
            .current
            .drain(..capacity.min(self.current.len()))
            .collect())
    }
    /// 按 task id 保序取出下一个完成任务；乱序到达的先放入 pending。
    pub fn getResultTask(&mut self) -> Result<Option<lookupTableTask>, DistSqlError> {
        if let Some(task) = self.pending.remove(&self.next_task_id) {
            self.next_task_id += 1;
            return Ok(Some(task));
        }
        loop {
            let task = match self.result_rx.as_ref() {
                Some(rx) => match rx.recv() {
                    Ok(task) => task,
                    Err(_) => return Ok(None),
                },
                None => return Ok(None),
            };
            if !self.keep_order || task.id == self.next_task_id {
                self.next_task_id = task.id + 1;
                return Ok(Some(task));
            }
            self.pending.insert(task.id, task);
        }
    }
    /// 重置 IndexLookUp 运行时统计。
    pub fn initRuntimeStats(&mut self) {
        *self.stats.lock().expect("runtime stats poisoned") = IndexLookUpRunTimeStats::default();
    }
    /// 索引侧计划根节点 id。
    pub fn getIndexPlanRootID(&self) -> i32 {
        self.idx_plans.last().copied().unwrap_or_default()
    }
    /// 表侧计划根节点 id。
    pub fn getTableRootPlanID(&self) -> i32 {
        self.tbl_plans.last().copied().unwrap_or_default()
    }
    /// 按来源从行中解码 handle。
    pub fn getHandle(
        &self,
        row: &[Datum],
        offsets: &[usize],
        kind: getHandleType,
    ) -> Result<Handle, DistSqlError> {
        match kind {
            getHandleType::GetHandleFromIndex => self.context.backend.decode_handle(
                row,
                offsets,
                self.common_handle,
                self.partition_mode,
            ),
            getHandleType::GetHandleFromTable => {
                self.context
                    .backend
                    .row_handle(row, offsets, self.common_handle)
            }
        }
    }
    /// 构造索引扫描请求模板。
    fn index_request(&self) -> Request {
        Request {
            ranges: Vec::new(),
            descending: self.descending,
            keep_order: self.keep_order,
            concurrency: self.context.dist_sql_concurrency,
            plan_ids: self.idx_plans.clone(),
            collect_stats: true,
        }
    }
    /// 构造回表扫描请求模板。
    fn table_request(&self) -> Request {
        Request {
            ranges: Vec::new(),
            descending: false,
            keep_order: self.keep_order,
            concurrency: self.context.dist_sql_concurrency,
            plan_ids: self.tbl_plans.clone(),
            collect_stats: true,
        }
    }
}
impl<B: DistSqlBackend> Closeable for IndexLookUpExecutor<B> {
    fn Close(&mut self) -> Result<(), DistSqlError> {
        IndexLookUpExecutor::Close(self)
    }
}

/// 按整表或分区映射构建带物理表 id 的索引 KV ranges。
pub fn buildKeyRanges<B: DistSqlBackend>(
    backend: &B,
    ranges: &[KeyRange],
    partitions: &BTreeMap<i64, Vec<KeyRange>>,
    table_id: i64,
    index_id: i64,
    mem_tracker: Option<&Tracker>,
) -> Result<Vec<kvRangesWithPhysicalTblID>, DistSqlError> {
    if partitions.is_empty() {
        let key_ranges = backend.index_ranges(&[table_id], index_id, ranges)?;
        consume_key_range_memory(mem_tracker, &key_ranges);
        Ok(vec![kvRangesWithPhysicalTblID {
            physicalTblID: table_id,
            keyRanges: key_ranges,
        }])
    } else {
        let mut result = Vec::new();
        for (id, part_ranges) in partitions {
            let key_ranges = backend.index_ranges(&[*id], index_id, part_ranges)?;
            consume_key_range_memory(mem_tracker, &key_ranges);
            result.push(kvRangesWithPhysicalTblID {
                physicalTblID: *id,
                keyRanges: key_ranges,
            });
        }
        Ok(result)
    }
}

fn consume_key_range_memory(mem_tracker: Option<&Tracker>, ranges: &[KeyRange]) {
    let Some(mem_tracker) = mem_tracker else {
        return;
    };
    let bytes = ranges.iter().fold(0usize, |total, range| {
        total
            .saturating_add(std::mem::size_of::<KeyRange>())
            .saturating_add(range.start.capacity())
            .saturating_add(range.end.capacity())
    });
    mem_tracker.Consume(i64::try_from(bytes).unwrap_or(i64::MAX));
}
/// 索引扫描最大 in-flight 数：concurrency * 2。
pub fn getIndexScanMaxInFlight(concurrency: usize) -> usize {
    concurrency.max(1).saturating_mul(2)
}
/// 读取 SelectResult 的 in-flight 代价，至少为 1。
pub fn getSelectResultInFlightCost(result: &dyn SelectResult) -> usize {
    result.in_flight_cost().max(1)
}
/// merge sort 时共享 coprocessor 请求速率上限。
pub fn getMergeSortSharedCoprRequestRateLimit(
    need_merge: bool,
    concurrency: usize,
) -> Option<usize> {
    need_merge.then(|| concurrency.max(1))
}
/// merge sort 时索引扫描并发：取 ranges 与 concurrency 的较小值。
pub fn getMergeSortIndexScanConcurrency(
    need_merge: bool,
    ranges: usize,
    concurrency: usize,
) -> usize {
    if need_merge {
        ranges.min(concurrency.max(1)).max(1)
    } else {
        concurrency.max(1)
    }
}
/// 按 Go `CalculateBatchSize` 逐次翻倍，直到覆盖估算行数，并受最大批大小限制。
pub fn CalculateBatchSize(estimated: usize, initial: usize, maximum: usize) -> usize {
    let mut batch_size = initial.min(maximum);
    if estimated >= maximum {
        return maximum;
    }
    while batch_size < estimated {
        batch_size = batch_size.saturating_mul(2);
        if batch_size >= maximum {
            return maximum;
        }
    }
    batch_size
}

/// 索引侧 worker：扫索引、抽 handle、按批派发回表任务。
pub struct indexWorker<B: DistSqlBackend> {
    pub backend: Arc<B>,
    pub table_tx: mpsc::SyncSender<lookupTableTask>,
    pub ranges: Vec<kvRangesWithPhysicalTblID>,
    pub request: Request,
    pub handle_offsets: Vec<usize>,
    pub common_handle: bool,
    pub partition_mode: bool,
    pub keep_order: bool,
    pub pushed_limit: Option<Limit>,
    pub batch_size: usize,
    pub max_batch_size: usize,
    pub scanned_keys: usize,
    pub stats: Arc<Mutex<IndexLookUpRunTimeStats>>,
}
impl<B: DistSqlBackend> indexWorker<B> {
    /// 把错误包装成任务发往 table_tx。
    pub fn syncErr(&self, error: DistSqlError) {
        let _ = self.table_tx.send(lookupTableTask {
            error: Some(error),
            ..Default::default()
        });
    }
    /// 扫完整索引结果，按递增 batch_size 切分并派发回表任务。
    pub fn fetchHandles(&mut self) -> Result<(), DistSqlError> {
        let start = Instant::now();
        let rows = self.backend.index_scan(&self.ranges, &self.request)?;
        let mut task_id = 0;
        let mut cursor = 0;
        while cursor < rows.len() && !self.limitReached() {
            let end = (cursor + self.batch_size).min(rows.len());
            let (completed, handles, _) =
                self.extractLookUpPushDownRowsOrHandles(&rows[cursor..end])?;
            let mut data = extractedLookupTaskData {
                completed_rows: completed,
                handles,
                exhausted: end == rows.len(),
                index_rows: rows[cursor..end].to_vec(),
            };
            if self.buildAndDispatchLookupTasks(task_id, &mut data)? {
                break;
            }
            task_id += 1;
            self.scanned_keys += end - cursor;
            cursor = end;
            // 批大小指数增长直至 max_batch_size，降低小批调度开销。
            self.batch_size = (self.batch_size * 2).min(self.max_batch_size);
        }
        let mut stats = self.stats.lock().expect("runtime stats poisoned");
        stats.fetch_handle += 1;
        stats.fetch_handle_total += start.elapsed();
        Ok(())
    }
    /// 滚动拉取多个 SelectResult，再一次性派发。
    pub fn fetchHandlesRolling(
        &mut self,
        _max_in_flight: usize,
        _range_count: usize,
        _types: &[usize],
        mut build_next: nextSelectResultBuilder,
    ) -> Result<(), DistSqlError> {
        let mut all = Vec::new();
        loop {
            let (mut result, done) = build_next()?;
            while let Some(row) = result.Next()? {
                all.push(row);
            }
            result.Close();
            if done {
                break;
            }
        }
        let (rows, handles, _) = self.extractLookUpPushDownRowsOrHandles(&all)?;
        let mut data = extractedLookupTaskData {
            completed_rows: rows,
            handles,
            exhausted: true,
            index_rows: all,
        };
        self.buildAndDispatchLookupTasks(0, &mut data)?;
        Ok(())
    }
    /// 准备 handle 列偏移。
    pub fn prepareHandleFetch(&self, index_types: &[usize]) -> Result<Vec<usize>, DistSqlError> {
        self.getHandleOffsets(index_types.len())
    }
    /// 从索引行提取 completed_rows / handles。
    pub fn extractLookupTaskData(
        &self,
        rows: &[Row],
    ) -> Result<extractedLookupTaskData, DistSqlError> {
        let (completed, handles, exhausted) = self.extractLookUpPushDownRowsOrHandles(rows)?;
        Ok(extractedLookupTaskData {
            completed_rows: completed,
            handles,
            exhausted,
            index_rows: rows.to_vec(),
        })
    }
    /// 无 handle 则直接完成任务，否则构建回表任务并 send。
    pub fn buildAndDispatchLookupTasks(
        &self,
        task_id: usize,
        data: &mut extractedLookupTaskData,
    ) -> Result<bool, DistSqlError> {
        if data.handles.is_empty() && data.completed_rows.is_empty() {
            return Ok(false);
        }
        let task = if data.handles.is_empty() {
            self.buildCompletedTask(task_id, std::mem::take(&mut data.completed_rows))
        } else {
            self.buildTableTask(
                task_id,
                std::mem::take(&mut data.handles),
                std::mem::take(&mut data.index_rows),
            )
        };
        self.table_tx.send(task).map_err(|_| DistSqlError::Closed)?;
        Ok(false)
    }
    /// 显式偏移优先，否则默认取最后一列。
    pub fn getHandleOffsets(&self, index_types_len: usize) -> Result<Vec<usize>, DistSqlError> {
        if !self.handle_offsets.is_empty() {
            return Ok(self.handle_offsets.clone());
        }
        if index_types_len == 0 {
            Err(DistSqlError::InvalidPlan(
                "index result has no handle column".into(),
            ))
        } else {
            Ok(vec![index_types_len - 1])
        }
    }
    /// 从索引行解码 handles（push-down 完成行暂为空）。
    pub fn extractLookUpPushDownRowsOrHandles(
        &self,
        rows: &[Row],
    ) -> Result<(Vec<Row>, Vec<Handle>, bool), DistSqlError> {
        let mut handles = Vec::with_capacity(rows.len());
        for row in rows {
            handles.push(self.backend.decode_handle(
                row,
                &self.handle_offsets,
                self.common_handle,
                self.partition_mode,
            )?);
        }
        Ok((Vec::new(), handles, rows.is_empty()))
    }
    /// 仅提取 handles。
    pub fn extractTaskHandles(&self, rows: &[Row]) -> Result<Vec<Handle>, DistSqlError> {
        self.extractLookUpPushDownRowsOrHandles(rows)
            .map(|(_, handles, _)| handles)
    }
    /// 构造已完成（无需回表）的任务。
    pub fn buildCompletedTask(&self, task_id: usize, rows: Vec<Row>) -> lookupTableTask {
        lookupTableTask {
            id: task_id,
            rows,
            build_done_time: Some(Instant::now()),
            ..Default::default()
        }
    }
    /// 构造待回表任务，row_idx 记录原始顺序。
    pub fn buildTableTask(
        &self,
        task_id: usize,
        handles: Vec<Handle>,
        index_rows: Vec<Row>,
    ) -> lookupTableTask {
        let row_idx = (0..handles.len()).collect();
        lookupTableTask {
            id: task_id,
            handles,
            index_rows,
            row_idx,
            ..Default::default()
        }
    }
    /// 是否已扫过 pushed LIMIT 所需的键数。
    fn limitReached(&self) -> bool {
        self.pushed_limit
            .as_ref()
            .is_some_and(|limit| self.scanned_keys >= limit.offset.saturating_add(limit.count))
    }
}

/// 多个 SelectResult 的列表包装。
pub struct selectResultList {
    pub results: Vec<selectResultWithMeta>,
}
/// 从一组 SelectResult 构造列表。
pub fn newSelectResultList(results: Vec<Box<dyn SelectResult>>) -> selectResultList {
    selectResultList {
        results: results
            .into_iter()
            .map(|result| selectResultWithMeta {
                result: Some(result),
                buffered: VecDeque::new(),
            })
            .collect(),
    }
}
/// 带类型元数据的构造入口（类型参数当前未使用）。
pub fn newSelectResultRowIterList(
    results: Vec<Box<dyn SelectResult>>,
    _types: Vec<Vec<usize>>,
) -> Result<selectResultList, DistSqlError> {
    Ok(newSelectResultList(results))
}
impl selectResultList {
    /// 关闭列表内全部结果。
    pub fn Close(&mut self) {
        for result in &mut self.results {
            result.Close();
        }
    }
}

/// 单个 SelectResult 及其行缓冲。
pub struct selectResultWithMeta {
    pub result: Option<Box<dyn SelectResult>>,
    pub buffered: VecDeque<Row>,
}
impl selectResultWithMeta {
    /// 先消费缓冲，再从底层 result 批量填充。
    pub fn Next(&mut self) -> Result<Option<Row>, DistSqlError> {
        if let Some(row) = self.buffered.pop_front() {
            return Ok(Some(row));
        }
        let Some(result) = self.result.as_mut() else {
            return Ok(None);
        };
        self.buffered.extend(result.next(128)?);
        Ok(self.buffered.pop_front())
    }
    /// 关闭底层 SelectResult 并清空缓冲。
    pub fn Close(&mut self) {
        if let Some(mut result) = self.result.take() {
            let _ = result.close();
        }
        self.buffered.clear();
    }
}
/// 惰性构造下一个 SelectResult 的回调（done 表示结束）。
pub type nextSelectResultBuilder =
    Box<dyn FnMut() -> Result<(selectResultWithMeta, bool), DistSqlError> + Send>;

#[derive(Clone, Debug, Default)]
/// 从索引批次提取的派发数据。
pub struct extractedLookupTaskData {
    pub completed_rows: Vec<Row>,
    pub handles: Vec<Handle>,
    pub exhausted: bool,
    pub index_rows: Vec<Row>,
}

/// 执行回表任务，错误写入 task.error。
pub fn execTableTask<B: DistSqlBackend>(worker: &mut tableWorker<B>, task: &mut lookupTableTask) {
    if let Err(error) = worker.executeTask(task) {
        task.error = Some(error);
    }
}

/// 回表 worker：按 handles 扫表，可选校验与保序。
pub struct tableWorker<B: DistSqlBackend> {
    pub backend: Arc<B>,
    pub table_id: i64,
    pub request: Request,
    pub handle_idx: Vec<usize>,
    pub common_handle: bool,
    pub keep_order: bool,
    pub weak_consistency: bool,
    pub check_index_value: bool,
    pub stats: Arc<Mutex<IndexLookUpRunTimeStats>>,
}
impl<B: DistSqlBackend> tableWorker<B> {
    /// 校验索引行与表行数量/checksum；不一致时 report 缺失 handles。
    pub fn compareData(&self, task: &lookupTableTask, rows: &[Row]) -> Result<(), DistSqlError> {
        if task.index_rows.len() != rows.len() {
            let mut obtained = rows
                .iter()
                .map(|row| {
                    self.backend
                        .row_handle(row, &self.handle_idx, self.common_handle)
                })
                .collect::<Result<BTreeSet<_>, _>>()?;
            let obtained_for_report = obtained.iter().cloned().collect::<Vec<_>>();
            let missing = GetLackHandles(&task.handles, &mut obtained);
            self.backend
                .report_inconsistency(&task.handles, &obtained_for_report, &missing)?;
            return Err(DistSqlError::Inconsistent {
                expected: task.handles.len(),
                obtained: rows.len(),
                missing,
            });
        }
        if self.check_index_value {
            for (index, row) in rows.iter().enumerate() {
                if self.backend.checksum(row)? != self.backend.checksum(&task.index_rows[index])? {
                    return Err(DistSqlError::Inconsistent {
                        expected: task.handles.len(),
                        obtained: rows.len(),
                        missing: Vec::new(),
                    });
                }
            }
        }
        Ok(())
    }
    /// 回表、可选校验、keep_order 排序，并检查弱一致下的行数差异。
    pub fn executeTask(&mut self, task: &mut lookupTableTask) -> Result<(), DistSqlError> {
        let started = Instant::now();
        let mut rows = self
            .backend
            .table_scan(self.table_id, &task.handles, &self.request)?;
        if self.check_index_value {
            self.compareData(task, &rows)?;
        }
        // 按原 handles 顺序重排表行，保证与索引扫描序一致。
        if self.keep_order {
            let mut order = BTreeMap::new();
            for (index, handle) in task.handles.iter().enumerate() {
                order.entry(handle.clone()).or_insert(index);
            }
            rows.sort_by_key(|row| {
                self.backend
                    .row_handle(row, &self.handle_idx, self.common_handle)
                    .ok()
                    .and_then(|handle| order.get(&handle).copied())
                    .unwrap_or(usize::MAX)
            });
        }
        // 强一致下回表行数必须等于 handles；弱一致允许短暂可见性差异。
        if rows.len() != task.handles.len() && !self.weak_consistency {
            let mut obtained = rows
                .iter()
                .map(|row| {
                    self.backend
                        .row_handle(row, &self.handle_idx, self.common_handle)
                })
                .collect::<Result<BTreeSet<_>, _>>()?;
            let obtained_for_report = obtained.iter().cloned().collect::<Vec<_>>();
            let missing = GetLackHandles(&task.handles, &mut obtained);
            self.backend
                .report_inconsistency(&task.handles, &obtained_for_report, &missing)?;
            return Err(DistSqlError::Inconsistent {
                expected: task.handles.len(),
                obtained: rows.len(),
                missing,
            });
        }
        task.rows = rows;
        task.build_done_time = Some(Instant::now());
        task.mem_usage = task.rows.iter().map(Vec::len).sum();
        let mut stats = self.stats.lock().expect("runtime stats poisoned");
        stats.table_task_num += 1;
        stats.table_task_total += started.elapsed();
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
/// IndexLookUp 运行时耗时与计数，供 explain analyze 展示。
pub struct IndexLookUpRunTimeStats {
    pub fetch_handle_total: Duration,
    pub fetch_handle: u64,
    pub task_wait: Duration,
    pub table_task_total: Duration,
    pub table_task_num: u64,
    pub next_wait_index: Duration,
    pub next_wait_table: Duration,
}
impl IndexLookUpRunTimeStats {
    /// 格式化为 explain 可读字符串。
    pub fn String(&self) -> String {
        let mut parts = Vec::new();
        if self.fetch_handle > 0 {
            parts.push(format!(
                "index_task: {{total_time: {:?}, fetch_handle: {}}}",
                self.fetch_handle_total, self.fetch_handle
            ));
        }
        if self.table_task_num > 0 {
            parts.push(format!(
                "table_task: {{total_time: {:?}, num: {}}}",
                self.table_task_total, self.table_task_num
            ));
        }
        if self.next_wait_index > Duration::ZERO || self.next_wait_table > Duration::ZERO {
            parts.push(format!(
                "next: {{wait_index: {:?}, wait_table_lookup: {:?}}}",
                self.next_wait_index, self.next_wait_table
            ));
        }
        parts.join(", ")
    }
    /// 显式 Clone（对齐 Go 方法名）。
    pub fn Clone(&self) -> Self {
        std::clone::Clone::clone(self)
    }
    /// 累加合并另一份统计。
    pub fn Merge(&mut self, other: &Self) {
        self.fetch_handle_total += other.fetch_handle_total;
        self.fetch_handle += other.fetch_handle;
        self.task_wait += other.task_wait;
        self.table_task_total += other.table_task_total;
        self.table_task_num += other.table_task_num;
        self.next_wait_index += other.next_wait_index;
        self.next_wait_table += other.next_wait_table;
    }
    /// 统计类型标识（对齐 Go）。
    pub fn Tp(&self) -> i32 {
        1
    }
}

/// 复制一行为 Datum 向量（fields 参数保留兼容）。
pub fn getDatumRow(row: &[Datum], fields: &[usize]) -> Row {
    row.iter().take(fields.len()).cloned().collect()
}
/// 计算 expected 相对 obtained 的缺失 handle 列表。
pub fn GetLackHandles(expected: &[Handle], obtained: &mut BTreeSet<Handle>) -> Vec<Handle> {
    let mut missing = Vec::with_capacity(expected.len().saturating_sub(obtained.len()));
    for handle in expected {
        if !obtained.remove(handle) {
            missing.push(handle.clone());
        }
    }
    missing
}
/// 复制物理计划 id 列表。
pub fn getPhysicalPlanIDs(plans: &[i32]) -> Vec<i32> {
    plans.to_vec()
}
/// 有排序项且 range 数大于 1 时需要 merge sort。
pub fn needMergeSort(by_items: &[ByItem], range_count: usize) -> bool {
    !by_items.is_empty() && range_count > 1
}

/// 按 ByItem 比较对行原地排序；比较失败时保留首个错误。
fn sort_rows<B: DistSqlBackend>(
    backend: &B,
    rows: &mut [Row],
    by: &[ByItem],
) -> Result<(), DistSqlError> {
    let mut error = None;
    rows.sort_by(|left, right| match backend.compare_rows(left, right, by) {
        Ok(order) => order,
        Err(cause) => {
            error = Some(cause);
            Ordering::Equal
        }
    });
    error.map_or(Ok(()), Err)
}
/// 把 worker join 的 panic payload 转为 DistSqlError。
fn panic_error(payload: Box<dyn std::any::Any + Send>) -> DistSqlError {
    if let Some(message) = payload.downcast_ref::<String>() {
        DistSqlError::WorkerPanic(message.clone())
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        DistSqlError::WorkerPanic((*message).into())
    } else {
        DistSqlError::WorkerPanic("unknown worker panic".into())
    }
}
