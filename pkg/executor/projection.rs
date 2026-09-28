// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// Projection（投影）执行器：对子节点输出按表达式求值，产出选定列。
//
// 支持串行与并行两种路径：并行时由 Fetcher 拉取子结果、Worker 池做表达式求值，
// 通过有界 channel 与对象池复用 Chunk，并跟踪语句内存与运行时统计。

#![allow(non_camel_case_types, non_snake_case)]

use std::any::Any;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{
    Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError, sync_channel,
};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use astersql_executor_sortexec::{Row, SortError};

/// 并行读 channel 时的轮询间隔，兼顾 finish 标志响应与 CPU 占用。
const CHANNEL_POLL_INTERVAL: Duration = Duration::from_millis(2);

/// Applies the resolved-column projection used by the serial outer
/// Projection executor path. SQL row order and duplicate values are retained.
/// 串行外层投影：按列下标抽取行字段，保留行序与重复值。
pub fn ProjectRows(rows: Vec<Row>, columns: &[usize]) -> Result<Vec<Row>, SortError> {
    rows.into_iter()
        .map(|row| {
            columns
                .iter()
                .map(|index| {
                    row.0.get(*index).cloned().ok_or_else(|| {
                        SortError(format!(
                            "projection column index {index} exceeds row width {}",
                            row.0.len()
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Row)
        })
        .collect()
}

/// 并行路径的输入单元：待投影的 Chunk 与目标 worker 编号。
pub struct projectionInput<C> {
    pub chk: C,
    pub targetWorker: usize,
}

/// 并行路径的输出单元：共享 Chunk，以及完成信号的发送/接收端。
pub struct projectionOutput<C, E> {
    pub chk: Arc<Mutex<C>>,
    done_sender: SyncSender<Result<(), E>>,
    done_receiver: Arc<Mutex<Receiver<Result<(), E>>>>,
}

impl<C, E> Clone for projectionOutput<C, E> {
    fn clone(&self) -> Self {
        Self {
            chk: Arc::clone(&self.chk),
            done_sender: self.done_sender.clone(),
            done_receiver: Arc::clone(&self.done_receiver),
        }
    }
}

impl<C, E> projectionOutput<C, E> {
    /// 用给定 Chunk 构造输出槽，容量为 1 的完成通知通道。
    fn new(chunk: C) -> Self {
        let (done_sender, done_receiver) = sync_channel(1);
        Self {
            chk: Arc::new(Mutex::new(chunk)),
            done_sender,
            done_receiver: Arc::new(Mutex::new(done_receiver)),
        }
    }

    /// 阻塞发送完成结果（成功或错误）。
    fn signal(&self, result: Result<(), E>) {
        let _ = self.done_sender.send(result);
    }

    /// 非阻塞尝试发送完成结果（用于 panic 恢复路径）。
    fn try_signal(&self, result: Result<(), E>) {
        let _ = self.done_sender.try_send(result);
    }

    /// 等待 worker/fetcher 发出的完成信号。
    fn wait(&self) -> Option<Result<(), E>> {
        self.done_receiver.lock().ok()?.recv().ok()
    }
}

#[derive(Clone)]
/// 投影求值上下文：语句内存追踪、运行时统计收集器与求值环境。
pub struct projectionExecutorContext<M, R, V> {
    pub stmtMemTracker: M,
    pub stmtRuntimeStatsColl: R,
    pub evalCtx: V,
    pub enableVectorizedExpression: bool,
}

/// Production boundary for BaseExecutorV2, chunks, expression evaluation,
/// memory/runtime tracking, tracing, failpoint recovery, and the child executor.
/// 生产路径边界：BaseExecutor、Chunk、表达式求值、内存/统计、追踪与子执行器。
pub trait ProjectionBackend: Send + Sync + 'static {
    type Context: Clone + Send + Sync + 'static;
    type Error: Send + 'static;
    type Chunk: Send + 'static;
    type EvaluatorSuite: Send + Sync + 'static;
    type EvalContext: Clone + Send + Sync + 'static;
    type MemoryTracker: Clone + Send + Sync + 'static;
    type RuntimeStatsCollection: Clone + Send + Sync + 'static;
    type TraceGuard: Send + 'static;

    fn error(&self, message: String) -> Self::Error;
    fn base_open(&self, context: &Self::Context) -> Result<(), Self::Error>;
    fn base_close(&self) -> Result<(), Self::Error>;
    fn executor_id(&self) -> i32;
    fn max_chunk_size(&self) -> usize;

    fn statement_memory_tracker(&self) -> Self::MemoryTracker;
    fn statement_runtime_stats_collection(&self) -> Self::RuntimeStatsCollection;
    fn evaluation_context(&self) -> Self::EvalContext;
    fn vectorized_expression_enabled(&self) -> bool;
    fn new_memory_tracker(&self, executor_id: i32) -> Self::MemoryTracker;
    fn reset_memory_tracker(&self, tracker: &Self::MemoryTracker);
    fn attach_memory_tracker(&self, tracker: &Self::MemoryTracker, parent: &Self::MemoryTracker);
    fn consume_memory(&self, tracker: &Self::MemoryTracker, bytes: i64);

    fn evaluator_vectorizable(&self, evaluator: &Self::EvaluatorSuite) -> bool;
    fn new_child_chunk(&self) -> Self::Chunk;
    fn new_output_chunk(&self) -> Self::Chunk;
    fn chunk_memory_usage(&self, chunk: &Self::Chunk) -> i64;
    fn chunk_grow_and_reset(&self, chunk: &mut Self::Chunk, max_chunk_size: usize);
    fn chunk_required_rows(&self, chunk: &Self::Chunk) -> usize;
    fn chunk_set_required_rows(
        &self,
        chunk: &mut Self::Chunk,
        required_rows: usize,
        max_chunk_size: usize,
    );
    fn chunk_num_rows(&self, chunk: &Self::Chunk) -> usize;
    fn chunk_swap_columns(&self, destination: &mut Self::Chunk, source: &mut Self::Chunk);
    fn child_next(
        &self,
        context: &Self::Context,
        chunk: &mut Self::Chunk,
    ) -> Result<(), Self::Error>;
    fn evaluate(
        &self,
        eval_context: &Self::EvalContext,
        vectorized: bool,
        evaluator: &Self::EvaluatorSuite,
        input: &mut Self::Chunk,
        output: &mut Self::Chunk,
    ) -> Result<(), Self::Error>;

    fn runtime_stats_enabled(&self) -> bool;
    fn register_projection_concurrency(
        &self,
        collection: &Self::RuntimeStatsCollection,
        executor_id: i32,
        concurrency: usize,
    );
    fn start_trace_region(&self, context: &Self::Context, name: &str) -> Self::TraceGuard;
    fn recovered_panic_error(&self, panic: &(dyn Any + Send)) -> Self::Error;
    fn log_projection_panic(&self, panic: &(dyn Any + Send));
}

/// 从 backend 快照构造投影执行上下文。
pub fn newProjectionExecutorContext<B: ProjectionBackend>(
    backend: &B,
) -> projectionExecutorContext<B::MemoryTracker, B::RuntimeStatsCollection, B::EvalContext> {
    projectionExecutorContext {
        stmtMemTracker: backend.statement_memory_tracker(),
        stmtRuntimeStatsColl: backend.statement_runtime_stats_collection(),
        evalCtx: backend.evaluation_context(),
        enableVectorizedExpression: backend.vectorized_expression_enabled(),
    }
}

/// 并行投影运行时状态：finish 标志、全局输出、输入/输出池与 worker 通道、线程句柄。
struct ProjectionParallelState<B: ProjectionBackend> {
    finish: Arc<AtomicBool>,
    global_output_receiver: Arc<Mutex<Receiver<projectionOutput<B::Chunk, B::Error>>>>,
    output_pool_sender: SyncSender<projectionOutput<B::Chunk, B::Error>>,
    input_pool_receiver: Arc<Mutex<Receiver<projectionInput<B::Chunk>>>>,
    output_pool_receiver: Arc<Mutex<Receiver<projectionOutput<B::Chunk, B::Error>>>>,
    worker_input_receivers: Vec<Arc<Mutex<Receiver<projectionInput<B::Chunk>>>>>,
    worker_output_receivers: Vec<Arc<Mutex<Receiver<projectionOutput<B::Chunk, B::Error>>>>>,
    threads: Vec<JoinHandle<()>>,
}

/// Projection 执行器：可在串行（numWorkers<=0）与并行模式间切换。
pub struct ProjectionExec<B: ProjectionBackend> {
    pub projectionExecutorContext:
        projectionExecutorContext<B::MemoryTracker, B::RuntimeStatsCollection, B::EvalContext>,
    pub backend: Arc<B>,
    pub evaluatorSuit: Arc<B::EvaluatorSuite>,
    pub numWorkers: i64,
    pub childResult: Option<B::Chunk>,
    pub parentReqRows: Arc<AtomicI64>,
    pub memTracker: Option<B::MemoryTracker>,
    pub calculateNoDelay: bool,
    pub prepared: bool,
    parallel: Option<ProjectionParallelState<B>>,
}

impl<B: ProjectionBackend> ProjectionExec<B> {
    /// 打开基类后初始化投影内部状态（内存追踪、子 Chunk 或并行准备标志）。
    pub fn Open(&mut self, context: &B::Context) -> Result<(), B::Error> {
        self.backend.base_open(context)?;
        self.open(context)
    }

    /// 重置 prepared；挂接内存追踪；不可向量化时强制串行；串行则预分配子 Chunk。
    pub fn open(&mut self, _context: &B::Context) -> Result<(), B::Error> {
        self.prepared = false;
        self.parentReqRows
            .store(self.backend.max_chunk_size() as i64, Ordering::Release);

        let tracker = match self.memTracker.take() {
            Some(tracker) => {
                self.backend.reset_memory_tracker(&tracker);
                tracker
            }
            None => self.backend.new_memory_tracker(self.backend.executor_id()),
        };
        self.backend
            .attach_memory_tracker(&tracker, &self.projectionExecutorContext.stmtMemTracker);
        self.memTracker = Some(tracker);

        // 表达式套件不可向量化时，并行路径不安全，回退为串行。
        if self.numWorkers > 0 && !self.backend.evaluator_vectorizable(&self.evaluatorSuit) {
            self.numWorkers = 0;
        }

        self.childResult = None;
        self.parallel = None;
        if self.isUnparallelExec() {
            let chunk = self.backend.new_child_chunk();
            self.consume(self.backend.chunk_memory_usage(&chunk));
            self.childResult = Some(chunk);
        }
        Ok(())
    }

    /// 按是否并行分发到 unParallelExecute / parallelExecute。
    pub fn Next(&mut self, context: &B::Context, request: &mut B::Chunk) -> Result<(), B::Error> {
        self.backend
            .chunk_grow_and_reset(request, self.backend.max_chunk_size());
        if self.isUnparallelExec() {
            self.unParallelExecute(context, request)
        } else {
            self.parallelExecute(context, request)
        }
    }

    /// numWorkers<=0 表示串行投影。
    pub fn isUnparallelExec(&self) -> bool {
        self.numWorkers <= 0
    }

    /// 串行：从子执行器取一批行，再对本批做表达式求值写入 request。
    pub fn unParallelExecute(
        &mut self,
        context: &B::Context,
        request: &mut B::Chunk,
    ) -> Result<(), B::Error> {
        let max_chunk_size = self.backend.max_chunk_size();
        let required_rows = self.backend.chunk_required_rows(request);
        let tracker = self
            .memTracker
            .as_ref()
            .expect("projection memory tracker must be initialized")
            .clone();
        let child_result = self
            .childResult
            .as_mut()
            .expect("serial projection chunk must be initialized");

        self.backend
            .chunk_set_required_rows(child_result, required_rows, max_chunk_size);
        let old_size = self.backend.chunk_memory_usage(child_result);
        let child_result_status = self.backend.child_next(context, child_result);
        let new_size = self.backend.chunk_memory_usage(child_result);
        self.backend
            .consume_memory(&tracker, new_size.saturating_sub(old_size));
        child_result_status?;
        if self.backend.chunk_num_rows(child_result) == 0 {
            return Ok(());
        }
        self.backend.evaluate(
            &self.projectionExecutorContext.evalCtx,
            self.projectionExecutorContext.enableVectorizedExpression,
            &self.evaluatorSuit,
            child_result,
            request,
        )
    }

    /// 并行：首次 prepare 启动 Fetcher/Worker；再从全局输出通道取结果并交换列。
    pub fn parallelExecute(
        &mut self,
        context: &B::Context,
        request: &mut B::Chunk,
    ) -> Result<(), B::Error> {
        self.parentReqRows.store(
            self.backend.chunk_required_rows(request) as i64,
            Ordering::Release,
        );
        // 惰性启动并行流水线，仅准备一次。
        if !self.prepared {
            self.prepare(context);
            self.prepared = true;
        }

        let state = self
            .parallel
            .as_ref()
            .expect("parallel projection state must be prepared");
        let output = match state.global_output_receiver.lock() {
            Ok(receiver) => match receiver.recv() {
                Ok(output) => output,
                Err(_) => return Ok(()),
            },
            Err(_) => {
                return Err(self
                    .backend
                    .error("projection output channel lock poisoned".to_owned()));
            }
        };

        match output.wait() {
            Some(Ok(())) => {}
            Some(Err(error)) => return Err(error),
            None => {
                return Err(self
                    .backend
                    .error("projection output completion channel closed".to_owned()));
            }
        }

        let mut output_chunk = output.chk.lock().map_err(|_| {
            self.backend
                .error("projection output chunk lock poisoned".to_owned())
        })?;
        let old_size = self.backend.chunk_memory_usage(&output_chunk);
        self.backend.chunk_swap_columns(request, &mut output_chunk);
        let new_size = self.backend.chunk_memory_usage(&output_chunk);
        self.consume(new_size.saturating_sub(old_size));
        drop(output_chunk);

        state.output_pool_sender.send(output).map_err(|_| {
            self.backend
                .error("projection output pool closed".to_owned())
        })
    }

    /// 构建输入/输出对象池、每 worker 通道，并拉起 Fetcher 与 Worker 线程。
    pub fn prepare(&mut self, context: &B::Context) {
        let worker_count = self.numWorkers as usize;
        let finish = Arc::new(AtomicBool::new(false));
        let (global_output_sender, global_output_receiver) = sync_channel(worker_count);
        let (input_pool_sender, input_pool_receiver) = sync_channel(worker_count);
        let (output_pool_sender, output_pool_receiver) = sync_channel(worker_count);
        let input_pool_receiver = Arc::new(Mutex::new(input_pool_receiver));
        let output_pool_receiver = Arc::new(Mutex::new(output_pool_receiver));

        let mut worker_input_senders = Vec::with_capacity(worker_count);
        let mut worker_output_senders = Vec::with_capacity(worker_count);
        let mut worker_input_receivers = Vec::with_capacity(worker_count);
        let mut worker_output_receivers = Vec::with_capacity(worker_count);
        let mut workers = Vec::with_capacity(worker_count);

        // 为每个 worker 建立专用输入/输出通道，并向池中预填 Chunk。
        for worker_id in 0..worker_count {
            let (worker_input_sender, worker_input_receiver) = sync_channel(1);
            let (worker_output_sender, worker_output_receiver) = sync_channel(1);
            let worker_input_receiver = Arc::new(Mutex::new(worker_input_receiver));
            let worker_output_receiver = Arc::new(Mutex::new(worker_output_receiver));

            worker_input_senders.push(worker_input_sender);
            worker_output_senders.push(worker_output_sender);
            worker_input_receivers.push(Arc::clone(&worker_input_receiver));
            worker_output_receivers.push(Arc::clone(&worker_output_receiver));
            workers.push(projectionWorker {
                backend: Arc::clone(&self.backend),
                projectionContext: self.projectionExecutorContext.clone(),
                evaluatorSuit: Arc::clone(&self.evaluatorSuit),
                memTracker: self
                    .memTracker
                    .as_ref()
                    .expect("projection memory tracker must be initialized")
                    .clone(),
                globalFinishCh: Arc::clone(&finish),
                inputGiveBackCh: input_pool_sender.clone(),
                inputCh: worker_input_receiver,
                outputCh: worker_output_receiver,
                context: context.clone(),
            });

            let input_chunk = self.backend.new_child_chunk();
            self.consume(self.backend.chunk_memory_usage(&input_chunk));
            input_pool_sender
                .send(projectionInput {
                    chk: input_chunk,
                    targetWorker: worker_id,
                })
                .expect("projection input pool must be open during preparation");

            let output_chunk = self.backend.new_output_chunk();
            self.consume(self.backend.chunk_memory_usage(&output_chunk));
            output_pool_sender
                .send(projectionOutput::new(output_chunk))
                .expect("projection output pool must be open during preparation");
        }

        let fetcher = projectionInputFetcher {
            backend: Arc::clone(&self.backend),
            context: context.clone(),
            memTracker: self
                .memTracker
                .as_ref()
                .expect("projection memory tracker must be initialized")
                .clone(),
            parentReqRows: Arc::clone(&self.parentReqRows),
            globalFinishCh: Arc::clone(&finish),
            globalOutputCh: global_output_sender,
            inputCh: Arc::clone(&input_pool_receiver),
            outputCh: Arc::clone(&output_pool_receiver),
            workerInputCh: worker_input_senders,
            workerOutputCh: worker_output_senders,
        };

        let mut threads = Vec::with_capacity(worker_count + 1);
        threads.push(thread::spawn(move || fetcher.run()));
        for worker in workers {
            threads.push(thread::spawn(move || worker.run()));
        }

        self.parallel = Some(ProjectionParallelState {
            finish,
            global_output_receiver: Arc::new(Mutex::new(global_output_receiver)),
            output_pool_sender,
            input_pool_receiver,
            output_pool_receiver,
            worker_input_receivers,
            worker_output_receivers,
            threads,
        });
    }

    /// 排空输入通道并回退对应 Chunk 的内存记账。
    pub fn drainInputCh(&self, receiver: &Arc<Mutex<Receiver<projectionInput<B::Chunk>>>>) {
        let Ok(receiver) = receiver.lock() else {
            return;
        };
        loop {
            match receiver.try_recv() {
                Ok(input) => self.consume(-self.backend.chunk_memory_usage(&input.chk)),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
            }
        }
    }

    /// 排空输出通道并回退对应 Chunk 的内存记账。
    pub fn drainOutputCh(
        &self,
        receiver: &Arc<Mutex<Receiver<projectionOutput<B::Chunk, B::Error>>>>,
    ) {
        let Ok(receiver) = receiver.lock() else {
            return;
        };
        loop {
            match receiver.try_recv() {
                Ok(output) => {
                    if let Ok(chunk) = output.chk.lock() {
                        self.consume(-self.backend.chunk_memory_usage(&chunk));
                    }
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
            }
        }
    }

    /// 关闭：释放串行 Chunk 或置 finish 并 join 并行线程，登记并发度后 base_close。
    pub fn Close(&mut self) -> Result<(), B::Error> {
        if self.isUnparallelExec()
            && let Some(child_result) = self.childResult.take()
        {
            self.consume(-self.backend.chunk_memory_usage(&child_result));
        }

        if self.prepared
            && let Some(mut parallel) = self.parallel.take()
        {
            parallel.finish.store(true, Ordering::Release);
            for thread in parallel.threads.drain(..) {
                let _ = thread.join();
            }

            self.drainInputCh(&parallel.input_pool_receiver);
            self.drainOutputCh(&parallel.output_pool_receiver);
            for receiver in &parallel.worker_input_receivers {
                self.drainInputCh(receiver);
            }
            for receiver in &parallel.worker_output_receivers {
                self.drainOutputCh(receiver);
            }
        }

        if self.backend.runtime_stats_enabled() {
            let concurrency = if self.isUnparallelExec() {
                0
            } else {
                self.numWorkers as usize
            };
            self.backend.register_projection_concurrency(
                &self.projectionExecutorContext.stmtRuntimeStatsColl,
                self.backend.executor_id(),
                concurrency,
            );
        }
        self.backend.base_close()
    }

    /// 向语句内存追踪器增减字节（正增负减）。
    fn consume(&self, bytes: i64) {
        if let Some(tracker) = self.memTracker.as_ref() {
            self.backend.consume_memory(tracker, bytes);
        }
    }
}

/// 并行 Fetcher：从子执行器取数，配对输出槽后派发给目标 worker。
pub struct projectionInputFetcher<B: ProjectionBackend> {
    backend: Arc<B>,
    context: B::Context,
    memTracker: B::MemoryTracker,
    parentReqRows: Arc<AtomicI64>,
    globalFinishCh: Arc<AtomicBool>,
    globalOutputCh: SyncSender<projectionOutput<B::Chunk, B::Error>>,
    inputCh: Arc<Mutex<Receiver<projectionInput<B::Chunk>>>>,
    outputCh: Arc<Mutex<Receiver<projectionOutput<B::Chunk, B::Error>>>>,
    workerInputCh: Vec<SyncSender<projectionInput<B::Chunk>>>,
    workerOutputCh: Vec<SyncSender<projectionOutput<B::Chunk, B::Error>>>,
}

impl<B: ProjectionBackend> projectionInputFetcher<B> {
    /// Fetcher 主循环入口；捕获 panic 后通过 recoveryProjection 通知下游。
    pub fn run(self) {
        let _trace = self
            .backend
            .start_trace_region(&self.context, "ProjectionFetcher");
        let current_output = Arc::new(Mutex::new(None));
        let recovery_output = Arc::clone(&current_output);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.run_loop(&current_output)
        }));
        if let Err(panic) = result {
            let output = recovery_output
                .lock()
                .ok()
                .and_then(|output| output.clone());
            recoveryProjection(self.backend.as_ref(), output.as_ref(), panic);
        }
    }

    /// 循环：取 input/output → 推全局输出 → child_next → 派发到对应 worker。
    fn run_loop(&self, current_output: &Arc<Mutex<Option<projectionOutput<B::Chunk, B::Error>>>>) {
        loop {
            let (input, finished) = readProjection(&self.inputCh, &self.globalFinishCh);
            if finished {
                return;
            }
            let mut input = input.expect("open projection input channel returned no item");
            let target_worker = input.targetWorker;

            let (output, finished) = readProjection(&self.outputCh, &self.globalFinishCh);
            if finished {
                self.backend.consume_memory(
                    &self.memTracker,
                    -self.backend.chunk_memory_usage(&input.chk),
                );
                return;
            }
            let output = output.expect("open projection output channel returned no item");
            if let Ok(mut current) = current_output.lock() {
                *current = Some(output.clone());
            }

            if !sendProjection(&self.globalOutputCh, output.clone(), &self.globalFinishCh) {
                return;
            }

            let required_rows = self.parentReqRows.load(Ordering::Acquire).max(0) as usize;
            self.backend.chunk_set_required_rows(
                &mut input.chk,
                required_rows,
                self.backend.max_chunk_size(),
            );
            let old_size = self.backend.chunk_memory_usage(&input.chk);
            let child_status = self.backend.child_next(&self.context, &mut input.chk);
            let new_size = self.backend.chunk_memory_usage(&input.chk);
            self.backend
                .consume_memory(&self.memTracker, new_size.saturating_sub(old_size));

            // 子节点结束或出错：把状态写入 output 并退出 Fetcher。
            if child_status.is_err() || self.backend.chunk_num_rows(&input.chk) == 0 {
                output.signal(child_status);
                self.backend.consume_memory(
                    &self.memTracker,
                    -self.backend.chunk_memory_usage(&input.chk),
                );
                return;
            }

            if !sendProjection(
                &self.workerInputCh[target_worker],
                input,
                &self.globalFinishCh,
            ) {
                return;
            }
            if !sendProjection(
                &self.workerOutputCh[target_worker],
                output,
                &self.globalFinishCh,
            ) {
                return;
            }
        }
    }
}

/// 并行 Worker：对分配到的输入 Chunk 做表达式求值，写回输出 Chunk。
pub struct projectionWorker<B: ProjectionBackend> {
    backend: Arc<B>,
    projectionContext:
        projectionExecutorContext<B::MemoryTracker, B::RuntimeStatsCollection, B::EvalContext>,
    evaluatorSuit: Arc<B::EvaluatorSuite>,
    memTracker: B::MemoryTracker,
    globalFinishCh: Arc<AtomicBool>,
    inputGiveBackCh: SyncSender<projectionInput<B::Chunk>>,
    inputCh: Arc<Mutex<Receiver<projectionInput<B::Chunk>>>>,
    outputCh: Arc<Mutex<Receiver<projectionOutput<B::Chunk, B::Error>>>>,
    context: B::Context,
}

impl<B: ProjectionBackend> projectionWorker<B> {
    /// Worker 主循环入口；panic 时同样走 recoveryProjection。
    pub fn run(self) {
        let _trace = self
            .backend
            .start_trace_region(&self.context, "ProjectionWorker");
        let current_output = Arc::new(Mutex::new(None));
        let recovery_output = Arc::clone(&current_output);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.run_loop(&current_output)
        }));
        if let Err(panic) = result {
            let output = recovery_output
                .lock()
                .ok()
                .and_then(|output| output.clone());
            recoveryProjection(self.backend.as_ref(), output.as_ref(), panic);
        }
    }

    /// 循环：收 input/output → evaluate → signal → 归还 input 到池。
    fn run_loop(&self, current_output: &Arc<Mutex<Option<projectionOutput<B::Chunk, B::Error>>>>) {
        loop {
            let (input, finished) = readProjection(&self.inputCh, &self.globalFinishCh);
            if finished {
                return;
            }
            let mut input = input.expect("open worker input channel returned no item");

            let (output, finished) = readProjection(&self.outputCh, &self.globalFinishCh);
            if finished {
                return;
            }
            let output = output.expect("open worker output channel returned no item");
            if let Ok(mut current) = current_output.lock() {
                *current = Some(output.clone());
            }

            let mut output_chunk = match output.chk.lock() {
                Ok(chunk) => chunk,
                Err(_) => {
                    output.signal(Err(self
                        .backend
                        .error("projection output chunk lock poisoned".to_owned())));
                    return;
                }
            };
            let old_size = self.backend.chunk_memory_usage(&output_chunk)
                + self.backend.chunk_memory_usage(&input.chk);
            let evaluation = self.backend.evaluate(
                &self.projectionContext.evalCtx,
                self.projectionContext.enableVectorizedExpression,
                &self.evaluatorSuit,
                &mut input.chk,
                &mut output_chunk,
            );
            let new_size = self.backend.chunk_memory_usage(&output_chunk)
                + self.backend.chunk_memory_usage(&input.chk);
            self.backend
                .consume_memory(&self.memTracker, new_size.saturating_sub(old_size));
            drop(output_chunk);

            match evaluation {
                Ok(()) => output.signal(Ok(())),
                Err(error) => {
                    output.signal(Err(error));
                    return;
                }
            }

            if !sendProjection(&self.inputGiveBackCh, input, &self.globalFinishCh) {
                return;
            }
        }
    }
}

/// panic 恢复：尽量向当前 output 投递错误，并记录日志。
pub fn recoveryProjection<B: ProjectionBackend>(
    backend: &B,
    output: Option<&projectionOutput<B::Chunk, B::Error>>,
    panic: Box<dyn Any + Send>,
) {
    if let Some(output) = output {
        output.try_signal(Err(backend.recovered_panic_error(panic.as_ref())));
    }
    backend.log_projection_panic(panic.as_ref());
}

/// 在 finish 未置位时带超时地从 receiver 读取一项；返回 (值, 是否结束)。
pub fn readProjection<T>(
    receiver: &Arc<Mutex<Receiver<T>>>,
    finish: &Arc<AtomicBool>,
) -> (Option<T>, bool) {
    loop {
        if finish.load(Ordering::Acquire) {
            return (None, true);
        }
        let result = match receiver.lock() {
            Ok(receiver) => receiver.recv_timeout(CHANNEL_POLL_INTERVAL),
            Err(_) => return (None, true),
        };
        match result {
            Ok(value) => return (Some(value), false),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return (None, true),
        }
    }
}

/// 在 finish 未置位时非阻塞发送；通道满则 yield 重试，断开则失败。
fn sendProjection<T>(sender: &SyncSender<T>, mut value: T, finish: &Arc<AtomicBool>) -> bool {
    loop {
        if finish.load(Ordering::Acquire) {
            return false;
        }
        match sender.try_send(value) {
            Ok(()) => return true,
            Err(TrySendError::Full(returned)) => {
                value = returned;
                thread::yield_now();
            }
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
}
