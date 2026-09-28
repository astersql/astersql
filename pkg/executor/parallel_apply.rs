// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 并行 Nested Loop Apply 执行器（Parallel Nested Loop Apply）。
//
// Apply 是相关子查询的物理实现：对外层每一行绑定相关列后执行内层计划。
// 本模块用 1 个外层拉取线程 + N 个内层 worker 并行处理；可选保序
//（keepOrder）与 Apply 结果缓存。通过 channel 回收输出 Chunk，并用
// finish 信号在取消 / 关闭时打断阻塞发送。

#![allow(non_camel_case_types, non_snake_case)]

use std::any::Any;
use std::backtrace::Backtrace;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use astersql_errors as errors;
use astersql_util_chunk as chunk;

/// 过滤表达式的类型擦除句柄（由运行时上下文解释）。
pub type ApplyExpression = Arc<dyn Any + Send + Sync>;
/// 相关列（correlated column）的类型擦除句柄。
pub type ApplyCorrelatedColumn = Arc<dyn Any + Send + Sync>;

/// 取消令牌：中止正在进行的 Next。
pub trait ParallelApplyCancellation: Send + Sync {
    /// 触发取消。
    fn Cancel(&self);
}

/// Production executor boundary. Cancellation must abort an in-flight Next call.
/// 生产侧执行器边界；取消必须能打断进行中的 Next。
pub trait ParallelApplyExecutor: Send {
    fn Open(
        &mut self,
        ctx: Arc<dyn ParallelApplyRuntimeContext>,
    ) -> Result<(), errors::SharedError>;
    fn Close(&mut self) -> Result<(), errors::SharedError>;
    fn Cancellation(&self) -> Arc<dyn ParallelApplyCancellation>;
    fn Next(
        &mut self,
        ctx: Arc<dyn ParallelApplyRuntimeContext>,
        req: &mut chunk::Chunk,
    ) -> Result<(), errors::SharedError>;
    fn NewFirstChunk(&mut self) -> chunk::Chunk;
    fn TryNewCacheChunk(&mut self) -> chunk::Chunk;
    fn HasRuntimeStats(&self) -> bool;
}

/// Join implementations own the inner cursor semantics and must advance cursor
/// whenever unconsumed inner rows remain.
/// 连接实现拥有内层游标语义；尚有未消费内层行时必须推进游标。
pub trait ParallelApplyJoiner: Send {
    fn TryToMatchInners(
        &mut self,
        outer: &chunk::Row,
        inners: &[chunk::Row],
        cursor: &mut usize,
        output: &mut chunk::Chunk,
    ) -> Result<(bool, bool), errors::SharedError>;
    fn OnMissMatch(&mut self, has_null: bool, outer: &chunk::Row, output: &mut chunk::Chunk);
}

/// 关闭时上报的运行时统计。
pub struct ParallelApplyRuntimeStats {
    /// worker 并发度。
    pub concurrency: usize,
    /// 是否启用 Apply 缓存。
    pub cache_enabled: bool,
    /// 缓存命中率。
    pub cache_hit_ratio: f64,
    /// 跟踪到的内存字节数。
    pub memory_bytes: i64,
}

/// Session, expression, cache, failpoint, memory and statistics boundary.
/// 会话、表达式、缓存、failpoint、内存与统计的运行时边界。
pub trait ParallelApplyRuntimeContext: Send + Sync {
    fn VectorizedFilterOuter(
        &self,
        input: &chunk::Chunk,
        filters: &[ApplyExpression],
        reuse: Vec<bool>,
    ) -> Result<Vec<bool>, errors::SharedError>;
    fn VectorizedFilterInner(
        &self,
        worker_id: usize,
        input: &chunk::Chunk,
        filters: &[ApplyExpression],
        reuse: Vec<bool>,
    ) -> Result<Vec<bool>, errors::SharedError>;
    fn BindCorrelatedColumns(
        &self,
        worker_id: usize,
        outer: &chunk::Row,
        columns: &[ApplyCorrelatedColumn],
    ) -> Result<(), errors::SharedError>;
    fn EncodeCorrelatedKey(
        &self,
        worker_id: usize,
        outer: &chunk::Row,
        columns: &[ApplyCorrelatedColumn],
    ) -> Result<Vec<u8>, errors::SharedError>;
    fn InitializeApplyCache(&self) -> Result<(), errors::SharedError>;
    fn ApplyCacheGet(&self, key: &[u8]) -> Result<Option<Vec<chunk::Chunk>>, errors::SharedError>;
    fn ApplyCacheSet(
        &self,
        key: Vec<u8>,
        chunks: Vec<chunk::Chunk>,
    ) -> Result<(), errors::SharedError>;
    fn AttachMemoryTracker(&self, tracker: Arc<ParallelApplyMemoryTracker>);
    fn DetachMemoryTracker(&self, tracker: Arc<ParallelApplyMemoryTracker>);
    fn RegisterRuntimeStats(&self, stats: ParallelApplyRuntimeStats);
    fn LogInnerCloseError(&self, worker_id: usize, error: &errors::SharedError);
    fn TriggerOuterWorkerFailpoint(&self);
    fn TriggerInnerWorkerFailpoint(&self, worker_id: usize);
    fn TriggerInnerWorkerOrderedFailpoint(&self, worker_id: usize);
    fn TriggerOrderedSleepFailpoint(&self, worker_id: usize) -> Result<(), errors::SharedError>;
    fn TriggerCacheGetFailpoint(&self);
    fn TriggerCacheSetFailpoint(&self);
    fn TriggerSlowInnerFailpoint(&self, worker_id: usize) -> Result<(), errors::SharedError>;
}

/// 原子累计 Apply 持有的 Chunk 内存字节数。
#[derive(Default)]
pub struct ParallelApplyMemoryTracker {
    bytes: AtomicI64,
}

impl ParallelApplyMemoryTracker {
    /// 当前累计消耗的字节数。
    pub fn BytesConsumed(&self) -> i64 {
        self.bytes.load(Ordering::Acquire)
    }

    /// 增减跟踪字节（释放时传负值）。
    fn Consume(&self, bytes: i64) {
        self.bytes.fetch_add(bytes, Ordering::AcqRel);
    }
}

/// 关闭 / 取消时唤醒阻塞在 channel 上的发送与接收。
#[derive(Default)]
struct finishSignal {
    finished: AtomicBool,
    lock: Mutex<()>,
    changed: Condvar,
}

impl finishSignal {
    /// 标记结束并唤醒所有等待者。
    fn Finish(&self) {
        self.finished.store(true, Ordering::Release);
        self.changed.notify_all();
    }

    /// 是否已结束。
    fn IsFinished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    /// 短暂等待 finish 变更，用于发送侧退避。
    fn WaitBriefly(&self) {
        let guard = self
            .lock
            .lock()
            .expect("parallel apply finish lock poisoned");
        let _wait = self
            .changed
            .wait_timeout(guard, Duration::from_millis(2))
            .expect("parallel apply finish wait poisoned");
    }
}

/// 非阻塞发送直到成功、通道断开或 finish；返回 true 表示被打断。
fn sendUntilFinished<T>(sender: &SyncSender<T>, finish: &finishSignal, mut value: T) -> bool {
    loop {
        if finish.IsFinished() {
            return true;
        }
        match sender.try_send(value) {
            Ok(()) => return false,
            Err(TrySendError::Full(returned)) => {
                value = returned;
                finish.WaitBriefly();
            }
            Err(TrySendError::Disconnected(_)) => return true,
        }
    }
}

/// 带超时接收直到拿到值、断开或 finish；finish 时返回 None。
fn receiveUntilFinished<T>(receiver: &Arc<Mutex<Receiver<T>>>, finish: &finishSignal) -> Option<T> {
    loop {
        if finish.IsFinished() {
            return None;
        }
        match receiver
            .lock()
            .expect("parallel apply receiver lock poisoned")
            .recv_timeout(Duration::from_millis(10))
        {
            Ok(value) => return Some(value),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

/// 向调用方 Next 返回的一块结果或错误。
pub struct result {
    /// 输出 Chunk；`None` 且无错误表示流结束。
    pub chk: Option<chunk::Chunk>,
    /// 若有错误则优先于 Chunk。
    pub err: Option<errors::SharedError>,
}

/// 外层行及其过滤结果与保序序号。
#[derive(Clone)]
pub struct outerRow {
    /// 外层行。
    pub row: chunk::Row,
    /// 是否通过外层过滤。
    pub selected: bool,
    /// 保序用递增序号。
    pub seq: u64,
}

/// 有序模式下单个外层行对应的内层产出。
pub struct orderedResult {
    /// 对应外层行序号。
    pub seq: u64,
    /// 产出的 Chunk 列表。
    pub chks: Vec<chunk::Chunk>,
    /// 处理该外层行时的错误。
    pub err: Option<errors::SharedError>,
}

/// 有序通道消息：结果或流结束标记。
enum orderedMessage {
    Result(orderedResult),
    Finished,
}

/// 持有若干 Chunk 并同步更新内存跟踪的行列表。
#[derive(Default)]
struct rowList {
    chunks: Vec<Box<chunk::Chunk>>,
}

impl rowList {
    /// 追加 Chunk，计入内存，并返回其中所有行视图。
    fn Add(
        &mut self,
        value: chunk::Chunk,
        tracker: &ParallelApplyMemoryTracker,
    ) -> Vec<chunk::Row> {
        tracker.Consume(value.MemoryUsage());
        self.chunks.push(Box::new(value));
        let chunk = self.chunks.last().expect("row list just received a chunk");
        (0..chunk.NumRows())
            .map(|index| chunk.GetRow(index))
            .collect()
    }

    /// 展平为行视图列表。
    fn Rows(&self) -> Vec<chunk::Row> {
        self.chunks
            .iter()
            .flat_map(|value| (0..value.NumRows()).map(|index| value.GetRow(index)))
            .collect()
    }

    /// 克隆全部 Chunk（用于写入缓存）。
    fn Snapshot(&self) -> Vec<chunk::Chunk> {
        self.chunks.iter().map(|value| (**value).clone()).collect()
    }

    /// 用新 Chunk 集合替换内容并校正内存跟踪。
    fn Replace(&mut self, values: Vec<chunk::Chunk>, tracker: &ParallelApplyMemoryTracker) {
        self.Reset(tracker);
        for value in values {
            self.Add(value, tracker);
        }
    }

    /// 清空并扣减内存跟踪。
    fn Reset(&mut self, tracker: &ParallelApplyMemoryTracker) {
        let bytes = self
            .chunks
            .iter()
            .map(|value| value.MemoryUsage())
            .sum::<i64>();
        tracker.Consume(-bytes);
        self.chunks.clear();
    }
}

/// 单个内层 worker 的可变状态。
struct applyWorkerState {
    /// 本 worker 绑定的相关列。
    corCols: Vec<ApplyCorrelatedColumn>,
    /// 内层过滤表达式。
    innerFilter: Vec<ApplyExpression>,
    /// 已拉取的内层行列表。
    innerList: rowList,
    /// 可复用的内层输入 Chunk。
    innerChunk: Option<chunk::Chunk>,
    /// 最近一次内层过滤选中位图。
    innerSelected: Vec<bool>,
    /// 当前正在处理的外层行。
    outerRow: Option<chunk::Row>,
    /// 内层行游标。
    innerCursor: usize,
    /// 是否已与任一内层行匹配。
    hasMatch: bool,
    /// 匹配过程中是否见过 NULL 语义。
    hasNull: bool,
    /// 连接器实现。
    joiner: Box<dyn ParallelApplyJoiner>,
}

/// 内层执行器 + 取消令牌 + 状态。
struct applyWorker {
    innerExec: Arc<Mutex<Box<dyn ParallelApplyExecutor>>>,
    cancellation: Arc<dyn ParallelApplyCancellation>,
    state: Mutex<applyWorkerState>,
}

/// 外层 / 内层 / 重排线程共享的通道与配置。
struct parallelApplyShared {
    base: Arc<Mutex<Box<dyn ParallelApplyExecutor>>>,
    outerExec: Arc<Mutex<Box<dyn ParallelApplyExecutor>>>,
    outerCancellation: Arc<dyn ParallelApplyCancellation>,
    outerFilter: Vec<ApplyExpression>,
    outerList: Mutex<rowList>,
    outer: bool,
    workers: Vec<Arc<applyWorker>>,
    concurrency: usize,
    keepOrder: bool,
    useCache: bool,
    cacheHitCounter: AtomicI64,
    cacheAccessCounter: AtomicI64,
    memTracker: Arc<ParallelApplyMemoryTracker>,
    ctx: Arc<dyn ParallelApplyRuntimeContext>,
    finish: Arc<finishSignal>,
    freeSender: SyncSender<chunk::Chunk>,
    freeReceiver: Arc<Mutex<Receiver<chunk::Chunk>>>,
    resultSender: SyncSender<result>,
    outerSender: SyncSender<outerRow>,
    outerReceiver: Arc<Mutex<Receiver<outerRow>>>,
    outerFinished: AtomicBool,
    orderedSender: Option<SyncSender<orderedMessage>>,
    orderedReceiver: Option<Arc<Mutex<Receiver<orderedMessage>>>>,
    paceSender: Option<SyncSender<()>>,
    paceReceiver: Option<Arc<Mutex<Receiver<()>>>>,
}

/// ParallelNestedLoopApplyExec runs one outer fetcher and N inner workers.
/// 并行 Nested Loop Apply：一个外层拉取线程与 N 个内层 worker。
pub struct ParallelNestedLoopApplyExec {
    /// 用于分配输出 Chunk 的基执行器。
    base: Arc<Mutex<Box<dyn ParallelApplyExecutor>>>,
    /// 外层执行器。
    outerExec: Arc<Mutex<Box<dyn ParallelApplyExecutor>>>,
    /// 外层取消令牌。
    outerCancellation: Arc<dyn ParallelApplyCancellation>,
    /// 外层过滤表达式。
    outerFilter: Vec<ApplyExpression>,
    /// 是否为外连接语义（未匹配时补 OnMissMatch）。
    outer: bool,
    /// 内层 worker 列表。
    workers: Vec<Arc<applyWorker>>,
    /// 并发 worker 数。
    concurrency: usize,
    /// 是否按外层行序输出。
    keepOrder: bool,
    /// 是否启用相关键结果缓存。
    useCache: bool,
    /// 是否已启动后台线程。
    started: AtomicBool,
    /// 结果流是否已排空。
    drained: AtomicBool,
    /// 主线程接收结果的通道端。
    resultReceiver: Option<Receiver<result>>,
    /// Open 后创建的共享状态。
    shared: Option<Arc<parallelApplyShared>>,
    /// 协调线程句柄（等待 worker / 重排结束）。
    coordinator: Option<JoinHandle<()>>,
}

impl ParallelNestedLoopApplyExec {
    /// 构造执行器；`concurrency` 与各 per-worker 向量长度必须一致且为正。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        base: Box<dyn ParallelApplyExecutor>,
        outerExec: Box<dyn ParallelApplyExecutor>,
        outerFilter: Vec<ApplyExpression>,
        outer: bool,
        corCols: Vec<Vec<ApplyCorrelatedColumn>>,
        innerFilter: Vec<Vec<ApplyExpression>>,
        innerExecs: Vec<Box<dyn ParallelApplyExecutor>>,
        joiners: Vec<Box<dyn ParallelApplyJoiner>>,
        concurrency: usize,
        keepOrder: bool,
        useCache: bool,
    ) -> Self {
        assert!(
            concurrency > 0,
            "parallel apply concurrency must be positive"
        );
        assert_eq!(
            innerExecs.len(),
            concurrency,
            "one inner executor per worker"
        );
        assert_eq!(joiners.len(), concurrency, "one joiner per worker");
        assert_eq!(
            corCols.len(),
            concurrency,
            "one correlated-column set per worker"
        );
        assert_eq!(
            innerFilter.len(),
            concurrency,
            "one inner filter per worker"
        );
        let outerCancellation = outerExec.Cancellation();
        let workers = innerExecs
            .into_iter()
            .zip(joiners)
            .zip(corCols)
            .zip(innerFilter)
            .map(|(((innerExec, joiner), corCols), innerFilter)| {
                let cancellation = innerExec.Cancellation();
                Arc::new(applyWorker {
                    innerExec: Arc::new(Mutex::new(innerExec)),
                    cancellation,
                    state: Mutex::new(applyWorkerState {
                        corCols,
                        innerFilter,
                        innerList: rowList::default(),
                        innerChunk: None,
                        innerSelected: Vec::new(),
                        outerRow: None,
                        innerCursor: 0,
                        hasMatch: false,
                        hasNull: false,
                        joiner,
                    }),
                })
            })
            .collect();
        Self {
            base: Arc::new(Mutex::new(base)),
            outerExec: Arc::new(Mutex::new(outerExec)),
            outerCancellation,
            outerFilter,
            outer,
            workers,
            concurrency,
            keepOrder,
            useCache,
            started: AtomicBool::new(false),
            drained: AtomicBool::new(false),
            resultReceiver: None,
            shared: None,
            coordinator: None,
        }
    }

    /// 打开：初始化 worker 状态、通道、缓存，并挂载内存跟踪器。
    pub fn Open(
        &mut self,
        ctx: Arc<dyn ParallelApplyRuntimeContext>,
    ) -> Result<(), errors::SharedError> {
        self.outerExec
            .lock()
            .expect("parallel apply outer executor lock poisoned")
            .Open(Arc::clone(&ctx))?;
        let tracker = Arc::new(ParallelApplyMemoryTracker::default());
        ctx.AttachMemoryTracker(Arc::clone(&tracker));

        // 重置每个 worker 的内层缓冲与匹配状态。
        for worker in &self.workers {
            let mut state = worker
                .state
                .lock()
                .expect("parallel apply worker state poisoned");
            state.innerList = rowList::default();
            state.innerChunk = Some(
                worker
                    .innerExec
                    .lock()
                    .expect("parallel apply inner executor lock poisoned")
                    .TryNewCacheChunk(),
            );
            state.innerSelected.clear();
            state.outerRow = None;
            state.innerCursor = 0;
            state.hasMatch = false;
            state.hasNull = false;
        }

        let (freeSender, freeReceiver) = mpsc::sync_channel(self.concurrency);
        let (resultSender, resultReceiver) = mpsc::sync_channel(self.concurrency + 1);
        let (outerSender, outerReceiver) = mpsc::sync_channel(0);
        // 保序模式额外建立有序结果通道与 pace 令牌通道。
        let (orderedSender, orderedReceiver) = if self.keepOrder {
            let (sender, receiver) = mpsc::sync_channel(self.concurrency * 2);
            (Some(sender), Some(Arc::new(Mutex::new(receiver))))
        } else {
            (None, None)
        };
        let (paceSender, paceReceiver) = if self.keepOrder {
            let (sender, receiver) = mpsc::sync_channel(self.concurrency * 4);
            (Some(sender), Some(Arc::new(Mutex::new(receiver))))
        } else {
            (None, None)
        };
        for _ in 0..self.concurrency {
            // 预填可回收的输出 Chunk。
            let output = self
                .base
                .lock()
                .expect("parallel apply base executor lock poisoned")
                .NewFirstChunk();
            freeSender
                .send(output)
                .map_err(|_| errors::New("parallel apply free chunk channel disconnected"))?;
        }
        if self.useCache {
            ctx.InitializeApplyCache()?;
        }

        let shared = Arc::new(parallelApplyShared {
            base: Arc::clone(&self.base),
            outerExec: Arc::clone(&self.outerExec),
            outerCancellation: Arc::clone(&self.outerCancellation),
            outerFilter: self.outerFilter.clone(),
            outerList: Mutex::new(rowList::default()),
            outer: self.outer,
            workers: self.workers.clone(),
            concurrency: self.concurrency,
            keepOrder: self.keepOrder,
            useCache: self.useCache,
            cacheHitCounter: AtomicI64::new(0),
            cacheAccessCounter: AtomicI64::new(0),
            memTracker: tracker,
            ctx,
            finish: Arc::new(finishSignal::default()),
            freeSender,
            freeReceiver: Arc::new(Mutex::new(freeReceiver)),
            resultSender,
            outerSender,
            outerReceiver: Arc::new(Mutex::new(outerReceiver)),
            outerFinished: AtomicBool::new(false),
            orderedSender,
            orderedReceiver,
            paceSender,
            paceReceiver,
        });
        self.started.store(false, Ordering::Release);
        self.drained.store(false, Ordering::Release);
        self.resultReceiver = Some(resultReceiver);
        self.shared = Some(shared);
        Ok(())
    }

    /// 拉取下一批输出；首次调用时惰性启动 worker 线程。
    pub fn Next(
        &mut self,
        _ctx: Arc<dyn ParallelApplyRuntimeContext>,
        req: &mut chunk::Chunk,
    ) -> Result<(), errors::SharedError> {
        if self.drained.load(Ordering::Acquire) {
            req.Reset();
            return Ok(());
        }
        // 仅第一次 Next 启动外层 / 内层 / 协调线程。
        if self
            .started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.startWorkers();
        }
        let received = self
            .resultReceiver
            .as_ref()
            .expect("parallel apply must be opened before Next")
            .recv()
            .map_err(|_| errors::New("parallel apply result channel disconnected"))?;
        if let Some(error) = received.err {
            return Err(error);
        }
        let Some(mut output) = received.chk else {
            req.Reset();
            self.drained.store(true, Ordering::Release);
            return Ok(());
        };
        req.SwapColumns(&mut output);
        let shared = self
            .shared
            .as_ref()
            .expect("parallel apply shared state missing");
        // 用过的输出 Chunk 归还 free 池供 worker 复用。
        if sendUntilFinished(&shared.freeSender, &shared.finish, output) {
            return Err(errors::New("parallel apply output recycle interrupted"));
        }
        Ok(())
    }

    /// 关闭：发 finish、取消所有执行器、汇总统计并释放内存跟踪。
    pub fn Close(&mut self) -> Result<(), errors::SharedError> {
        if let Some(shared) = &self.shared {
            shared.finish.Finish();
            shared.outerCancellation.Cancel();
            for worker in &shared.workers {
                worker.cancellation.Cancel();
            }
        }
        let mut firstError = None;
        if let Some(coordinator) = self.coordinator.take()
            && coordinator.join().is_err()
        {
            firstError = Some(errors::New("parallel apply coordinator panicked"));
        }
        if let Some(receiver) = &self.resultReceiver {
            while receiver.try_recv().is_ok() {}
        }
        if let Err(error) = self
            .outerExec
            .lock()
            .expect("parallel apply outer executor lock poisoned")
            .Close()
            && firstError.is_none()
        {
            firstError = Some(error);
        }

        if let Some(shared) = &self.shared {
            // Go drops the apply-owned lists as part of Close. Release their
            // tracked memory before publishing the final close-time snapshot,
            // otherwise runtime stats report bytes that no longer remain live.
            shared
                .outerList
                .lock()
                .expect("parallel apply outer list lock poisoned")
                .Reset(&shared.memTracker);
            for worker in &shared.workers {
                worker
                    .state
                    .lock()
                    .expect("parallel apply worker state poisoned")
                    .innerList
                    .Reset(&shared.memTracker);
            }
            let cacheAccess = shared.cacheAccessCounter.load(Ordering::Acquire);
            let cacheHit = shared.cacheHitCounter.load(Ordering::Acquire);
            if shared
                .base
                .lock()
                .expect("parallel apply base executor lock poisoned")
                .HasRuntimeStats()
            {
                shared.ctx.RegisterRuntimeStats(ParallelApplyRuntimeStats {
                    concurrency: shared.concurrency,
                    cache_enabled: shared.useCache,
                    cache_hit_ratio: if cacheAccess > 0 {
                        cacheHit as f64 / cacheAccess as f64
                    } else {
                        0.0
                    },
                    memory_bytes: shared.memTracker.BytesConsumed(),
                });
            }
            shared
                .ctx
                .DetachMemoryTracker(Arc::clone(&shared.memTracker));
        }
        self.started.store(false, Ordering::Release);
        self.drained.store(false, Ordering::Release);
        self.resultReceiver = None;
        self.shared = None;
        match firstError {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// 启动外层拉取、内层 worker，以及保序重排或无序结束通知协调线程。
    fn startWorkers(&mut self) {
        let shared = Arc::clone(
            self.shared
                .as_ref()
                .expect("parallel apply must be opened before start"),
        );
        let mut workers = Vec::with_capacity(shared.concurrency + 1);
        let outerShared = Arc::clone(&shared);
        workers.push(thread::spawn(move || {
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Self::outerWorker(outerShared.clone());
            }));
            if let Err(payload) = panic {
                Self::handleWorkerPanic(&outerShared, payload);
            }
        }));

        for id in 0..shared.concurrency {
            let workerShared = Arc::clone(&shared);
            workers.push(thread::spawn(move || {
                let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    // 保序与乱序走不同的内层驱动循环。
                    if workerShared.keepOrder {
                        Self::innerWorkerOrdered(workerShared.clone(), id);
                    } else {
                        Self::innerWorker(workerShared.clone(), id);
                    }
                }));
                if let Err(payload) = panic {
                    Self::handleWorkerPanic(&workerShared, payload);
                }
            }));
        }

        self.coordinator = Some(thread::spawn(move || {
            if shared.keepOrder {
                // 保序：额外启动 reorderWorker，等全部 worker 结束后发 Finished。
                let reorderShared = Arc::clone(&shared);
                let reorder = thread::spawn(move || {
                    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        Self::reorderWorker(reorderShared.clone());
                    }));
                    if let Err(payload) = panic {
                        Self::handleWorkerPanic(&reorderShared, payload);
                    }
                });
                for worker in workers {
                    let _ = worker.join();
                }
                if let Some(sender) = &shared.orderedSender {
                    let _ = sendUntilFinished(sender, &shared.finish, orderedMessage::Finished);
                }
                let _ = reorder.join();
            } else {
                Self::notifyWorker(shared, workers);
            }
        }));
    }

    /// 乱序模式：等待全部 worker 结束后发送流结束哨兵。
    fn notifyWorker(shared: Arc<parallelApplyShared>, workers: Vec<JoinHandle<()>>) {
        for worker in workers {
            let _ = worker.join();
        }
        Self::putResult(&shared, None, None);
    }

    /// 外层线程：拉取外层 Chunk、过滤，并按行投递到 outer 通道。
    fn outerWorker(shared: Arc<parallelApplyShared>) {
        shared.ctx.TriggerOuterWorkerFailpoint();
        let mut selected = Vec::new();
        let mut seq = 0_u64;
        loop {
            if shared.finish.IsFinished() {
                return;
            }
            let mut input = shared
                .outerExec
                .lock()
                .expect("parallel apply outer executor lock poisoned")
                .TryNewCacheChunk();
            let next = shared
                .outerExec
                .lock()
                .expect("parallel apply outer executor lock poisoned")
                .Next(Arc::clone(&shared.ctx), &mut input);
            if let Err(error) = next {
                if !shared.finish.IsFinished() {
                    Self::putResult(&shared, None, Some(error));
                }
                return;
            }
            if input.NumRows() == 0 {
                shared.outerFinished.store(true, Ordering::Release);
                return;
            }
            selected = match shared
                .ctx
                .VectorizedFilterOuter(&input, &shared.outerFilter, selected)
            {
                Ok(value) => value,
                Err(error) => {
                    Self::putResult(&shared, None, Some(error));
                    return;
                }
            };
            if selected.len() != input.NumRows() {
                Self::putResult(
                    &shared,
                    None,
                    Some(errors::New("outer filter returned the wrong row count")),
                );
                return;
            }
            let rows = shared
                .outerList
                .lock()
                .expect("parallel apply outer list lock poisoned")
                .Add(input, &shared.memTracker);
            for (row, selected) in rows.into_iter().zip(selected.iter().copied()) {
                // 保序时先发 pace 令牌，防止 reorder 超前消费。
                if let Some(pace) = &shared.paceSender
                    && sendUntilFinished(pace, &shared.finish, ())
                {
                    return;
                }
                let output = outerRow { row, selected, seq };
                seq += 1;
                if sendUntilFinished(&shared.outerSender, &shared.finish, output) {
                    return;
                }
            }
        }
    }

    /// 乱序内层：从 free 池取输出 Chunk，填充后送入结果通道。
    fn innerWorker(shared: Arc<parallelApplyShared>, id: usize) {
        shared.ctx.TriggerInnerWorkerFailpoint(id);
        loop {
            let Some(mut output) = receiveUntilFinished(&shared.freeReceiver, &shared.finish)
            else {
                return;
            };
            let error = Self::fillInnerChunk(&shared, id, &mut output).err();
            // 空 Chunk 且无错误表示该 worker 已无更多外层行可处理。
            if error.is_none() && output.NumRows() == 0 {
                return;
            }
            if error.is_some() && shared.finish.IsFinished() {
                return;
            }
            if Self::putResult(&shared, Some(output), error) {
                return;
            }
        }
    }

    /// 保序内层：每次完整处理一行外层，按 seq 发送有序结果。
    fn innerWorkerOrdered(shared: Arc<parallelApplyShared>, id: usize) {
        shared.ctx.TriggerInnerWorkerOrderedFailpoint(id);
        loop {
            let Some(outer) = Self::receiveOuterRow(&shared) else {
                return;
            };
            if let Err(error) = shared.ctx.TriggerOrderedSleepFailpoint(id) {
                Self::sendOrdered(
                    &shared,
                    orderedResult {
                        seq: outer.seq,
                        chks: Vec::new(),
                        err: Some(error),
                    },
                );
                return;
            }
            match Self::processOneOuterRow(&shared, id, outer.clone()) {
                Ok(chks) => {
                    if Self::sendOrdered(
                        &shared,
                        orderedResult {
                            seq: outer.seq,
                            chks,
                            err: None,
                        },
                    ) {
                        return;
                    }
                }
                Err(error) => {
                    Self::sendOrdered(
                        &shared,
                        orderedResult {
                            seq: outer.seq,
                            chks: Vec::new(),
                            err: Some(error),
                        },
                    );
                    return;
                }
            }
        }
    }

    /// 处理单个外层行：绑相关列、拉全内层、连接写出；未匹配时补 OnMissMatch。
    fn processOneOuterRow(
        shared: &parallelApplyShared,
        id: usize,
        outer: outerRow,
    ) -> Result<Vec<chunk::Chunk>, errors::SharedError> {
        if !outer.selected {
            // 未通过外层过滤：外连接仍需输出 OnMissMatch 行。
            if shared.outer {
                let mut output = shared
                    .base
                    .lock()
                    .expect("parallel apply base executor lock poisoned")
                    .NewFirstChunk();
                output.SetRequiredRows(1, 1);
                shared.workers[id]
                    .state
                    .lock()
                    .expect("parallel apply worker state poisoned")
                    .joiner
                    .OnMissMatch(false, &outer.row, &mut output);
                return Ok(vec![output]);
            }
            return Ok(Vec::new());
        }
        {
            let mut state = shared.workers[id]
                .state
                .lock()
                .expect("parallel apply worker state poisoned");
            state.outerRow = Some(outer.row.clone());
            state.hasMatch = false;
            state.hasNull = false;
            state.innerCursor = 0;
        }
        Self::fetchAllInners(shared, id)?;
        let mut chunks = Vec::new();
        let mut output = shared
            .base
            .lock()
            .expect("parallel apply base executor lock poisoned")
            .NewFirstChunk();
        loop {
            let done = {
                let mut state = shared.workers[id]
                    .state
                    .lock()
                    .expect("parallel apply worker state poisoned");
                let rows = state.innerList.Rows();
                if state.innerCursor >= rows.len() {
                    true
                } else {
                    let before = state.innerCursor;
                    let outer = state
                        .outerRow
                        .clone()
                        .expect("parallel apply outer row missing");
                    let applyWorkerState {
                        joiner,
                        innerCursor,
                        hasMatch,
                        hasNull,
                        ..
                    } = &mut *state;
                    let (matched, isNull) =
                        joiner.TryToMatchInners(&outer, &rows, innerCursor, &mut output)?;
                    *hasMatch |= matched;
                    *hasNull |= isNull;
                    // joiner 未推进游标会导致死循环，显式报错。
                    if *innerCursor == before && *innerCursor < rows.len() {
                        return Err(errors::New(
                            "parallel apply joiner did not advance inner cursor",
                        ));
                    }
                    false
                }
            };
            if done {
                break;
            }
            if output.IsFull() {
                chunks.push(output);
                output = shared
                    .base
                    .lock()
                    .expect("parallel apply base executor lock poisoned")
                    .NewFirstChunk();
            }
        }
        {
            let mut state = shared.workers[id]
                .state
                .lock()
                .expect("parallel apply worker state poisoned");
            if !state.hasMatch {
                let outer = state
                    .outerRow
                    .clone()
                    .expect("parallel apply outer row missing");
                let hasNull = state.hasNull;
                state.joiner.OnMissMatch(hasNull, &outer, &mut output);
            }
        }
        if output.NumRows() > 0 {
            chunks.push(output);
        }
        Ok(chunks)
    }

    /// 按 seq 重排有序结果并写回主结果通道；用 pace 与外层生产节流对齐。
    fn reorderWorker(shared: Arc<parallelApplyShared>) {
        let receiver = shared
            .orderedReceiver
            .as_ref()
            .expect("ordered result receiver missing");
        let Some(mut output) = receiveUntilFinished(&shared.freeReceiver, &shared.finish) else {
            return;
        };
        let mut pending = BTreeMap::new();
        let mut nextSeq = 0_u64;
        let mut channelFinished = false;
        loop {
            if !channelFinished {
                let Some(message) = receiveUntilFinished(receiver, &shared.finish) else {
                    return;
                };
                match message {
                    orderedMessage::Result(value) => {
                        if let Some(error) = value.err {
                            Self::putResult(&shared, None, Some(error));
                            return;
                        }
                        pending.insert(value.seq, value);
                    }
                    orderedMessage::Finished => channelFinished = true,
                }
            }

            while let Some(value) = pending.remove(&nextSeq) {
                nextSeq += 1;
                // 消费一个 pace 令牌，保证不超前于外层产出顺序。
                let pace = shared.paceReceiver.as_ref().expect("pace receiver missing");
                if receiveUntilFinished(pace, &shared.finish).is_none() {
                    return;
                }
                for chunk in value.chks {
                    for rowIndex in 0..chunk.NumRows() {
                        output.AppendRow(chunk.GetRow(rowIndex));
                        if output.IsFull() {
                            let Some(nextOutput) = Self::flushOutput(&shared, output) else {
                                return;
                            };
                            output = nextOutput;
                        }
                    }
                }
            }

            if channelFinished {
                if !pending.is_empty() {
                    Self::putResult(
                        &shared,
                        None,
                        Some(errors::New("ordered apply results contain a sequence gap")),
                    );
                    return;
                }
                if output.NumRows() > 0 {
                    Self::putResult(&shared, Some(output), None);
                }
                Self::putResult(&shared, None, None);
                return;
            }
            if output.NumRows() == 0 {
                continue;
            }
            match receiver
                .lock()
                .expect("ordered result receiver lock poisoned")
                .try_recv()
            {
                Ok(orderedMessage::Result(value)) => {
                    if let Some(error) = value.err {
                        Self::putResult(&shared, None, Some(error));
                        return;
                    }
                    pending.insert(value.seq, value);
                    continue;
                }
                Ok(orderedMessage::Finished) => {
                    channelFinished = true;
                    continue;
                }
                Err(TryRecvError::Empty) => {
                    let Some(nextOutput) = Self::flushOutput(&shared, output) else {
                        return;
                    };
                    output = nextOutput;
                }
                Err(TryRecvError::Disconnected) => return,
            }
        }
    }

    /// 刷出当前输出 Chunk 并换取回收的空 Chunk。
    fn flushOutput(shared: &parallelApplyShared, output: chunk::Chunk) -> Option<chunk::Chunk> {
        if Self::putResult(shared, Some(output), None) {
            return None;
        }
        let mut recycled = receiveUntilFinished(&shared.freeReceiver, &shared.finish)?;
        recycled.Reset();
        Some(recycled)
    }

    /// 发送有序结果；返回 true 表示被 finish 打断。
    fn sendOrdered(shared: &parallelApplyShared, value: orderedResult) -> bool {
        sendUntilFinished(
            shared
                .orderedSender
                .as_ref()
                .expect("ordered result sender missing"),
            &shared.finish,
            orderedMessage::Result(value),
        )
    }

    /// 向主结果通道投递；返回 true 表示被 finish 打断。
    fn putResult(
        shared: &parallelApplyShared,
        chk: Option<chunk::Chunk>,
        err: Option<errors::SharedError>,
    ) -> bool {
        sendUntilFinished(&shared.resultSender, &shared.finish, result { chk, err })
    }

    /// 将 worker panic 转为错误写入结果通道并打印回溯。
    fn handleWorkerPanic(shared: &parallelApplyShared, payload: Box<dyn Any + Send>) {
        let error = panicError(payload.as_ref());
        let message = error.to_string();
        Self::putResult(shared, None, Some(error));
        eprintln!(
            "parallel nested loop join worker panicked: {message}\n{}",
            Backtrace::force_capture()
        );
    }

    /// 绑定相关列后拉取该外层行的全部内层结果；命中缓存则跳过执行。
    fn fetchAllInners(shared: &parallelApplyShared, id: usize) -> Result<(), errors::SharedError> {
        let worker = &shared.workers[id];
        let (outer, corCols) = {
            let state = worker
                .state
                .lock()
                .expect("parallel apply worker state poisoned");
            (
                state
                    .outerRow
                    .clone()
                    .expect("parallel apply outer row missing"),
                state.corCols.clone(),
            )
        };
        shared.ctx.BindCorrelatedColumns(id, &outer, &corCols)?;
        let key = if shared.useCache {
            Some(shared.ctx.EncodeCorrelatedKey(id, &outer, &corCols)?)
        } else {
            None
        };
        // 缓存命中则直接替换 innerList。
        if let Some(key) = &key {
            shared.cacheAccessCounter.fetch_add(1, Ordering::AcqRel);
            shared.ctx.TriggerCacheGetFailpoint();
            if let Some(cached) = shared.ctx.ApplyCacheGet(key)? {
                worker
                    .state
                    .lock()
                    .expect("parallel apply worker state poisoned")
                    .innerList
                    .Replace(cached, &shared.memTracker);
                shared.cacheHitCounter.fetch_add(1, Ordering::AcqRel);
                return Ok(());
            }
        }

        {
            worker
                .innerExec
                .lock()
                .expect("parallel apply inner executor lock poisoned")
                .Open(Arc::clone(&shared.ctx))?;
        }
        // 真正执行内层：过滤后写入 innerList。
        let runResult = (|| {
            shared.ctx.TriggerSlowInnerFailpoint(id)?;
            {
                let mut state = worker
                    .state
                    .lock()
                    .expect("parallel apply worker state poisoned");
                state.innerList.Reset(&shared.memTracker);
            }
            loop {
                if shared.finish.IsFinished() {
                    return Ok(());
                }
                let mut input = {
                    let mut state = worker
                        .state
                        .lock()
                        .expect("parallel apply worker state poisoned");
                    state.innerChunk.take().unwrap_or_else(|| {
                        worker
                            .innerExec
                            .lock()
                            .expect("parallel apply inner executor lock poisoned")
                            .TryNewCacheChunk()
                    })
                };
                let next = worker
                    .innerExec
                    .lock()
                    .expect("parallel apply inner executor lock poisoned")
                    .Next(Arc::clone(&shared.ctx), &mut input);
                if let Err(error) = next {
                    worker
                        .state
                        .lock()
                        .expect("parallel apply worker state poisoned")
                        .innerChunk = Some(input);
                    return Err(error);
                }
                if input.NumRows() == 0 {
                    worker
                        .state
                        .lock()
                        .expect("parallel apply worker state poisoned")
                        .innerChunk = Some(input);
                    break;
                }
                let reuse = {
                    let mut state = worker
                        .state
                        .lock()
                        .expect("parallel apply worker state poisoned");
                    std::mem::take(&mut state.innerSelected)
                };
                let filters = {
                    worker
                        .state
                        .lock()
                        .expect("parallel apply worker state poisoned")
                        .innerFilter
                        .clone()
                };
                let selected = shared
                    .ctx
                    .VectorizedFilterInner(id, &input, &filters, reuse)?;
                if selected.len() != input.NumRows() {
                    return Err(errors::New("inner filter returned the wrong row count"));
                }
                let mut filtered = worker
                    .innerExec
                    .lock()
                    .expect("parallel apply inner executor lock poisoned")
                    .NewFirstChunk();
                for (rowIndex, include) in selected.iter().copied().enumerate() {
                    if include {
                        filtered.AppendRow(input.GetRow(rowIndex));
                    }
                }
                let mut state = worker
                    .state
                    .lock()
                    .expect("parallel apply worker state poisoned");
                state.innerSelected = selected;
                state.innerChunk = Some(input);
                if filtered.NumRows() > 0 {
                    state.innerList.Add(filtered, &shared.memTracker);
                }
            }
            Ok(())
        })();
        if let Err(error) = worker
            .innerExec
            .lock()
            .expect("parallel apply inner executor lock poisoned")
            .Close()
        {
            shared.ctx.LogInnerCloseError(id, &error);
        }
        runResult?;
        if shared.finish.IsFinished() {
            return Ok(());
        }
        // 成功执行后写入缓存快照。
        if let Some(key) = key {
            shared.ctx.TriggerCacheSetFailpoint();
            let chunks = worker
                .state
                .lock()
                .expect("parallel apply worker state poisoned")
                .innerList
                .Snapshot();
            shared.ctx.ApplyCacheSet(key, chunks)?;
        }
        Ok(())
    }

    /// 取下一通过过滤的外层行；未选中且为外连接时写入 OnMissMatch。
    fn fetchNextOuterRow(
        shared: &parallelApplyShared,
        id: usize,
        req: &mut chunk::Chunk,
    ) -> (Option<chunk::Row>, bool) {
        loop {
            let Some(value) = Self::receiveOuterRow(shared) else {
                return (None, shared.finish.IsFinished());
            };
            if !value.selected {
                if shared.outer {
                    shared.workers[id]
                        .state
                        .lock()
                        .expect("parallel apply worker state poisoned")
                        .joiner
                        .OnMissMatch(false, &value.row, req);
                    if req.IsFull() {
                        return (None, false);
                    }
                }
                continue;
            }
            return (Some(value.row), false);
        }
    }

    /// 从外层通道接收一行；外层已结束或 finish 时返回 None。
    fn receiveOuterRow(shared: &parallelApplyShared) -> Option<outerRow> {
        loop {
            if shared.finish.IsFinished() {
                return None;
            }
            match shared
                .outerReceiver
                .lock()
                .expect("parallel apply outer receiver lock poisoned")
                .recv_timeout(Duration::from_millis(10))
            {
                Ok(value) => return Some(value),
                Err(RecvTimeoutError::Timeout) => {
                    // 超时后若外层已标记结束则退出，避免空转。
                    if shared.outerFinished.load(Ordering::Acquire) {
                        return None;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    /// 乱序路径：驱动当前 worker 对外层行做连接，填满输出 Chunk。
    fn fillInnerChunk(
        shared: &parallelApplyShared,
        id: usize,
        req: &mut chunk::Chunk,
    ) -> Result<(), errors::SharedError> {
        req.Reset();
        loop {
            // 没有外层行或内层行已耗尽时换下一外层行。
            let needOuter = {
                let state = shared.workers[id]
                    .state
                    .lock()
                    .expect("parallel apply worker state poisoned");
                state.outerRow.is_none() || state.innerCursor >= state.innerList.Rows().len()
            };
            if needOuter {
                {
                    let mut state = shared.workers[id]
                        .state
                        .lock()
                        .expect("parallel apply worker state poisoned");
                    if let Some(outer) = state.outerRow.clone()
                        && !state.hasMatch
                    {
                        let hasNull = state.hasNull;
                        state.joiner.OnMissMatch(hasNull, &outer, req);
                    }
                }
                let (outer, exit) = Self::fetchNextOuterRow(shared, id, req);
                if exit || req.IsFull() || outer.is_none() {
                    return Ok(());
                }
                {
                    let mut state = shared.workers[id]
                        .state
                        .lock()
                        .expect("parallel apply worker state poisoned");
                    state.outerRow = outer;
                    state.hasMatch = false;
                    state.hasNull = false;
                    state.innerCursor = 0;
                }
                Self::fetchAllInners(shared, id)?;
            }

            let mut state = shared.workers[id]
                .state
                .lock()
                .expect("parallel apply worker state poisoned");
            let rows = state.innerList.Rows();
            let outer = state
                .outerRow
                .clone()
                .expect("parallel apply outer row missing");
            let before = state.innerCursor;
            let applyWorkerState {
                joiner,
                innerCursor,
                hasMatch,
                hasNull,
                ..
            } = &mut *state;
            let (matched, isNull) = joiner.TryToMatchInners(&outer, &rows, innerCursor, req)?;
            *hasMatch |= matched;
            *hasNull |= isNull;
            if *innerCursor == before && *innerCursor < rows.len() {
                return Err(errors::New(
                    "parallel apply joiner did not advance inner cursor",
                ));
            }
            if req.IsFull() {
                return Ok(());
            }
        }
    }
}

fn panicError(payload: &(dyn Any + Send)) -> errors::SharedError {
    // 尽量还原 panic 载荷中的字符串消息。
    if let Some(message) = payload.downcast_ref::<&str>() {
        errors::New((*message).to_owned())
    } else if let Some(message) = payload.downcast_ref::<String>() {
        errors::New(message.clone())
    } else {
        errors::New("parallel apply worker panicked")
    }
}
