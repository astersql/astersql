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

// Shuffle 并行执行算子：多数据源按分区键拆分到多个 worker 并发消费。
//
// Shuffle（混洗）在执行计划中把上游行按哈希或有序分组重新分发到 N 个并发 worker，
// 以便下游算子（如并行聚合）获得分区局部性。通道采用带缓冲的 SyncSender，并用
// `finishSignal` 在关闭时打断阻塞发送/接收。

#![allow(non_camel_case_types, non_snake_case)]

use std::any::Any;
use std::backtrace::Backtrace;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use astersql_errors as errors;
use astersql_util_chunk as chunk;

/// 分区/分组表达式的类型擦除句柄（对应 Go 侧 expression）。
pub type ShuffleExpression = Arc<dyn Any + Send + Sync>;

/// Executor boundary used by shuffle. Implementations perform the real child/source work;
/// no successful fallback implementation is provided.
pub trait ShuffleExecutor: Send {
    fn Open(&mut self, ctx: Arc<dyn ShuffleRuntimeContext>) -> Result<(), errors::SharedError>;
    fn Close(&mut self) -> Result<(), errors::SharedError>;
    fn Next(
        &mut self,
        ctx: Arc<dyn ShuffleRuntimeContext>,
        req: &mut chunk::Chunk,
    ) -> Result<(), errors::SharedError>;
    fn NewFirstChunk(&mut self) -> chunk::Chunk;
    fn TryNewCacheChunk(&mut self) -> chunk::Chunk;
    fn HasRuntimeStats(&self) -> bool;
}

/// 将 Chunk 按分组键切成连续行区间，供 range splitter 轮询分配 worker。
pub trait ShuffleGroupChecker: Send {
    fn SplitIntoGroups(&mut self, input: &chunk::Chunk) -> Result<(), errors::SharedError>;
    fn IsExhausted(&self) -> bool;
    fn GetNextGroup(&mut self) -> (usize, usize);
}

/// Session/evaluation/failpoint boundary required by the concurrent topology.
pub trait ShuffleRuntimeContext: Send + Sync {
    fn GetGroupKey(
        &self,
        input: &chunk::Chunk,
        reuse: Vec<Vec<u8>>,
        byItems: &[ShuffleExpression],
    ) -> Result<Vec<Vec<u8>>, errors::SharedError>;
    fn NewGroupChecker(&self, byItems: &[ShuffleExpression]) -> Box<dyn ShuffleGroupChecker>;
    fn ShuffleNextError(&self) -> Option<errors::SharedError>;
    fn TriggerSourceFailpoint(&self);
    fn TriggerWorkerFailpoint(&self);
    fn InTest(&self) -> bool;
    fn RegisterConcurrencyStats(&self, name: &str, concurrency: usize);
}

#[derive(Default)]
/// 关闭信号：原子 finished 标志 + Condvar，供短暂等待后重试发送。
struct finishSignal {
    finished: AtomicBool,
    changed: Condvar,
    lock: Mutex<()>,
}

impl finishSignal {
    /// 标记结束并唤醒所有等待者。
    fn Finish(&self) {
        self.finished.store(true, Ordering::Release);
        self.changed.notify_all();
    }

    /// 是否已收到结束信号。
    fn IsFinished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    /// 短暂等待（约 2ms），避免忙等占满 CPU。
    fn WaitBriefly(&self) {
        let guard = self.lock.lock().expect("shuffle finish lock poisoned");
        let _ = self
            .changed
            .wait_timeout(guard, Duration::from_millis(2))
            .expect("shuffle finish lock poisoned while waiting");
    }
}

/// 向同步通道发送，通道满则等待；若已 finish 则归还未发送值。
fn sendUntilFinished<T>(
    sender: &SyncSender<T>,
    finish: &finishSignal,
    mut value: T,
) -> Result<(), T> {
    loop {
        if finish.IsFinished() {
            return Err(value);
        }
        match sender.try_send(value) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Full(returned)) => {
                value = returned;
                finish.WaitBriefly();
            }
            Err(TrySendError::Disconnected(returned)) => return Err(returned),
        }
    }
}

/// 带超时接收；finish 或断开时返回 None。
fn receiveUntilFinished<T>(receiver: &Arc<Mutex<Receiver<T>>>, finish: &finishSignal) -> Option<T> {
    loop {
        if finish.IsFinished() {
            return None;
        }
        let result = receiver
            .lock()
            .expect("shuffle channel receiver lock poisoned")
            .recv_timeout(Duration::from_millis(10));
        match result {
            Ok(value) => return Some(value),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

/// Worker 产出：可选 Chunk、错误，以及归还空 Chunk 的回收通道。
pub struct shuffleOutput {
    chk: Option<chunk::Chunk>,
    err: Option<errors::SharedError>,
    giveBackCh: Option<SyncSender<chunk::Chunk>>,
}

/// 协调通道消息：正常输出或全部结束。
enum shuffleMessage {
    Output(shuffleOutput),
    Finished,
}

#[derive(Clone)]
/// 单个 source→worker 路由：输入通道与空 Chunk 回收通道。
struct sourceRoute {
    inputCh: SyncSender<chunk::Chunk>,
    inputHolderCh: Arc<Mutex<Receiver<chunk::Chunk>>>,
}

/// ShuffleExec runs M sources and N child workers concurrently.
pub struct ShuffleExec {
    pub base: Arc<Mutex<Box<dyn ShuffleExecutor>>>,
    pub concurrency: usize,
    pub workers: Vec<Arc<Mutex<shuffleWorker>>>,
    pub splitters: Vec<Arc<Mutex<Box<dyn partitionSplitter>>>>,
    pub dataSources: Vec<Arc<Mutex<Box<dyn ShuffleExecutor>>>>,

    prepared: bool,
    executed: bool,
    finishCh: Arc<finishSignal>,
    outputCh: Option<SyncSender<shuffleMessage>>,
    outputReceiver: Option<Receiver<shuffleMessage>>,
    sourceRoutes: Vec<Vec<sourceRoute>>,
    coordinator: Option<JoinHandle<()>>,
    allSourceAndWorkerExitForTest: Arc<AtomicBool>,
    inTest: bool,
    runtimeContext: Option<Arc<dyn ShuffleRuntimeContext>>,
}

impl ShuffleExec {
    /// 校验 concurrency/workers/splitters 数量一致后构造未 Open 的 ShuffleExec。
    /// 由 child 执行器与各 source 的 receiver 列表构造。
    pub fn new(
        base: Box<dyn ShuffleExecutor>,
        concurrency: usize,
        workers: Vec<Arc<Mutex<shuffleWorker>>>,
        splitters: Vec<Box<dyn partitionSplitter>>,
        dataSources: Vec<Box<dyn ShuffleExecutor>>,
    ) -> Self {
        assert!(concurrency > 0, "shuffle concurrency must be positive");
        assert_eq!(
            concurrency,
            workers.len(),
            "one worker is required per partition"
        );
        assert_eq!(
            splitters.len(),
            dataSources.len(),
            "one partition splitter is required per data source"
        );
        Self {
            base: Arc::new(Mutex::new(base)),
            concurrency,
            workers,
            splitters: splitters
                .into_iter()
                .map(|splitter| Arc::new(Mutex::new(splitter)))
                .collect(),
            dataSources: dataSources
                .into_iter()
                .map(|source| Arc::new(Mutex::new(source)))
                .collect(),
            prepared: false,
            executed: false,
            finishCh: Arc::new(finishSignal::default()),
            outputCh: None,
            outputReceiver: None,
            sourceRoutes: Vec::new(),
            coordinator: None,
            allSourceAndWorkerExitForTest: Arc::new(AtomicBool::new(true)),
            inTest: false,
            runtimeContext: None,
        }
    }

    /// 打开各 source/base/child，安装通道并预填首个空 Chunk。
    /// 打开内嵌 base 并重置 executed。
    pub fn Open(&mut self, ctx: Arc<dyn ShuffleRuntimeContext>) -> Result<(), errors::SharedError> {
        for source in &self.dataSources {
            source
                .lock()
                .expect("shuffle data source lock poisoned")
                .Open(Arc::clone(&ctx))?;
        }
        self.base
            .lock()
            .expect("shuffle base executor lock poisoned")
            .Open(Arc::clone(&ctx))?;

        self.prepared = false;
        self.executed = false;
        self.finishCh = Arc::new(finishSignal::default());
        self.inTest = ctx.InTest();
        self.runtimeContext = Some(Arc::clone(&ctx));
        let (outputSender, outputReceiver) =
            mpsc::sync_channel(self.concurrency + self.dataSources.len());
        self.outputCh = Some(outputSender.clone());
        self.outputReceiver = Some(outputReceiver);
        self.sourceRoutes = (0..self.dataSources.len()).map(|_| Vec::new()).collect();

        // 为每个 worker 安装与各 source 的一对通道，并 Open child。
        for worker in &self.workers {
            let mut worker = worker.lock().expect("shuffle worker lock poisoned");
            worker.finishCh = Arc::clone(&self.finishCh);
            worker.outputCh = Some(outputSender.clone());

            for (sourceIndex, receiver) in worker.receivers.iter().enumerate() {
                let (inputSender, inputReceiver) = mpsc::sync_channel(1);
                let (holderSender, holderReceiver) = mpsc::sync_channel(1);
                receiver
                    .lock()
                    .expect("shuffle receiver lock poisoned")
                    .InstallChannels(
                        Arc::clone(&self.finishCh),
                        inputReceiver,
                        holderSender.clone(),
                    );
                self.sourceRoutes[sourceIndex].push(sourceRoute {
                    inputCh: inputSender,
                    inputHolderCh: Arc::new(Mutex::new(holderReceiver)),
                });
            }

            let (outputHolderSender, outputHolderReceiver) = mpsc::sync_channel(1);
            worker.outputHolderCh = Some(Arc::new(Mutex::new(outputHolderReceiver)));
            worker.outputHolderSender = Some(outputHolderSender.clone());
            worker
                .childExec
                .lock()
                .expect("shuffle child executor lock poisoned")
                .Open(Arc::clone(&ctx))?;

            for (sourceIndex, receiver) in worker.receivers.iter().enumerate() {
                let firstChunk = self.dataSources[sourceIndex]
                    .lock()
                    .expect("shuffle data source lock poisoned")
                    .NewFirstChunk();
                receiver
                    .lock()
                    .expect("shuffle receiver lock poisoned")
                    .SeedInputHolder(firstChunk)?;
            }
            let firstOutput = self
                .base
                .lock()
                .expect("shuffle base executor lock poisoned")
                .NewFirstChunk();
            outputHolderSender
                .send(firstOutput)
                .map_err(|_| errors::New("shuffle output holder disconnected during open"))?;
        }
        Ok(())
    }

    /// 发 finish、排空输出、关闭 worker/source，并登记并发统计。
    /// 关闭内嵌 base。
    pub fn Close(&mut self) -> Result<(), errors::SharedError> {
        let mut firstError = None;
        self.finishCh.Finish();
        self.sourceRoutes.clear();

        if let Some(coordinator) = self.coordinator.take()
            && coordinator.join().is_err()
        {
            firstError = Some(errors::New("shuffle coordinator panicked"));
        }
        if let Some(receiver) = &self.outputReceiver {
            while receiver.try_recv().is_ok() {}
        }

        if self.inTest
            && self.prepared
            && !self.allSourceAndWorkerExitForTest.load(Ordering::Acquire)
        {
            panic!("there are still some running sources or workers");
        }

        for worker in &self.workers {
            let mut worker = worker.lock().expect("shuffle worker lock poisoned");
            for receiver in &worker.receivers {
                receiver
                    .lock()
                    .expect("shuffle receiver lock poisoned")
                    .ClearInput();
            }
            if let Err(error) = worker
                .childExec
                .lock()
                .expect("shuffle child executor lock poisoned")
                .Close()
                && firstError.is_none()
            {
                firstError = Some(error);
            }
            worker.DisconnectChannels();
        }

        self.executed = false;
        if self
            .base
            .lock()
            .expect("shuffle base executor lock poisoned")
            .HasRuntimeStats()
            && let Some(ctx) = &self.runtimeContext
        {
            ctx.RegisterConcurrencyStats("ShuffleConcurrency", self.concurrency);
        }

        for source in &self.dataSources {
            if let Err(error) = source
                .lock()
                .expect("shuffle data source lock poisoned")
                .Close()
                && firstError.is_none()
            {
                firstError = Some(error);
            }
        }
        if let Err(error) = self
            .base
            .lock()
            .expect("shuffle base executor lock poisoned")
            .Close()
            && firstError.is_none()
        {
            firstError = Some(error);
        }
        self.outputCh = None;
        self.outputReceiver = None;
        self.runtimeContext = None;

        match firstError {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// 启动 source 拉取线程、worker 线程与协调线程（首次 Next 时调用）。
    pub fn prepare4ParallelExec(&mut self, ctx: Arc<dyn ShuffleRuntimeContext>) {
        let routes = std::mem::take(&mut self.sourceRoutes);
        self.allSourceAndWorkerExitForTest
            .store(false, Ordering::Release);
        let output = self
            .outputCh
            .as_ref()
            .expect("shuffle must be opened before prepare")
            .clone();
        let mut handles = Vec::with_capacity(self.dataSources.len() + self.workers.len());

        for (sourceIndex, sourceRoutes) in routes.into_iter().enumerate() {
            let source = Arc::clone(&self.dataSources[sourceIndex]);
            let splitter = Arc::clone(&self.splitters[sourceIndex]);
            let finish = Arc::clone(&self.finishCh);
            let sourceOutput = output.clone();
            let sourceContext = Arc::clone(&ctx);
            handles.push(thread::spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    Self::fetchDataAndSplit(
                        sourceContext,
                        source,
                        splitter,
                        sourceRoutes,
                        Arc::clone(&finish),
                        sourceOutput.clone(),
                    );
                }));
                if let Err(payload) = result {
                    recoveryShuffleExec(&sourceOutput, &finish, payload);
                }
            }));
        }

        for worker in &self.workers {
            let worker = Arc::clone(worker);
            let workerContext = Arc::clone(&ctx);
            let finish = Arc::clone(&self.finishCh);
            let workerOutput = output.clone();
            handles.push(thread::spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    shuffleWorker::run(worker, workerContext);
                }));
                if let Err(payload) = result {
                    recoveryShuffleExec(&workerOutput, &finish, payload);
                }
            }));
        }

        let allExited = Arc::clone(&self.allSourceAndWorkerExitForTest);
        let coordinatorFinish = Arc::clone(&self.finishCh);
        self.coordinator = Some(thread::spawn(move || {
            Self::waitWorkerAndCloseOutput(handles, &output, &coordinatorFinish, allExited);
        }));
    }

    /// 等待全部线程结束，转发 panic 错误，最后发送 Finished。
    fn waitWorkerAndCloseOutput(
        handles: Vec<JoinHandle<()>>,
        output: &SyncSender<shuffleMessage>,
        finish: &finishSignal,
        allExited: Arc<AtomicBool>,
    ) {
        for handle in handles {
            if let Err(payload) = handle.join() {
                let error = panicError(payload.as_ref());
                let _ = sendUntilFinished(
                    output,
                    finish,
                    shuffleMessage::Output(shuffleOutput {
                        chk: None,
                        err: Some(error),
                        giveBackCh: None,
                    }),
                );
            }
        }
        allExited.store(true, Ordering::Release);
        let _ = sendUntilFinished(output, finish, shuffleMessage::Finished);
    }

    /// 从输出通道取下一批；必要时懒启动并行拓扑，并把 Chunk 列交换给调用方。
    pub fn Next(
        &mut self,
        ctx: Arc<dyn ShuffleRuntimeContext>,
        req: &mut chunk::Chunk,
    ) -> Result<(), errors::SharedError> {
        req.Reset();
        if !self.prepared {
            self.prepare4ParallelExec(Arc::clone(&ctx));
            self.prepared = true;
        }
        if let Some(error) = ctx.ShuffleNextError() {
            return Err(error);
        }
        if self.executed {
            return Ok(());
        }

        let receiver = self
            .outputReceiver
            .as_ref()
            .expect("shuffle must be opened before Next");
        match receiver.recv() {
            Ok(shuffleMessage::Finished) | Err(_) => {
                self.executed = true;
                Ok(())
            }
            Ok(shuffleMessage::Output(mut result)) => {
                if let Some(error) = result.err.take() {
                    return Err(error);
                }
                let mut output = result
                    .chk
                    .take()
                    .expect("shuffle worker output must contain a chunk");
                req.SwapColumns(&mut output);
                result
                    .giveBackCh
                    .take()
                    .expect("shuffle worker output must have a recycle channel")
                    .send(output)
                    .map_err(|_| errors::New("shuffle output recycle channel disconnected"))
            }
        }
    }

    /// Source 循环：Next 取行 → splitter 算 worker 下标 → 按分区追加并满则发送。
    fn fetchDataAndSplit(
        ctx: Arc<dyn ShuffleRuntimeContext>,
        source: Arc<Mutex<Box<dyn ShuffleExecutor>>>,
        splitter: Arc<Mutex<Box<dyn partitionSplitter>>>,
        routes: Vec<sourceRoute>,
        finish: Arc<finishSignal>,
        output: SyncSender<shuffleMessage>,
    ) {
        ctx.TriggerSourceFailpoint();
        let mut workerIndices = Vec::new();
        let mut results: Vec<Option<chunk::Chunk>> = (0..routes.len()).map(|_| None).collect();
        let mut input = source
            .lock()
            .expect("shuffle data source lock poisoned")
            .TryNewCacheChunk();

        loop {
            let nextResult = source
                .lock()
                .expect("shuffle data source lock poisoned")
                .Next(Arc::clone(&ctx), &mut input);
            if let Err(error) = nextResult {
                let _ = sendUntilFinished(
                    &output,
                    &finish,
                    shuffleMessage::Output(shuffleOutput {
                        chk: None,
                        err: Some(error),
                        giveBackCh: None,
                    }),
                );
                return;
            }
            if input.NumRows() == 0 {
                break;
            }

            workerIndices = match splitter
                .lock()
                .expect("shuffle splitter lock poisoned")
                .split(ctx.as_ref(), &input, workerIndices)
            {
                Ok(indices) => indices,
                Err(error) => {
                    let _ = sendUntilFinished(
                        &output,
                        &finish,
                        shuffleMessage::Output(shuffleOutput {
                            chk: None,
                            err: Some(error),
                            giveBackCh: None,
                        }),
                    );
                    return;
                }
            };
            if workerIndices.len() != input.NumRows() {
                let _ = sendUntilFinished(
                    &output,
                    &finish,
                    shuffleMessage::Output(shuffleOutput {
                        chk: None,
                        err: Some(errors::New("shuffle splitter returned the wrong row count")),
                        giveBackCh: None,
                    }),
                );
                return;
            }

            for rowIndex in 0..input.NumRows() {
                let workerIndex = workerIndices[rowIndex];
                if workerIndex >= routes.len() {
                    let _ = sendUntilFinished(
                        &output,
                        &finish,
                        shuffleMessage::Output(shuffleOutput {
                            chk: None,
                            err: Some(errors::New("shuffle splitter returned an invalid worker")),
                            giveBackCh: None,
                        }),
                    );
                    return;
                }
                if results[workerIndex].is_none() {
                    results[workerIndex] =
                        receiveUntilFinished(&routes[workerIndex].inputHolderCh, &finish);
                    if results[workerIndex].is_none() {
                        return;
                    }
                }
                let result = results[workerIndex]
                    .as_mut()
                    .expect("shuffle holder was acquired");
                result.AppendRow(input.GetRow(rowIndex));
                if result.IsFull() {
                    let full = results[workerIndex]
                        .take()
                        .expect("full shuffle chunk must exist");
                    if sendUntilFinished(&routes[workerIndex].inputCh, &finish, full).is_err() {
                        return;
                    }
                }
            }
        }

        for (workerIndex, result) in results.into_iter().enumerate() {
            if let Some(result) = result
                && sendUntilFinished(&routes[workerIndex].inputCh, &finish, result).is_err()
            {
                return;
            }
        }
    }
}

/// 将 panic payload 转为 SharedError。
fn panicError(payload: &(dyn Any + Send)) -> errors::SharedError {
    if let Some(message) = payload.downcast_ref::<&str>() {
        errors::New((*message).to_owned())
    } else if let Some(message) = payload.downcast_ref::<String>() {
        errors::New(message.clone())
    } else {
        errors::New("shuffle panicked")
    }
}

/// 捕获线程 panic：向输出通道投递错误并打印 backtrace。
fn recoveryShuffleExec(
    output: &SyncSender<shuffleMessage>,
    finish: &finishSignal,
    payload: Box<dyn Any + Send>,
) {
    let error = panicError(payload.as_ref());
    let logMessage = error.to_string();
    let _ = sendUntilFinished(
        output,
        finish,
        shuffleMessage::Output(shuffleOutput {
            chk: None,
            err: Some(error),
            giveBackCh: None,
        }),
    );
    eprintln!(
        "shuffle panicked: {logMessage}\n{}",
        Backtrace::force_capture()
    );
}

/// Worker 侧接收器：从对应 source 路由读入分区后的 Chunk。
pub struct shuffleReceiver {
    pub base: Arc<Mutex<Box<dyn ShuffleExecutor>>>,
    finishCh: Arc<finishSignal>,
    executed: bool,
    inputCh: Option<Arc<Mutex<Receiver<chunk::Chunk>>>>,
    inputHolderCh: Option<SyncSender<chunk::Chunk>>,
}

impl shuffleReceiver {
    /// 包装下游 base executor 构造接收器。
    pub fn new(base: Box<dyn ShuffleExecutor>) -> Self {
        Self {
            base: Arc::new(Mutex::new(base)),
            finishCh: Arc::new(finishSignal::default()),
            executed: false,
            inputCh: None,
            inputHolderCh: None,
        }
    }

    /// 安装 finish 与输入/回收通道。
    fn InstallChannels(
        &mut self,
        finish: Arc<finishSignal>,
        input: Receiver<chunk::Chunk>,
        holder: SyncSender<chunk::Chunk>,
    ) {
        self.finishCh = finish;
        self.inputCh = Some(Arc::new(Mutex::new(input)));
        self.inputHolderCh = Some(holder);
    }

    /// Open 时向 holder 放入首个可写空 Chunk。
    fn SeedInputHolder(&self, firstChunk: chunk::Chunk) -> Result<(), errors::SharedError> {
        self.inputHolderCh
            .as_ref()
            .expect("shuffle receiver holder channel not installed")
            .send(firstChunk)
            .map_err(|_| errors::New("shuffle input holder disconnected during open"))
    }

    /// Close 时排空并断开输入通道。
    fn ClearInput(&mut self) {
        if let Some(input) = &self.inputCh {
            while input
                .lock()
                .expect("shuffle input channel lock poisoned")
                .try_recv()
                .is_ok()
            {}
        }
        self.inputCh = None;
        self.inputHolderCh = None;
    }

    pub fn Open(&mut self, ctx: Arc<dyn ShuffleRuntimeContext>) -> Result<(), errors::SharedError> {
        self.base
            .lock()
            .expect("shuffle receiver base lock poisoned")
            .Open(ctx)?;
        self.executed = false;
        Ok(())
    }

    pub fn Close(&mut self) -> Result<(), errors::SharedError> {
        self.base
            .lock()
            .expect("shuffle receiver base lock poisoned")
            .Close()
    }

    /// 从输入通道取 Chunk，SwapColumns 后把空壳归还 holder。
    pub fn Next(&mut self, req: &mut chunk::Chunk) -> Result<(), errors::SharedError> {
        req.Reset();
        if self.executed {
            return Ok(());
        }
        let Some(mut result) = receiveUntilFinished(
            self.inputCh
                .as_ref()
                .expect("shuffle receiver input channel not installed"),
            &self.finishCh,
        ) else {
            self.executed = true;
            return Ok(());
        };
        if result.NumRows() == 0 {
            self.executed = true;
            return Ok(());
        }
        req.SwapColumns(&mut result);
        if sendUntilFinished(
            self.inputHolderCh
                .as_ref()
                .expect("shuffle receiver holder channel not installed"),
            &self.finishCh,
            result,
        )
        .is_err()
        {
            self.executed = true;
        }
        Ok(())
    }
}

/// 单个并发分区 worker：聚合多个 receiver，驱动 child 算子并写出。
pub struct shuffleWorker {
    pub childExec: Arc<Mutex<Box<dyn ShuffleExecutor>>>,
    pub receivers: Vec<Arc<Mutex<shuffleReceiver>>>,
    finishCh: Arc<finishSignal>,
    outputCh: Option<SyncSender<shuffleMessage>>,
    outputHolderCh: Option<Arc<Mutex<Receiver<chunk::Chunk>>>>,
    outputHolderSender: Option<SyncSender<chunk::Chunk>>,
}

impl shuffleWorker {
    pub fn new(
        childExec: Box<dyn ShuffleExecutor>,
        receivers: Vec<Arc<Mutex<shuffleReceiver>>>,
    ) -> Self {
        Self {
            childExec: Arc::new(Mutex::new(childExec)),
            receivers,
            finishCh: Arc::new(finishSignal::default()),
            outputCh: None,
            outputHolderCh: None,
            outputHolderSender: None,
        }
    }

    /// 断开输出相关通道引用。
    fn DisconnectChannels(&mut self) {
        self.outputCh = None;
        self.outputHolderCh = None;
        self.outputHolderSender = None;
    }

    /// Worker 主循环：取空输出 Chunk → child.Next → 通过输出通道交还结果。
    pub fn run(worker: Arc<Mutex<Self>>, ctx: Arc<dyn ShuffleRuntimeContext>) {
        shuffle_worker_failpoint();
        ctx.TriggerWorkerFailpoint();
        loop {
            let (finish, holder, output, giveBack) = {
                let worker = worker.lock().expect("shuffle worker lock poisoned");
                (
                    Arc::clone(&worker.finishCh),
                    Arc::clone(
                        worker
                            .outputHolderCh
                            .as_ref()
                            .expect("shuffle worker output holder not installed"),
                    ),
                    worker
                        .outputCh
                        .as_ref()
                        .expect("shuffle worker output channel not installed")
                        .clone(),
                    worker
                        .outputHolderSender
                        .as_ref()
                        .expect("shuffle worker output holder sender not installed")
                        .clone(),
                )
            };
            let Some(mut outputChunk) = receiveUntilFinished(&holder, &finish) else {
                return;
            };
            let child = {
                Arc::clone(
                    &worker
                        .lock()
                        .expect("shuffle worker lock poisoned")
                        .childExec,
                )
            };
            if let Err(error) = child
                .lock()
                .expect("shuffle child executor lock poisoned")
                .Next(Arc::clone(&ctx), &mut outputChunk)
            {
                let _ = sendUntilFinished(
                    &output,
                    &finish,
                    shuffleMessage::Output(shuffleOutput {
                        chk: None,
                        err: Some(error),
                        giveBackCh: None,
                    }),
                );
                return;
            }
            if outputChunk.NumRows() == 0 {
                return;
            }
            if sendUntilFinished(
                &output,
                &finish,
                shuffleMessage::Output(shuffleOutput {
                    chk: Some(outputChunk),
                    err: None,
                    giveBackCh: Some(giveBack),
                }),
            )
            .is_err()
            {
                return;
            }
        }
    }
}

/// 测试用 failpoint 钩子（对应 Go shuffleWorkerRun）。
pub(crate) fn shuffle_worker_failpoint() {
    let _ = fail::eval(
        "github.com/pingcap/tidb/pkg/executor/shuffleWorkerRun",
        |_| (),
    );
}

/// 将输入 Chunk 各行映射到 worker 下标。
pub trait partitionSplitter: Send {
    fn split(
        &mut self,
        ctx: &dyn ShuffleRuntimeContext,
        input: &chunk::Chunk,
        workerIndices: Vec<usize>,
    ) -> Result<Vec<usize>, errors::SharedError>;
}

/// 按分组键哈希（Murmur3）取模分发到 worker。
pub struct partitionHashSplitter {
    byItems: Vec<ShuffleExpression>,
    numWorkers: usize,
    hashKeys: Vec<Vec<u8>>,
}

impl partitionSplitter for partitionHashSplitter {
    fn split(
        &mut self,
        ctx: &dyn ShuffleRuntimeContext,
        input: &chunk::Chunk,
        mut workerIndices: Vec<usize>,
    ) -> Result<Vec<usize>, errors::SharedError> {
        self.hashKeys =
            ctx.GetGroupKey(input, std::mem::take(&mut self.hashKeys), &self.byItems)?;
        if self.hashKeys.len() != input.NumRows() {
            return Err(errors::New(
                "group key count does not match shuffle input rows",
            ));
        }
        // 对每行 group key 做 murmur3 哈希后对 worker 数取模。
        workerIndices.clear();
        workerIndices.extend(
            self.hashKeys
                .iter()
                .map(|key| murmur3Sum32(key) as usize % self.numWorkers),
        );
        Ok(workerIndices)
    }
}

/// 构造哈希 splitter；concurrency 必须为正。
pub fn buildPartitionHashSplitter(
    concurrency: usize,
    byItems: Vec<ShuffleExpression>,
) -> partitionHashSplitter {
    assert!(concurrency > 0, "shuffle concurrency must be positive");
    partitionHashSplitter {
        byItems,
        numWorkers: concurrency,
        hashKeys: Vec::new(),
    }
}

/// 按有序分组轮询（round-robin）分配连续组到各 worker。
pub struct partitionRangeSplitter {
    byItems: Vec<ShuffleExpression>,
    numWorkers: usize,
    groupChecker: Box<dyn ShuffleGroupChecker>,
    idx: usize,
}

/// 基于 RuntimeContext 的 GroupChecker 构造 range splitter。
pub fn buildPartitionRangeSplitter(
    ctx: &dyn ShuffleRuntimeContext,
    concurrency: usize,
    byItems: Vec<ShuffleExpression>,
) -> partitionRangeSplitter {
    assert!(concurrency > 0, "shuffle concurrency must be positive");
    partitionRangeSplitter {
        groupChecker: ctx.NewGroupChecker(&byItems),
        byItems,
        numWorkers: concurrency,
        idx: 0,
    }
}

impl partitionSplitter for partitionRangeSplitter {
    fn split(
        &mut self,
        _ctx: &dyn ShuffleRuntimeContext,
        input: &chunk::Chunk,
        mut workerIndices: Vec<usize>,
    ) -> Result<Vec<usize>, errors::SharedError> {
        self.groupChecker.SplitIntoGroups(input)?;
        workerIndices.clear();
        while !self.groupChecker.IsExhausted() {
            let (begin, end) = self.groupChecker.GetNextGroup();
            workerIndices.extend((begin..end).map(|_| self.idx));
            self.idx = (self.idx + 1) % self.numWorkers;
        }
        Ok(workerIndices)
    }
}

/// twmb/murmur3.Sum32: MurmurHash3 x86 32-bit, seed 0.
/// MurmurHash3 x86 32-bit（seed=0），与 Go twmb/murmur3.Sum32 对齐。
fn murmur3Sum32(input: &[u8]) -> u32 {
    let mut hash = 0_u32;
    let mut chunks = input.chunks_exact(4);
    for chunk in &mut chunks {
        let mut key = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        key = key.wrapping_mul(0xcc9e_2d51);
        key = key.rotate_left(15);
        key = key.wrapping_mul(0x1b87_3593);
        hash ^= key;
        hash = hash.rotate_left(13);
        hash = hash.wrapping_mul(5).wrapping_add(0xe654_6b64);
    }

    let tail = chunks.remainder();
    let mut key = 0_u32;
    match tail.len() {
        3 => {
            key ^= u32::from(tail[2]) << 16;
            key ^= u32::from(tail[1]) << 8;
            key ^= u32::from(tail[0]);
        }
        2 => {
            key ^= u32::from(tail[1]) << 8;
            key ^= u32::from(tail[0]);
        }
        1 => key ^= u32::from(tail[0]),
        _ => {}
    }
    if !tail.is_empty() {
        key = key.wrapping_mul(0xcc9e_2d51);
        key = key.rotate_left(15);
        key = key.wrapping_mul(0x1b87_3593);
        hash ^= key;
    }

    hash ^= input.len() as u32;
    hash ^= hash >> 16;
    hash = hash.wrapping_mul(0x85eb_ca6b);
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(0xc2b2_ae35);
    hash ^ (hash >> 16)
}
