// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// IndexMerge 读执行器：多路索引/表扫描合并后回表取行。
//
// 对应 Go 的 `IndexMergeReaderExecutor`。工作流为：各 partial worker 扫描部分路径
// 产出 handle（行定位键），process worker 做并集或交集去重，table-scan worker
// 按 handle 批量回表读行。支持有序合并、下推 LIMIT、分区表与全局索引等分支。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::any::Any;
use std::backtrace::Backtrace;
use std::collections::{HashMap, HashSet};
use std::fmt::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use astersql_errors as errors;
use astersql_util_chunk as chunk;

/// 部分索引扫描 worker 的运行时统计/日志类型名。
pub const partialIndexWorkerType: &str = "IndexMergePartialIndexWorker";
/// 部分表扫描 worker 的运行时统计/日志类型名。
pub const partialTableWorkerType: &str = "IndexMergePartialTableWorker";
/// handle 合并（并集/交集）process worker 的类型名。
pub const processWorkerType: &str = "IndexMergeProcessWorker";
/// 分区表交集 worker 的类型名。
pub const partTblIntersectionWorkerType: &str = "IndexMergePartTblIntersectionWorker";
/// 按 handle 回表的 table-scan worker 类型名。
pub const tableScanWorkerType: &str = "IndexMergeTableScanWorker";
/// 测试钩子：process 完成后可注入取消回调。
pub static IndexMergeCancelFuncForTest: Mutex<Option<fn()>> = Mutex::new(None);

/// 规划器注入的不透明句柄，承载具体 KV/表达式实现细节。
pub type IndexMergeOpaque = Arc<dyn Any + Send + Sync>;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
/// 一行定位信息：分区 ID、编码后的 handle 以及有序合并用的 order keys。
pub struct IndexMergeHandle {
    pub partition_id: i64,
    pub encoded: Vec<u8>,
    pub order_keys: Vec<Vec<u8>>,
}

impl IndexMergeHandle {
    /// 估算本 handle 占用的堆内存字节数。
    fn MemUsage(&self) -> i64 {
        (self.encoded.capacity()
            + self.order_keys.iter().map(Vec::capacity).sum::<usize>()
            + std::mem::size_of::<Self>()) as i64
    }
}

#[derive(Clone)]
/// 一条 partial 扫描路径的计划摘要（索引或表、相关列标记）。
pub struct IndexMergePartialPlan {
    pub id: i32,
    pub is_index: bool,
    pub correlated_filter: bool,
    pub correlated_access: bool,
    pub opaque: IndexMergeOpaque,
}

#[derive(Clone)]
/// 某一物理表上的一组扫描 range，opaque 持有具体范围实现。
pub struct IndexMergeRangeGroup {
    pub physical_table_id: i64,
    pub opaque: IndexMergeOpaque,
}

#[derive(Clone, Copy)]
/// 下推到 IndexMerge 的 LIMIT：先跳过 `offset` 行再取 `count` 行。
pub struct PushedDownLimit {
    pub offset: u64,
    pub count: u64,
}

/// 可取消的异步扫描源；关闭执行器时调用 Cancel 打断阻塞读。
pub trait IndexMergeCancellation: Send + Sync {
    fn Cancel(&self);
}

/// partial worker 一次产出的 handle 批次及其分区下标。
pub struct IndexMergePartialBatch {
    pub handles: Vec<IndexMergeHandle>,
    pub partition_index: usize,
}

/// 部分路径（索引或表）的 handle 数据源。
pub trait IndexMergePartialSource: Send {
    fn Cancellation(&self) -> Arc<dyn IndexMergeCancellation>;
    fn NextHandles(
        &mut self,
        batch_size: usize,
    ) -> Result<Option<IndexMergePartialBatch>, errors::SharedError>;
    fn Close(&mut self) -> Result<(), errors::SharedError>;
}

/// 按 handle 批量回表读行的表读取器。
pub trait IndexMergeTableReader: Send {
    fn Cancellation(&self) -> Arc<dyn IndexMergeCancellation>;
    fn Next(&mut self, output: &mut chunk::Chunk) -> Result<(), errors::SharedError>;
    fn NewChunk(&mut self) -> chunk::Chunk;
    fn Close(&mut self) -> Result<(), errors::SharedError>;
}

/// All planner, KV, expression, table and session operations are mandatory.
///
/// IndexMerge 运行时上下文：构建 partial/表读取器、重排有序结果、挂接内存跟踪与统计。
pub trait IndexMergeRuntimeContext: Send + Sync {
    fn TableID(&self) -> i64;
    fn RebuildRangeForCorCol(
        &self,
        path: usize,
        plan: &IndexMergePartialPlan,
    ) -> Result<(), errors::SharedError>;
    fn BuildPartialWorkerKVRanges(
        &self,
        plans: &[IndexMergePartialPlan],
    ) -> Result<Vec<Vec<IndexMergeRangeGroup>>, errors::SharedError>;
    fn BuildPartialSource(
        &self,
        path: usize,
        plan: &IndexMergePartialPlan,
        ranges: &[IndexMergeRangeGroup],
        keep_order: bool,
    ) -> Result<Box<dyn IndexMergePartialSource>, errors::SharedError>;
    fn BuildFinalTableReader(
        &self,
        partition_id: Option<i64>,
        handles: &[IndexMergeHandle],
    ) -> Result<Box<dyn IndexMergeTableReader>, errors::SharedError>;
    fn ReorderFinalRows(
        &self,
        handles: &[IndexMergeHandle],
        chunks: Vec<chunk::Chunk>,
    ) -> Result<Vec<chunk::Chunk>, errors::SharedError>;
    fn NewOutputChunk(&self) -> chunk::Chunk;
    fn ValidatePartitionHandle(
        &self,
        path: usize,
        is_index: bool,
    ) -> Result<bool, errors::SharedError>;
    fn IndexLookupConcurrency(&self) -> usize;
    fn IntersectionConcurrency(&self) -> usize;
    fn LookupTaskChannelSize(&self) -> usize;
    fn IndexLookupSize(&self) -> usize;
    fn MaxChunkSize(&self) -> usize;
    fn ValidateFinalRowCount(&self) -> bool;
    fn HasRuntimeStats(&self) -> bool;
    fn RuntimeStatsType(&self) -> i32;
    fn AttachMemoryTracker(&self, tracker: Arc<IndexMergeMemoryTracker>);
    fn DetachMemoryTracker(&self, tracker: Arc<IndexMergeMemoryTracker>);
    fn RegisterRuntimeStats(&self, stats: IndexMergeRuntimeStat);
    fn ReportIndexUsage(&self, plan: &IndexMergePartialPlan);
    fn LogCloseError(&self, worker: &str, error: &errors::SharedError);
    fn TriggerFailpoint(&self, name: &str);
}

#[derive(Default)]
/// 原子累计 IndexMerge 路径上的内存占用，供会话内存跟踪器使用。
pub struct IndexMergeMemoryTracker {
    bytes: AtomicI64,
}

impl IndexMergeMemoryTracker {
    /// 增减已消费字节数（可为负以释放）。
    fn Consume(&self, amount: i64) {
        self.bytes.fetch_add(amount, Ordering::AcqRel);
    }

    /// 当前累计已消费字节。
    pub fn BytesConsumed(&self) -> i64 {
        self.bytes.load(Ordering::Acquire)
    }
}

#[derive(Default)]
/// 执行结束信号：Finish 后各发送/接收循环可退出。
struct finishSignal {
    done: AtomicBool,
    lock: Mutex<()>,
    changed: Condvar,
}

impl finishSignal {
    /// 标记完成并唤醒等待方。
    fn Finish(&self) {
        self.done.store(true, Ordering::Release);
        self.changed.notify_all();
    }
    /// 是否已收到结束信号。
    fn IsFinished(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }
    /// 短暂等待结束信号变化，避免忙等。
    fn Wait(&self) {
        let guard = self.lock.lock().expect("index merge finish lock poisoned");
        let _wait = self
            .changed
            .wait_timeout(guard, Duration::from_millis(2))
            .expect("index merge finish wait poisoned");
    }
}

/// 在通道满时等待 finish；返回 true 表示已结束或通道断开而未送出。
fn sendUntilFinished<T>(sender: &SyncSender<T>, finish: &finishSignal, mut value: T) -> bool {
    loop {
        if finish.IsFinished() {
            return true;
        }
        match sender.try_send(value) {
            Ok(()) => return false,
            Err(TrySendError::Full(returned)) => {
                value = returned;
                finish.Wait();
            }
            Err(TrySendError::Disconnected(_)) => return true,
        }
    }
}

/// 带超时轮询接收，直到拿到消息、结束或通道断开。
fn receiveUntilFinished<T>(receiver: &Arc<Mutex<Receiver<T>>>, finish: &finishSignal) -> Option<T> {
    loop {
        if finish.IsFinished() {
            return None;
        }
        match receiver
            .lock()
            .expect("index merge receiver lock poisoned")
            .recv_timeout(Duration::from_millis(10))
        {
            Ok(value) => return Some(value),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

/// 单个回表任务的完成状态：结果 chunk 游标、错误与内存占用。
struct taskState {
    chunks: Vec<Box<chunk::Chunk>>,
    chunkCursor: usize,
    rowCursor: usize,
    error: Option<errors::SharedError>,
    completed: bool,
    memUsage: i64,
}

/// 用条件变量同步 taskState 的完成通知。
struct taskCompletion {
    state: Mutex<taskState>,
    changed: Condvar,
}

/// 一批待回表的 handle 及其所属分区/partial 路径；完成后填充行 chunk。
pub struct indexMergeTableTask {
    pub handles: Vec<IndexMergeHandle>,
    pub parTblIdx: usize,
    pub partialPlanID: usize,
    completion: taskCompletion,
}

impl indexMergeTableTask {
    /// 创建未完成的回表任务。
    fn new(handles: Vec<IndexMergeHandle>, parTblIdx: usize, partialPlanID: usize) -> Self {
        Self {
            handles,
            parTblIdx,
            partialPlanID,
            completion: taskCompletion {
                state: Mutex::new(taskState {
                    chunks: Vec::new(),
                    chunkCursor: 0,
                    rowCursor: 0,
                    error: None,
                    completed: false,
                    memUsage: 0,
                }),
                changed: Condvar::new(),
            },
        }
    }

    /// 构造已携带错误的完成任务，用于跨通道传播失败。
    fn error(error: errors::SharedError) -> Self {
        let task = Self::new(Vec::new(), 0, 0);
        task.Complete(Vec::new(), Some(error), 0);
        task
    }

    /// 写入结果 chunk 或错误并唤醒 Wait。
    fn Complete(
        &self,
        chunks: Vec<chunk::Chunk>,
        error: Option<errors::SharedError>,
        memUsage: i64,
    ) {
        let mut state = self
            .completion
            .state
            .lock()
            .expect("index merge task poisoned");
        state.chunks = chunks.into_iter().map(Box::new).collect();
        state.error = error;
        state.memUsage = memUsage;
        state.completed = true;
        self.completion.changed.notify_all();
    }

    /// 阻塞直到任务完成；若执行器已结束则返回中断错误。
    fn Wait(&self, finish: &finishSignal) -> Result<(), errors::SharedError> {
        let mut state = self
            .completion
            .state
            .lock()
            .expect("index merge task poisoned");
        while !state.completed {
            if finish.IsFinished() {
                return Err(errors::New("index merge task interrupted"));
            }
            let (next, _) = self
                .completion
                .changed
                .wait_timeout(state, Duration::from_millis(10))
                .expect("index merge task wait poisoned");
            state = next;
        }
        match state.error.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// partial → process 通道上的消息：任务或全部 partial 结束。
enum fetchMessage {
    Task(Arc<indexMergeTableTask>),
    Finished,
}

/// process → 主线程结果通道：可消费的回表任务或全部结束。
enum resultMessage {
    Task(Arc<indexMergeTableTask>),
    Finished,
}

/// 各 worker 共享的执行状态：计划、通道、结束信号、内存跟踪与取消列表。
struct indexMergeShared {
    ctx: Arc<dyn IndexMergeRuntimeContext>,
    plans: Vec<IndexMergePartialPlan>,
    ranges: Vec<Vec<IndexMergeRangeGroup>>,
    keepOrder: bool,
    byItemsDesc: Vec<bool>,
    pushedLimit: Option<PushedDownLimit>,
    isIntersection: bool,
    hasGlobalIndex: bool,
    partitionTableMode: bool,
    finish: Arc<finishSignal>,
    tracker: Arc<IndexMergeMemoryTracker>,
    stats: Option<Arc<IndexMergeRuntimeStat>>,
    fetchSender: SyncSender<fetchMessage>,
    fetchReceiver: Arc<Mutex<Receiver<fetchMessage>>>,
    workSender: SyncSender<Arc<indexMergeTableTask>>,
    workReceiver: Arc<Mutex<Receiver<Arc<indexMergeTableTask>>>>,
    workFinished: AtomicBool,
    resultSender: SyncSender<resultMessage>,
    cancellations: Mutex<Vec<Arc<dyn IndexMergeCancellation>>>,
}

/// IndexMerge 执行器：Open 建通道，Next 拉回表结果，Close 取消并回收 worker。
pub struct IndexMergeReaderExecutor {
    ctx: Arc<dyn IndexMergeRuntimeContext>,
    partialPlans: Vec<IndexMergePartialPlan>,
    partialWorkerKVRanges: Vec<Vec<IndexMergeRangeGroup>>,
    keepOrder: bool,
    pushedLimit: Option<PushedDownLimit>,
    byItemsDesc: Vec<bool>,
    partitionTableMode: bool,
    isIntersection: bool,
    hasGlobalIndex: bool,
    workerStarted: bool,
    resultReceiver: Option<Receiver<resultMessage>>,
    resultCurr: Option<Arc<indexMergeTableTask>>,
    memTracker: Option<Arc<IndexMergeMemoryTracker>>,
    stats: Option<Arc<IndexMergeRuntimeStat>>,
    shared: Option<Arc<indexMergeShared>>,
    coordinator: Option<JoinHandle<()>>,
}

impl IndexMergeReaderExecutor {
    #[allow(clippy::too_many_arguments)]
    /// 构造执行器；`partialPlans` 不可为空。
    pub fn new(
        ctx: Arc<dyn IndexMergeRuntimeContext>,
        partialPlans: Vec<IndexMergePartialPlan>,
        keepOrder: bool,
        pushedLimit: Option<PushedDownLimit>,
        byItemsDesc: Vec<bool>,
        partitionTableMode: bool,
        isIntersection: bool,
        hasGlobalIndex: bool,
    ) -> Self {
        assert!(!partialPlans.is_empty(), "index merge needs a partial plan");
        Self {
            ctx,
            partialPlans,
            partialWorkerKVRanges: Vec::new(),
            keepOrder,
            pushedLimit,
            byItemsDesc,
            partitionTableMode,
            isIntersection,
            hasGlobalIndex,
            workerStarted: false,
            resultReceiver: None,
            resultCurr: None,
            memTracker: None,
            stats: None,
            shared: None,
            coordinator: None,
        }
    }

    /// 目标逻辑表 ID。
    pub fn Table(&self) -> i64 {
        self.ctx.TableID()
    }

    /// 初始化统计、重建相关列 range、构建 KV ranges 并创建共享通道状态。
    pub fn Open(&mut self) -> Result<(), errors::SharedError> {
        self.initRuntimeStats();
        self.rebuildRangeForCorCol()?;
        self.buildPartialWorkerKVRanges()?;
        let tracker = Arc::new(IndexMergeMemoryTracker::default());
        self.ctx.AttachMemoryTracker(Arc::clone(&tracker));
        let capacity = self.ctx.LookupTaskChannelSize().max(1);
        let (fetchSender, fetchReceiver) = mpsc::sync_channel(capacity);
        let (workSender, workReceiver) = mpsc::sync_channel(1);
        let (resultSender, resultReceiver) = mpsc::sync_channel(capacity);
        self.shared = Some(Arc::new(indexMergeShared {
            ctx: Arc::clone(&self.ctx),
            plans: self.partialPlans.clone(),
            ranges: self.partialWorkerKVRanges.clone(),
            keepOrder: self.keepOrder,
            byItemsDesc: self.byItemsDesc.clone(),
            pushedLimit: self.pushedLimit,
            isIntersection: self.isIntersection,
            hasGlobalIndex: self.hasGlobalIndex,
            partitionTableMode: self.partitionTableMode,
            finish: Arc::new(finishSignal::default()),
            tracker: Arc::clone(&tracker),
            stats: self.stats.clone(),
            fetchSender,
            fetchReceiver: Arc::new(Mutex::new(fetchReceiver)),
            workSender,
            workReceiver: Arc::new(Mutex::new(workReceiver)),
            workFinished: AtomicBool::new(false),
            resultSender,
            cancellations: Mutex::new(Vec::new()),
        }));
        self.resultReceiver = Some(resultReceiver);
        self.resultCurr = None;
        self.memTracker = Some(tracker);
        self.workerStarted = false;
        Ok(())
    }

    /// 对带相关列访问的 partial 路径重建扫描 range。
    fn rebuildRangeForCorCol(&self) -> Result<(), errors::SharedError> {
        for (path, plan) in self.partialPlans.iter().enumerate() {
            if plan.correlated_access {
                self.ctx.RebuildRangeForCorCol(path, plan)?;
            }
        }
        Ok(())
    }

    /// 为每条 partial 路径构建 KV range 组，长度必须与计划一致。
    fn buildPartialWorkerKVRanges(&mut self) -> Result<(), errors::SharedError> {
        self.partialWorkerKVRanges = self.ctx.BuildPartialWorkerKVRanges(&self.partialPlans)?;
        if self.partialWorkerKVRanges.len() != self.partialPlans.len() {
            return Err(errors::New(
                "partial plans and KV ranges have different lengths",
            ));
        }
        Ok(())
    }

    /// 启动 partial、process、table-scan worker 及协调线程；交集+有序不支持。
    fn startWorkers(&mut self) -> Result<(), errors::SharedError> {
        let shared = Arc::clone(self.shared.as_ref().expect("index merge must be opened"));
        if shared.isIntersection && shared.keepOrder {
            return Err(errors::New("intersection with keepOrder is not supported"));
        }
        // 先为每条路径构建数据源并登记取消句柄。
        let mut sources = Vec::with_capacity(shared.plans.len());
        for path in 0..shared.plans.len() {
            let source = shared.ctx.BuildPartialSource(
                path,
                &shared.plans[path],
                &shared.ranges[path],
                shared.keepOrder,
            )?;
            shared
                .cancellations
                .lock()
                .expect("index merge cancellation list poisoned")
                .push(source.Cancellation());
            sources.push(source);
        }

        // 每条 partial 路径一个线程：索引或表扫描产出 handle。
        let mut partialHandles = Vec::with_capacity(sources.len());
        for (path, source) in sources.into_iter().enumerate() {
            let workerShared = Arc::clone(&shared);
            partialHandles.push(thread::spawn(move || {
                let worker = || {
                    if workerShared.plans[path].is_index {
                        Self::startPartialIndexWorker(workerShared.clone(), path, source);
                    } else {
                        Self::startPartialTableWorker(workerShared.clone(), path, source);
                    }
                };
                if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(worker))
                {
                    handleWorkerPanic(&workerShared, payload, partialIndexWorkerType);
                }
            }));
        }

        // process worker：并集/交集合并 handle，结束后标记 workFinished。
        let processShared = Arc::clone(&shared);
        let processHandle = thread::spawn(move || {
            let worker = || Self::startIndexMergeProcessWorker(processShared.clone());
            if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(worker)) {
                handleWorkerPanic(&processShared, payload, processWorkerType);
            }
            processShared.workFinished.store(true, Ordering::Release);
        });

        let mut tableHandles = Vec::new();
        self.startIndexMergeTableScanWorker(&mut tableHandles);
        // 协调线程：等待 partial → 关闭 fetch 通道 → 等待 process 与回表 worker。
        self.coordinator = Some(thread::spawn(move || {
            Self::waitPartialWorkersAndCloseFetchChan(&shared, partialHandles);
            let _ = processHandle.join();
            for worker in tableHandles {
                let _ = worker.join();
            }
        }));
        self.workerStarted = true;
        Ok(())
    }

    /// 等待全部 partial worker 结束后向 process 发送 Finished。
    fn waitPartialWorkersAndCloseFetchChan(
        shared: &indexMergeShared,
        workers: Vec<JoinHandle<()>>,
    ) {
        for worker in workers {
            let _ = worker.join();
        }
        let _ = sendUntilFinished(&shared.fetchSender, &shared.finish, fetchMessage::Finished);
    }

    /// 按交集/有序并集/普通并集选择合并循环，结束后关闭结果通道。
    fn startIndexMergeProcessWorker(shared: Arc<indexMergeShared>) {
        let worker = indexMergeProcessWorker {
            indexMerge: Arc::clone(&shared),
            stats: shared.stats.clone(),
        };
        if shared.isIntersection {
            worker.fetchLoopIntersection();
        } else if !shared.byItemsDesc.is_empty() {
            worker.fetchLoopUnionWithOrderBy();
        } else {
            worker.fetchLoopUnion();
        }
        let _ = sendUntilFinished(
            &shared.resultSender,
            &shared.finish,
            resultMessage::Finished,
        );
    }

    /// 启动部分索引扫描 worker 并同步错误到 fetch 通道。
    fn startPartialIndexWorker(
        shared: Arc<indexMergeShared>,
        workID: usize,
        source: Box<dyn IndexMergePartialSource>,
    ) {
        shared
            .ctx
            .TriggerFailpoint("testIndexMergePanicPartialIndexWorker");
        let mut worker = partialIndexWorker::new(&shared, workID, source);
        if let Err(error) = worker.fetchHandles() {
            syncErr(&shared, error);
        }
    }

    /// 启动部分表扫描 worker 并同步错误到 fetch 通道。
    fn startPartialTableWorker(
        shared: Arc<indexMergeShared>,
        workID: usize,
        source: Box<dyn IndexMergePartialSource>,
    ) {
        shared
            .ctx
            .TriggerFailpoint("testIndexMergePanicPartialTableWorker");
        let mut worker = partialTableWorker::new(&shared, workID, source);
        if let Err(error) = worker.fetchHandles() {
            syncErr(&shared, error);
        }
    }

    /// 若会话启用运行时统计则创建 IndexMergeRuntimeStat。
    fn initRuntimeStats(&mut self) {
        self.stats = self.ctx.HasRuntimeStats().then(|| {
            Arc::new(IndexMergeRuntimeStat::new(
                self.ctx.IndexLookupConcurrency(),
                self.ctx.RuntimeStatsType(),
            ))
        });
    }

    /// 返回指定 partial worker 对应计划 ID。
    fn getPartitalPlanID(&self, workID: usize) -> i32 {
        self.partialPlans.get(workID).map_or(0, |plan| plan.id)
    }

    /// 返回 partial 计划列表末项 ID（常作表侧根计划）。
    fn getTablePlanRootID(&self) -> i32 {
        self.partialPlans.last().map_or(0, |plan| plan.id)
    }

    /// 按 IndexLookupConcurrency 启动回表 worker 池。
    fn startIndexMergeTableScanWorker(&self, handles: &mut Vec<JoinHandle<()>>) {
        let shared = Arc::clone(self.shared.as_ref().expect("index merge must be opened"));
        for _ in 0..shared.ctx.IndexLookupConcurrency().max(1) {
            let workerShared = Arc::clone(&shared);
            handles.push(thread::spawn(move || {
                let mut worker = indexMergeTableScanWorker {
                    stats: workerShared.stats.clone(),
                    indexMergeExec: Arc::clone(&workerShared),
                    memTracker: Arc::clone(&workerShared.tracker),
                    current: Mutex::new(None),
                };
                let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    worker.pickAndExecTask();
                }));
                if let Err(payload) = panic {
                    worker.handleTableScanWorkerPanic(payload, tableScanWorkerType);
                }
            }));
        }
    }

    /// 委托上下文按 handle 批次构建最终表读取器。
    fn buildFinalTableReader(
        shared: &indexMergeShared,
        partition: Option<i64>,
        handles: &[IndexMergeHandle],
    ) -> Result<Box<dyn IndexMergeTableReader>, errors::SharedError> {
        shared.ctx.BuildFinalTableReader(partition, handles)
    }

    /// 拉取下一结果块：惰性启动 worker，从 result 通道消费已完成回表任务。
    pub fn Next(&mut self, req: &mut chunk::Chunk) -> Result<(), errors::SharedError> {
        if !self.workerStarted {
            self.startWorkers()?;
        }
        req.Reset();
        while req.NumRows() < self.ctx.MaxChunkSize() {
            let Some(task) = self.getResultTask()? else {
                return Ok(());
            };
            let mut state = task
                .completion
                .state
                .lock()
                .expect("index merge task poisoned");
            // 从当前任务的 chunk/行游标拷贝到输出，填满 MaxChunkSize 即返回。
            while state.chunkCursor < state.chunks.len() {
                let chunkIndex = state.chunkCursor;
                let rowCount = state.chunks[chunkIndex].NumRows();
                while state.rowCursor < rowCount && req.NumRows() < self.ctx.MaxChunkSize() {
                    req.AppendRow(state.chunks[chunkIndex].GetRow(state.rowCursor));
                    state.rowCursor += 1;
                }
                if state.rowCursor == rowCount {
                    state.chunkCursor += 1;
                    state.rowCursor = 0;
                }
                if req.NumRows() >= self.ctx.MaxChunkSize() {
                    return Ok(());
                }
            }
            drop(state);
            let mut state = task
                .completion
                .state
                .lock()
                .expect("index merge task poisoned");
            // 任务读完后释放其登记的内存占用。
            self.shared
                .as_ref()
                .expect("index merge shared state missing")
                .tracker
                .Consume(-state.memUsage);
            state.memUsage = 0;
            drop(state);
            self.resultCurr = None;
        }
        Ok(())
    }

    /// 取得仍有未读行的当前任务，或阻塞接收下一个已完成任务。
    fn getResultTask(&mut self) -> Result<Option<Arc<indexMergeTableTask>>, errors::SharedError> {
        if let Some(task) = &self.resultCurr {
            let state = task
                .completion
                .state
                .lock()
                .expect("index merge task poisoned");
            if state.chunkCursor < state.chunks.len() {
                return Ok(self.resultCurr.clone());
            }
        }
        let shared = self
            .shared
            .as_ref()
            .expect("index merge shared state missing");
        let message = self
            .resultReceiver
            .as_ref()
            .expect("index merge result receiver missing")
            .recv()
            .map_err(|_| errors::New("index merge result channel disconnected"))?;
        match message {
            resultMessage::Finished => Ok(None),
            resultMessage::Task(task) => {
                task.Wait(&shared.finish)?;
                self.resultCurr = Some(task.clone());
                Ok(Some(task))
            }
        }
    }

    /// 发结束信号、取消所有源、等待协调线程并登记统计/卸下内存跟踪。
    pub fn Close(&mut self) -> Result<(), errors::SharedError> {
        if let Some(shared) = &self.shared {
            shared.finish.Finish();
            let cancellations = shared
                .cancellations
                .lock()
                .expect("index merge cancellation list poisoned")
                .clone();
            for cancellation in cancellations {
                cancellation.Cancel();
            }
        }
        if let Some(coordinator) = self.coordinator.take() {
            let _ = coordinator.join();
        }
        for plan in &self.partialPlans {
            self.ctx.ReportIndexUsage(plan);
        }
        if let Some(stats) = &self.stats {
            self.ctx.RegisterRuntimeStats(stats.Clone());
        }
        if let Some(tracker) = self.memTracker.take() {
            self.ctx.DetachMemoryTracker(tracker);
        }
        self.workerStarted = false;
        self.resultReceiver = None;
        self.resultCurr = None;
        self.shared = None;
        Ok(())
    }
}

/// 从部分表路径拉取 handle 并发送到 fetch 通道的 worker。
pub struct partialTableWorker {
    shared: Arc<indexMergeShared>,
    workID: usize,
    source: Box<dyn IndexMergePartialSource>,
    batchSize: usize,
    maxBatchSize: usize,
    scannedKeys: u64,
}

impl partialTableWorker {
    fn new(
        shared: &Arc<indexMergeShared>,
        workID: usize,
        source: Box<dyn IndexMergePartialSource>,
    ) -> Self {
        Self {
            shared: Arc::clone(shared),
            workID,
            source,
            batchSize: shared.ctx.MaxChunkSize(),
            maxBatchSize: shared.ctx.IndexLookupSize(),
            scannedKeys: 0,
        }
    }
    /// 校验该路径是否需要附带分区 handle。
    fn needPartitionHandle(&self) -> Result<bool, errors::SharedError> {
        self.shared.ctx.ValidatePartitionHandle(self.workID, false)
    }
    /// 循环提取任务并投递；退出时关闭 source 并返回累计 handle 数。
    fn fetchHandles(&mut self) -> Result<i64, errors::SharedError> {
        let result = (|| {
            let _ = self.needPartitionHandle()?;
            let mut count = 0;
            loop {
                let start = Instant::now();
                let Some(task) = self.extractTaskHandles()? else {
                    break;
                };
                if let Some(stats) = &self.shared.stats {
                    stats
                        .FetchIdxTime
                        .fetch_add(start.elapsed().as_nanos() as i64, Ordering::AcqRel);
                }
                count += task.handles.len() as i64;
                if sendUntilFinished(
                    &self.shared.fetchSender,
                    &self.shared.finish,
                    fetchMessage::Task(task),
                ) {
                    break;
                }
            }
            Ok(count)
        })();
        if let Err(error) = self.source.Close() {
            self.shared
                .ctx
                .LogCloseError(partialTableWorkerType, &error);
        }
        result
    }
    /// 表扫描路径返回类型个数占位（依赖是否需要分区 handle）。
    fn getRetTpsForTableScan(&self) -> usize {
        self.needPartitionHandle().map_or(0, usize::from)
    }
    /// 从 source 取一批 handle，非交集时应用 partial LIMIT，并倍增 batchSize。
    fn extractTaskHandles(
        &mut self,
    ) -> Result<Option<Arc<indexMergeTableTask>>, errors::SharedError> {
        let batch = self.source.NextHandles(self.batchSize)?;
        let Some(mut batch) = batch else {
            return Ok(None);
        };
        if !self.shared.isIntersection {
            applyPartialLimit(
                &mut batch.handles,
                self.shared.pushedLimit,
                &mut self.scannedKeys,
            );
        }
        self.batchSize = (self.batchSize * 2).min(self.maxBatchSize);
        if batch.handles.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.buildTableTask(
            batch.handles,
            batch.partition_index,
            self.workID,
        )))
    }
    /// 将 handle 列表封装为回表任务。
    fn buildTableTask(
        &self,
        handles: Vec<IndexMergeHandle>,
        parTblIdx: usize,
        partialPlanID: usize,
    ) -> Arc<indexMergeTableTask> {
        Arc::new(indexMergeTableTask::new(handles, parTblIdx, partialPlanID))
    }
}

/// 部分索引扫描 worker：复用 partialTableWorker 的拉取逻辑。
pub struct partialIndexWorker(partialTableWorker);

impl partialIndexWorker {
    fn new(
        shared: &Arc<indexMergeShared>,
        workID: usize,
        source: Box<dyn IndexMergePartialSource>,
    ) -> Self {
        Self(partialTableWorker::new(shared, workID, source))
    }
    fn needPartitionHandle(&self) -> Result<bool, errors::SharedError> {
        self.0
            .shared
            .ctx
            .ValidatePartitionHandle(self.0.workID, true)
    }
    fn fetchHandles(&mut self) -> Result<i64, errors::SharedError> {
        let _ = self.needPartitionHandle()?;
        self.0.fetchHandles()
    }
    fn getRetTpsForIndexScan(&self) -> usize {
        self.needPartitionHandle().map_or(0, usize::from)
    }
    fn extractTaskHandles(
        &mut self,
    ) -> Result<Option<Arc<indexMergeTableTask>>, errors::SharedError> {
        self.0.extractTaskHandles()
    }
    fn buildTableTask(
        &self,
        handles: Vec<IndexMergeHandle>,
        parTblIdx: usize,
        partialPlanID: usize,
    ) -> Arc<indexMergeTableTask> {
        self.0.buildTableTask(handles, parTblIdx, partialPlanID)
    }
}

/// 按已扫描计数过滤 handle，实现下推 LIMIT 的 offset+count 窗口。
fn applyPartialLimit(
    handles: &mut Vec<IndexMergeHandle>,
    limit: Option<PushedDownLimit>,
    scanned: &mut u64,
) {
    let Some(limit) = limit else { return };
    handles.retain(|_| {
        *scanned += 1;
        *scanned <= limit.offset + limit.count
    });
}

/// 合并各 partial handle：并集去重、有序堆或分区交集，再派发回表任务。
pub struct indexMergeProcessWorker {
    indexMerge: Arc<indexMergeShared>,
    stats: Option<Arc<IndexMergeRuntimeStat>>,
}

#[derive(Clone, Copy)]
/// 堆中一行的定位：属于哪个 partial、哪个任务、任务内第几行。
pub struct rowIdx {
    pub partialID: usize,
    pub taskID: usize,
    pub rowID: usize,
}

/// 有序 IndexMerge 用的最小堆：按 order keys 与 byItems 方向比较。
pub struct handleHeap {
    requiredCnt: u64,
    tracker: Arc<IndexMergeMemoryTracker>,
    taskMap: HashMap<usize, Vec<Arc<indexMergeTableTask>>>,
    idx: Vec<rowIdx>,
    byItems: Vec<bool>,
}

impl handleHeap {
    /// 堆中元素个数。
    pub fn Len(&self) -> usize {
        self.idx.len()
    }
    /// 堆序比较：idx[i] 是否应排在 idx[j] 之前。
    pub fn Less(&self, i: usize, j: usize) -> bool {
        let a =
            &self.taskMap[&self.idx[i].partialID][self.idx[i].taskID].handles[self.idx[i].rowID];
        let b =
            &self.taskMap[&self.idx[j].partialID][self.idx[j].taskID].handles[self.idx[j].rowID];
        compareHandles(a, b, &self.byItems).is_lt()
    }
    /// 交换堆中两个下标。
    pub fn Swap(&mut self, i: usize, j: usize) {
        self.idx.swap(i, j);
    }
    /// 压入一行索引并计入内存。
    pub fn Push(&mut self, value: rowIdx) {
        self.idx.push(value);
        self.tracker.Consume(std::mem::size_of::<rowIdx>() as i64);
    }
    /// 弹出堆顶并释放对应内存记账。
    pub fn Pop(&mut self) -> Option<rowIdx> {
        let value = self.idx.pop();
        if value.is_some() {
            self.tracker
                .Consume(-(std::mem::size_of::<rowIdx>() as i64));
        }
        value
    }
}

/// 按 order_keys 比较两个 handle；`desc[i]` 为 true 时该键降序。
fn compareHandles(a: &IndexMergeHandle, b: &IndexMergeHandle, desc: &[bool]) -> std::cmp::Ordering {
    for (index, (left, right)) in a.order_keys.iter().zip(&b.order_keys).enumerate() {
        let mut order = left.cmp(right);
        if desc.get(index).copied().unwrap_or(false) {
            order = order.reverse();
        }
        if !order.is_eq() {
            return order;
        }
    }
    a.cmp(b)
}

impl indexMergeProcessWorker {
    /// 按下推 LIMIT 设置逻辑保留上限，并限制初始预分配大小。
    fn NewHandleHeap(
        &self,
        taskMap: HashMap<usize, Vec<Arc<indexMergeTableTask>>>,
        tracker: Arc<IndexMergeMemoryTracker>,
    ) -> handleHeap {
        let requiredCnt = self
            .indexMerge
            .pushedLimit
            .map_or(0, |v| v.offset + v.count);
        handleHeap {
            requiredCnt,
            tracker,
            taskMap,
            idx: Vec::with_capacity(requiredCnt.min(1024) as usize),
            byItems: self.indexMerge.byItemsDesc.clone(),
        }
    }
    /// 有序路径要求每个 handle 的 order_keys 数量足够。
    fn pruneTableWorkerTaskIdxRows(
        &self,
        task: &indexMergeTableTask,
    ) -> Result<(), errors::SharedError> {
        if task
            .handles
            .iter()
            .any(|h| h.order_keys.len() < self.indexMerge.byItemsDesc.len())
        {
            return Err(errors::New(
                "ordered index merge task has insufficient order keys",
            ));
        }
        Ok(())
    }
    /// 排空 fetch 通道，等待每个任务完成后再收集。
    fn collectFetch(&self) -> Result<Vec<Arc<indexMergeTableTask>>, errors::SharedError> {
        let mut tasks = Vec::new();
        loop {
            match receiveUntilFinished(&self.indexMerge.fetchReceiver, &self.indexMerge.finish) {
                Some(fetchMessage::Task(task)) => {
                    task.Wait(&self.indexMerge.finish)?;
                    tasks.push(task);
                }
                Some(fetchMessage::Finished) => return Ok(tasks),
                None => return Ok(tasks),
            }
        }
    }
    /// 将任务送入 work 通道，并同时送入 result 通道供主线程等待完成。
    fn dispatch(&self, task: Arc<indexMergeTableTask>) -> bool {
        if sendUntilFinished(
            &self.indexMerge.workSender,
            &self.indexMerge.finish,
            task.clone(),
        ) {
            return true;
        }
        sendUntilFinished(
            &self.indexMerge.resultSender,
            &self.indexMerge.finish,
            resultMessage::Task(task),
        )
    }
    /// 有序并集：收集全部 handle、去重排序、应用 LIMIT 后分批派发。
    fn fetchLoopUnionWithOrderBy(&self) {
        let start = Instant::now();
        let result = (|| {
            let tasks = self.collectFetch()?;
            let mut distinct = HashSet::new();
            let mut handles = Vec::new();
            // 按 parTblIdx 哈希到交集 worker，减少跨分区竞争。
            for task in tasks {
                self.pruneTableWorkerTaskIdxRows(&task)?;
                for handle in &task.handles {
                    if distinct.insert((handle.partition_id, handle.encoded.clone())) {
                        handles.push(handle.clone());
                    }
                }
            }
            // 去重后按 ORDER BY 方向全局排序。
            handles.sort_by(|a, b| compareHandles(a, b, &self.indexMerge.byItemsDesc));
            let (_, handles) = pushedLimitCountingDown(self.indexMerge.pushedLimit, handles);
            self.dispatchHandles(handles);
            notifyCancelHook();
            Ok::<(), errors::SharedError>(())
        })();
        if let Err(error) = result {
            syncResultErr(&self.indexMerge, error);
        }
        if let Some(stats) = &self.stats {
            stats
                .IndexMergeProcess
                .fetch_add(start.elapsed().as_nanos() as i64, Ordering::AcqRel);
        }
    }
    /// 无序并集：按 (partition, encoded) 去重后派发回表。
    fn fetchLoopUnion(&self) {
        let start = Instant::now();
        let result = (|| {
            let tasks = self.collectFetch()?;
            let mut distinct = HashSet::new();
            let mut handles = Vec::new();
            for task in tasks {
                for handle in &task.handles {
                    if distinct.insert((handle.partition_id, handle.encoded.clone())) {
                        self.indexMerge.tracker.Consume(handle.MemUsage());
                        handles.push(handle.clone());
                    }
                }
            }
            let (_, handles) = pushedLimitCountingDown(self.indexMerge.pushedLimit, handles);
            self.dispatchHandles(handles);
            notifyCancelHook();
            Ok::<(), errors::SharedError>(())
        })();
        if let Err(error) = result {
            syncResultErr(&self.indexMerge, error);
        }
        if let Some(stats) = &self.stats {
            stats
                .IndexMergeProcess
                .fetch_add(start.elapsed().as_nanos() as i64, Ordering::AcqRel);
        }
    }
    /// 按 IndexLookupSize 分批；分区表无全局索引时按 partition 归组。
    fn dispatchHandles(&self, handles: Vec<IndexMergeHandle>) {
        let batch = self.indexMerge.ctx.IndexLookupSize().max(1);
        // 分区表 + 局部索引：保持顺序时逐 handle 派发，否则按分区聚批。
        if self.indexMerge.partitionTableMode && !self.indexMerge.hasGlobalIndex {
            if self.indexMerge.keepOrder {
                for handle in handles {
                    let partition = handle.partition_id.max(0) as usize;
                    if self.dispatch(Arc::new(indexMergeTableTask::new(
                        vec![handle],
                        partition,
                        0,
                    ))) {
                        return;
                    }
                }
                return;
            }
            let mut partitions: HashMap<i64, Vec<IndexMergeHandle>> = HashMap::new();
            for handle in handles {
                partitions
                    .entry(handle.partition_id)
                    .or_default()
                    .push(handle);
            }
            for (partition, values) in partitions {
                for values in values.chunks(batch) {
                    if self.dispatch(Arc::new(indexMergeTableTask::new(
                        values.to_vec(),
                        partition.max(0) as usize,
                        0,
                    ))) {
                        return;
                    }
                }
            }
            return;
        }
        for values in handles.chunks(batch) {
            let partition = values.first().map_or(0, |v| v.partition_id.max(0) as usize);
            if self.dispatch(Arc::new(indexMergeTableTask::new(
                values.to_vec(),
                partition,
                0,
            ))) {
                return;
            }
        }
    }
    /// 交集与有序合并不兼容，显式 panic 与 startWorkers 校验一致。
    fn fetchLoopIntersectionWithOrderBy(&self) {
        panic!("intersection with keepOrder is not supported");
    }
    /// 交集：按分区桶并发求各 partial 共有 handle，再 LIMIT 并派发。
    fn fetchLoopIntersection(&self) {
        let start = Instant::now();
        let result = (|| {
            let tasks = self.collectFetch()?;
            let workerCount = self
                .indexMerge
                .ctx
                .IntersectionConcurrency()
                .max(1)
                .min(tasks.len().max(1));
            let mut buckets: Vec<Vec<Arc<indexMergeTableTask>>> =
                (0..workerCount).map(|_| Vec::new()).collect();
            for task in tasks {
                let index = task.parTblIdx % workerCount;
                buckets[index].push(task);
            }
            let (sender, receiver) = mpsc::sync_channel(workerCount);
            let mut workers = Vec::new();
            for (id, tasks) in buckets.into_iter().enumerate() {
                let shared = Arc::clone(&self.indexMerge);
                let sender = sender.clone();
                workers.push(thread::spawn(move || {
                    let mut worker = intersectionProcessWorker::new(id, shared);
                    let values = worker.doIntersectionPerPartition(tasks);
                    let _ = sender.send(values);
                }));
            }
            drop(sender);
            let mut handles = Vec::new();
            for values in receiver {
                handles.extend(values?);
            }
            for worker in workers {
                let _ = worker.join();
            }
            let mut collector = intersectionCollectWorker {
                pushedLimit: self.indexMerge.pushedLimit,
            };
            handles = collector.doIntersectionLimitAndDispatch(handles);
            self.dispatchHandles(handles);
            Ok::<(), errors::SharedError>(())
        })();
        if let Err(error) = result {
            syncResultErr(&self.indexMerge, error);
        }
        if let Some(stats) = &self.stats {
            stats
                .IndexMergeProcess
                .fetch_add(start.elapsed().as_nanos() as i64, Ordering::AcqRel);
        }
    }
}

/// 对已排序/已收集的 handle 切片应用 offset/count；返回是否读尽原列表。
fn pushedLimitCountingDown(
    limit: Option<PushedDownLimit>,
    handles: Vec<IndexMergeHandle>,
) -> (bool, Vec<IndexMergeHandle>) {
    let Some(limit) = limit else {
        return (true, handles);
    };
    let start = (limit.offset as usize).min(handles.len());
    let end = start
        .saturating_add(limit.count as usize)
        .min(handles.len());
    (end == handles.len(), handles[start..end].to_vec())
}

/// 若测试注册了取消钩子则调用。
fn notifyCancelHook() {
    if let Some(callback) = *IndexMergeCancelFuncForTest
        .lock()
        .expect("index merge cancel test hook poisoned")
    {
        callback();
    }
}

/// 交集结果收集器：统一应用下推 LIMIT。
pub struct intersectionCollectWorker {
    pushedLimit: Option<PushedDownLimit>,
}
impl intersectionCollectWorker {
    /// 对交集得到的 handle 应用 LIMIT 并返回裁剪后列表。
    fn doIntersectionLimitAndDispatch(
        &mut self,
        handles: Vec<IndexMergeHandle>,
    ) -> Vec<IndexMergeHandle> {
        pushedLimitCountingDown(self.pushedLimit, handles).1
    }
}

/// 单分区桶上的交集计算：仅保留出现在全部 partial 路径中的 handle。
pub struct intersectionProcessWorker {
    workerID: usize,
    indexMerge: Arc<indexMergeShared>,
    memTracker: Arc<IndexMergeMemoryTracker>,
    rowDelta: i64,
    mapUsageDelta: i64,
}
impl intersectionProcessWorker {
    fn new(workerID: usize, indexMerge: Arc<indexMergeShared>) -> Self {
        Self {
            workerID,
            memTracker: Arc::clone(&indexMerge.tracker),
            indexMerge,
            rowDelta: 0,
            mapUsageDelta: 0,
        }
    }
    /// 将本轮 map/行增量计入内存跟踪器后清零。
    fn consumeMemDelta(&mut self) {
        self.memTracker
            .Consume(self.mapUsageDelta + self.rowDelta * std::mem::size_of::<usize>() as i64);
        self.mapUsageDelta = 0;
        self.rowDelta = 0;
    }
    /// 统计每个 handle 覆盖的 partial 数，等于路径总数时保留。
    fn doIntersectionPerPartition(
        &mut self,
        tasks: Vec<Arc<indexMergeTableTask>>,
    ) -> Result<Vec<IndexMergeHandle>, errors::SharedError> {
        self.indexMerge
            .ctx
            .TriggerFailpoint("testIndexMergeIntersectionWorkerPanic");
        let mut paths: HashMap<(i64, Vec<u8>), (IndexMergeHandle, HashSet<usize>)> = HashMap::new();
        for task in tasks {
            task.Wait(&self.indexMerge.finish)?;
            for handle in &task.handles {
                let entry = paths
                    .entry((handle.partition_id, handle.encoded.clone()))
                    .or_insert_with(|| (handle.clone(), HashSet::new()));
                if entry.1.insert(task.partialPlanID) {
                    self.rowDelta += 1;
                }
            }
        }
        self.consumeMemDelta();
        Ok(paths
            .into_values()
            .filter_map(|(handle, paths)| {
                (paths.len() == self.indexMerge.plans.len()).then_some(handle)
            })
            .collect())
    }
}

/// 从 work 通道取任务，按 handle 回表并 Complete 结果。
pub struct indexMergeTableScanWorker {
    stats: Option<Arc<IndexMergeRuntimeStat>>,
    indexMergeExec: Arc<indexMergeShared>,
    memTracker: Arc<IndexMergeMemoryTracker>,
    current: Mutex<Option<Arc<indexMergeTableTask>>>,
}
impl indexMergeTableScanWorker {
    /// 循环领取并执行回表任务，直至 work 通道结束。
    fn pickAndExecTask(&mut self) {
        loop {
            let wait = Instant::now();
            let task = match receiveWork(&self.indexMergeExec) {
                Some(task) => task,
                None => return,
            };
            *self
                .current
                .lock()
                .expect("index merge current task poisoned") = Some(task.clone());
            let start = Instant::now();
            let result = self.executeTask(&task);
            if let Some(stats) = &self.stats {
                stats.WaitTime.fetch_add(
                    start.duration_since(wait).as_nanos() as i64,
                    Ordering::AcqRel,
                );
                stats
                    .FetchRow
                    .fetch_add(start.elapsed().as_nanos() as i64, Ordering::AcqRel);
                stats.TableTaskNum.fetch_add(1, Ordering::AcqRel);
            }
            match result {
                Ok((chunks, memory)) => task.Complete(chunks, None, memory),
                Err(error) => task.Complete(Vec::new(), Some(error), 0),
            }
            *self
                .current
                .lock()
                .expect("index merge current task poisoned") = None;
        }
    }
    /// panic 时若有当前任务则写入错误完成态。
    fn handleTableScanWorkerPanic(&self, payload: Box<dyn Any + Send>, worker: &str) {
        let error = panicError(payload.as_ref(), worker);
        if let Some(task) = self
            .current
            .lock()
            .expect("index merge current task poisoned")
            .take()
        {
            task.Complete(Vec::new(), Some(error), 0);
        }
    }
    /// 构建最终表读取器拉齐行；有序时重排；可选校验行数与 handle 数一致。
    fn executeTask(
        &self,
        task: &indexMergeTableTask,
    ) -> Result<(Vec<chunk::Chunk>, i64), errors::SharedError> {
        // 分区表且无全局索引时，回表限定在 handle 所属物理分区。
        let partition = (self.indexMergeExec.partitionTableMode
            && !self.indexMergeExec.hasGlobalIndex)
            .then(|| task.handles.first().map_or(0, |h| h.partition_id));
        let mut reader = IndexMergeReaderExecutor::buildFinalTableReader(
            &self.indexMergeExec,
            partition,
            &task.handles,
        )?;
        let cancellation = reader.Cancellation();
        self.indexMergeExec
            .cancellations
            .lock()
            .expect("index merge cancellation list poisoned")
            .push(cancellation.clone());
        if self.indexMergeExec.finish.IsFinished() {
            cancellation.Cancel();
        }
        let fetched = (|| {
            let mut chunks = Vec::new();
            let mut rows = 0;
            let mut memory =
                (task.handles.capacity() * std::mem::size_of::<IndexMergeHandle>()) as i64;
            loop {
                let mut output = reader.NewChunk();
                reader.Next(&mut output)?;
                if output.NumRows() == 0 {
                    break;
                }
                rows += output.NumRows();
                memory += output.MemoryUsage();
                chunks.push(output);
            }
            Ok::<_, errors::SharedError>((chunks, rows, memory))
        })();
        if let Err(error) = reader.Close() {
            self.indexMergeExec
                .ctx
                .LogCloseError(tableScanWorkerType, &error);
        }
        let (mut chunks, rows, memory) = fetched?;
        // 有序 IndexMerge：按 handle 顺序重排最终行块。
        if self.indexMergeExec.keepOrder {
            chunks = self
                .indexMergeExec
                .ctx
                .ReorderFinalRows(&task.handles, chunks)?;
        }
        if self.indexMergeExec.ctx.ValidateFinalRowCount() && rows != task.handles.len() {
            return Err(errors::New(format!(
                "handle count {} isn't equal to value count {rows}",
                task.handles.len()
            )));
        }
        self.memTracker.Consume(memory);
        Ok((chunks, memory))
    }
}

/// 带超时领取回表任务；finish 或 workFinished 后返回 None。
fn receiveWork(shared: &indexMergeShared) -> Option<Arc<indexMergeTableTask>> {
    loop {
        if shared.finish.IsFinished() {
            return None;
        }
        match shared
            .workReceiver
            .lock()
            .expect("index merge work receiver poisoned")
            .recv_timeout(Duration::from_millis(10))
        {
            Ok(task) => return Some(task),
            Err(RecvTimeoutError::Timeout) if shared.workFinished.load(Ordering::Acquire) => {
                return None;
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

/// 将错误封装为任务写入 fetch 通道。
fn syncErr(shared: &indexMergeShared, error: errors::SharedError) {
    let task = Arc::new(indexMergeTableTask::error(error));
    let _ = sendUntilFinished(
        &shared.fetchSender,
        &shared.finish,
        fetchMessage::Task(task),
    );
}

/// 将错误封装为任务写入 result 通道。
fn syncResultErr(shared: &indexMergeShared, error: errors::SharedError) {
    let task = Arc::new(indexMergeTableTask::error(error));
    let _ = sendUntilFinished(
        &shared.resultSender,
        &shared.finish,
        resultMessage::Task(task),
    );
}

/// worker panic：构造错误任务送入 result，并打印堆栈。
fn handleWorkerPanic(shared: &indexMergeShared, payload: Box<dyn Any + Send>, worker: &str) {
    let error = panicError(payload.as_ref(), worker);
    let message = error.to_string();
    let task = Arc::new(indexMergeTableTask::error(error));
    let _ = sendUntilFinished(
        &shared.resultSender,
        &shared.finish,
        resultMessage::Task(task),
    );
    eprintln!("{message}\n{}", Backtrace::force_capture());
}

/// 将 panic payload 格式化为带 worker 名前缀的 SharedError。
fn panicError(payload: &(dyn Any + Send), worker: &str) -> errors::SharedError {
    let detail = payload
        .downcast_ref::<&str>()
        .map(|v| (*v).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned());
    errors::New(format!("{worker}: {detail}"))
}

/// IndexMerge 运行时耗时与任务计数统计。
pub struct IndexMergeRuntimeStat {
    pub IndexMergeProcess: AtomicI64,
    pub FetchIdxTime: AtomicI64,
    pub WaitTime: AtomicI64,
    pub FetchRow: AtomicI64,
    pub TableTaskNum: AtomicI64,
    pub Concurrency: usize,
    pub Type: i32,
}
impl IndexMergeRuntimeStat {
    /// 创建空统计，记录回表并发度与统计类型。
    fn new(concurrency: usize, statsType: i32) -> Self {
        Self {
            IndexMergeProcess: AtomicI64::new(0),
            FetchIdxTime: AtomicI64::new(0),
            WaitTime: AtomicI64::new(0),
            FetchRow: AtomicI64::new(0),
            TableTaskNum: AtomicI64::new(0),
            Concurrency: concurrency,
            Type: statsType,
        }
    }
    /// 格式化为 `index_task`/`table_task` 可读摘要。
    pub fn String(&self) -> String {
        let mut output = String::new();
        let fetch = self.FetchIdxTime.load(Ordering::Acquire);
        let merge = self.IndexMergeProcess.load(Ordering::Acquire);
        let fetchRow = self.FetchRow.load(Ordering::Acquire);
        if fetch != 0 {
            let _ = write!(
                output,
                "index_task:{{fetch_handle:{:?}",
                Duration::from_nanos(fetch as u64)
            );
            if merge != 0 {
                let _ = write!(output, ", merge:{:?}", Duration::from_nanos(merge as u64));
            }
            output.push('}');
        }
        if fetchRow != 0 {
            if !output.is_empty() {
                output.push(',');
            }
            let _ = write!(
                output,
                " table_task:{{num:{}, concurrency:{}, fetch_row:{:?}, wait_time:{:?}}}",
                self.TableTaskNum.load(Ordering::Acquire),
                self.Concurrency,
                Duration::from_nanos(fetchRow as u64),
                Duration::from_nanos(self.WaitTime.load(Ordering::Acquire) as u64)
            );
        }
        output
    }
    /// 快照当前原子计数到新实例。
    pub fn Clone(&self) -> Self {
        Self {
            IndexMergeProcess: AtomicI64::new(self.IndexMergeProcess.load(Ordering::Acquire)),
            FetchIdxTime: AtomicI64::new(self.FetchIdxTime.load(Ordering::Acquire)),
            WaitTime: AtomicI64::new(self.WaitTime.load(Ordering::Acquire)),
            FetchRow: AtomicI64::new(self.FetchRow.load(Ordering::Acquire)),
            TableTaskNum: AtomicI64::new(self.TableTaskNum.load(Ordering::Acquire)),
            Concurrency: self.Concurrency,
            Type: self.Type,
        }
    }
    /// 将另一份统计累加到自身。
    pub fn Merge(&self, other: &Self) {
        self.IndexMergeProcess.fetch_add(
            other.IndexMergeProcess.load(Ordering::Acquire),
            Ordering::AcqRel,
        );
        self.FetchIdxTime
            .fetch_add(other.FetchIdxTime.load(Ordering::Acquire), Ordering::AcqRel);
        self.FetchRow
            .fetch_add(other.FetchRow.load(Ordering::Acquire), Ordering::AcqRel);
        self.WaitTime
            .fetch_add(other.WaitTime.load(Ordering::Acquire), Ordering::AcqRel);
        self.TableTaskNum
            .fetch_add(other.TableTaskNum.load(Ordering::Acquire), Ordering::AcqRel);
    }
    /// 返回统计类型编号。
    pub fn Tp(&self) -> i32 {
        self.Type
    }
}
impl fmt::Display for IndexMergeRuntimeStat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.String())
    }
}
