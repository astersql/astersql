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

// 通用 Worker 池：可取消上下文、可关闭通道、任务恢复与动态调容。
//
// 对应 Go `workerpool`：Worker 消费 Task、可选产出 Result；Context 保留首个错误
// 并广播取消；Tune 增减 worker 数量，可选等待 Close 完成。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

pub use crossbeam_channel::RecvTimeoutError;
use crossbeam_channel::{Receiver, Sender};
use std::any::TypeId;
use std::error::Error as StdError;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

/// Error is the clonable business error passed between workers and the pool.
///
/// Go errors are interface values and can be stored behind an atomic pointer.
/// The Rust pool needs to copy the first error back to callers, so the message
/// is kept in an `Arc` while retaining normal `Error` behavior.
/// 可克隆业务错误；消息存于 Arc，便于在 worker 与调用方之间传递首个错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    message: Arc<str>,
}

impl Error {
    /// 由消息构造 Error。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: Arc::from(message.into()),
        }
    }

    /// 返回错误消息文本。
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl StdError for Error {}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

/// Context 的共享状态：取消标志、首错、Done 通道与子上下文弱引用列表。
struct ContextInner {
    cancelled: AtomicBool,
    first_error: Mutex<Option<Error>>,
    done_sender: Mutex<Option<Sender<()>>>,
    done_receiver: Receiver<()>,
    children: Mutex<Vec<Weak<ContextInner>>>,
}

impl ContextInner {
    fn new() -> Arc<Self> {
        let (done_sender, done_receiver) = crossbeam_channel::bounded(0);
        Arc::new(Self {
            cancelled: AtomicBool::new(false),
            first_error: Mutex::new(std::option::Option::None),
            done_sender: Mutex::new(Some(done_sender)),
            done_receiver,
            children: Mutex::new(Vec::new()),
        })
    }

    /// 幂等取消：关闭 Done sender，并递归取消所有仍存活的子上下文。
    fn cancel(self: &Arc<Self>) {
        if self.cancelled.swap(true, Ordering::SeqCst) {
            return;
        }

        // Dropping the only sender disconnects every cloned receiver and wakes
        // all selects waiting on Done, just like closing a Go context channel.
        self.done_sender.lock().unwrap().take();

        let children = {
            let mut registered = self.children.lock().unwrap();
            let live = registered
                .iter()
                .filter_map(Weak::upgrade)
                .collect::<Vec<_>>();
            registered.retain(|child| child.strong_count() > 0);
            live
        };
        for child in children {
            child.cancel();
        }
    }
}

/// Context stores the first operator error and broadcasts cancellation.
/// Child contexts observe parent cancellation, while cancelling a child does
/// not cancel its parent, matching `context.WithCancel`.
/// 保存操作首错并广播取消；子上下文跟随父取消，子取消不影响父。
#[derive(Clone)]
pub struct Context {
    inner: Arc<ContextInner>,
}

impl Default for Context {
    fn default() -> Self {
        Self::background()
    }
}

impl Context {
    /// 创建根（background）上下文。
    pub fn background() -> Self {
        Self {
            inner: ContextInner::new(),
        }
    }

    /// 创建子上下文；若父已取消则子立即取消，否则注册到父的 children。
    fn child(&self) -> Self {
        let inner = ContextInner::new();
        if self.IsCancelled() {
            inner.cancel();
        } else {
            self.inner
                .children
                .lock()
                .unwrap()
                .push(Arc::downgrade(&inner));
            // Close the race between the cancellation check and registration.
            // 关闭「检查取消」与「注册子节点」之间的竞态窗口。
            if self.IsCancelled() {
                inner.cancel();
            }
        }
        Self { inner }
    }

    /// OnError keeps the first error but always broadcasts cancellation.
    /// 保留首个错误，但始终触发取消广播。
    pub fn OnError(&self, error: Error) {
        log::error!("worker pool encountered error: {error}");
        let mut first_error = self.inner.first_error.lock().unwrap();
        if first_error.is_none() {
            *first_error = Some(error);
        }
        drop(first_error);
        self.Cancel();
    }

    /// 返回已记录的首个操作错误（若有）。
    pub fn OperatorErr(&self) -> Option<Error> {
        self.inner.first_error.lock().unwrap().clone()
    }

    /// 取消本上下文及其子树。
    pub fn Cancel(&self) {
        self.inner.cancel();
    }

    /// 是否已取消。
    pub fn IsCancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// 返回 Done 接收端，供 select 等待取消。
    fn done(&self) -> Receiver<()> {
        self.inner.done_receiver.clone()
    }
}

/// NewContext creates a cancellable child of the supplied parent context.
/// 基于父上下文创建可取消的子上下文。
pub fn NewContext(parent: Context) -> Context {
    parent.child()
}

/// Channel 内部：数据收发端、关闭标志与独立 close 信号。
struct ChannelInner<T> {
    sender: Sender<T>,
    receiver: Receiver<T>,
    closed: AtomicBool,
    close_sender: Mutex<Option<Sender<()>>>,
    close_receiver: Receiver<()>,
}

/// Channel is a clonable, explicitly closable MPMC channel.
///
/// Crossbeam supplies the select semantics needed by the Go implementation.
/// The separate close signal is necessary because the pool and callers retain
/// cloned handles while Go channels have shared explicit close state.
/// 可克隆、显式可关闭的多生产者多消费者通道；独立 close 信号模拟 Go 关闭语义。
pub struct Channel<T> {
    inner: Arc<ChannelInner<T>>,
}

impl<T> Clone for Channel<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> Channel<T> {
    /// 创建有界通道。
    pub fn bounded(capacity: usize) -> Self {
        let (sender, receiver) = crossbeam_channel::bounded(capacity);
        let (close_sender, close_receiver) = crossbeam_channel::bounded(0);
        Self {
            inner: Arc::new(ChannelInner {
                sender,
                receiver,
                closed: AtomicBool::new(false),
                close_sender: Mutex::new(Some(close_sender)),
                close_receiver,
            }),
        }
    }

    /// 创建无界通道。
    pub fn unbounded() -> Self {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let (close_sender, close_receiver) = crossbeam_channel::bounded(0);
        Self {
            inner: Arc::new(ChannelInner {
                sender,
                receiver,
                closed: AtomicBool::new(false),
                close_sender: Mutex::new(Some(close_sender)),
                close_receiver,
            }),
        }
    }

    /// 发送；通道已关闭或关闭信号先到则返回 false。
    pub fn send(&self, value: T) -> bool {
        if self.is_closed() {
            return false;
        }
        let sender = self.inner.sender.clone();
        let closed = self.inner.close_receiver.clone();
        crossbeam_channel::select! {
            send(sender, value) -> result => result.is_ok(),
            recv(closed) -> _ => false,
        }
    }

    /// 发送时可被 Context 取消打断。
    fn send_or_cancel(&self, value: T, context: &Context) -> bool {
        if self.is_closed() || context.IsCancelled() {
            return false;
        }
        let sender = self.inner.sender.clone();
        let closed = self.inner.close_receiver.clone();
        let done = context.done();
        crossbeam_channel::select! {
            send(sender, value) -> result => result.is_ok(),
            recv(closed) -> _ => false,
            recv(done) -> _ => false,
        }
    }

    /// recv drains already buffered values before reporting a closed channel.
    /// 先排空缓冲；关闭后若无缓冲则返回 None。
    pub fn recv(&self) -> Option<T> {
        if let Ok(value) = self.inner.receiver.try_recv() {
            return Some(value);
        }
        if self.is_closed() {
            return std::option::Option::None;
        }
        let receiver = self.inner.receiver.clone();
        let closed = self.inner.close_receiver.clone();
        crossbeam_channel::select_biased! {
            recv(receiver) -> result => result.ok(),
            recv(closed) -> _ => self.inner.receiver.try_recv().ok(),
        }
    }

    /// Receive buffered values before observing closure, as recv does. A timeout
    /// leaves the channel open and lets callers re-check their cancellation state.
    /// Exhausted closed channels return Ok(None), distinct from Err(Timeout).
    pub fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<Option<T>, RecvTimeoutError> {
        if let Ok(value) = self.inner.receiver.try_recv() {
            return Ok(Some(value));
        }
        if self.is_closed() {
            return Ok(std::option::Option::None);
        }
        let receiver = self.inner.receiver.clone();
        let closed = self.inner.close_receiver.clone();
        let elapsed = crossbeam_channel::after(timeout);
        crossbeam_channel::select_biased! {
            recv(receiver) -> result => Ok(result.ok()),
            recv(closed) -> _ => Ok(self.inner.receiver.try_recv().ok()),
            recv(elapsed) -> _ => Err(RecvTimeoutError::Timeout),
        }
    }

    /// 幂等关闭通道。
    pub fn close(&self) {
        if !self.inner.closed.swap(true, Ordering::SeqCst) {
            self.inner.close_sender.lock().unwrap().take();
        }
    }

    /// 是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst)
    }

    fn data_receiver(&self) -> Receiver<T> {
        self.inner.receiver.clone()
    }

    fn close_receiver(&self) -> Receiver<()> {
        self.inner.close_receiver.clone()
    }
}

/// TaskMayPanic supplies the same recovery metadata as the Go task interface.
/// 任务 panic 恢复所需的标签、函数信息与可选预置错误。
pub trait TaskMayPanic {
    fn RecoverArgs(&self) -> (String, String, Option<Error>);
}

/// Worker consumes tasks, optionally sends results, and releases its resources.
/// Worker 消费任务、可选发送结果，并在退出时 Close 释放资源。
pub trait Worker<T: TaskMayPanic, R>: Send {
    fn HandleTask(&mut self, task: T, send: &mut dyn FnMut(R)) -> Result<(), Error>;
    fn Close(&mut self) -> Result<(), Error>;
}

impl<T, R, W> Worker<T, R> for Box<W>
where
    T: TaskMayPanic,
    W: Worker<T, R> + ?Sized,
{
    fn HandleTask(&mut self, task: T, send: &mut dyn FnMut(R)) -> Result<(), Error> {
        (**self).HandleTask(task, send)
    }

    fn Close(&mut self) -> Result<(), Error> {
        (**self).Close()
    }
}

/// Tuner exposes dynamic capacity without exposing the generic worker types.
/// 对外暴露动态调容，隐藏具体泛型 Worker 类型。
pub trait Tuner {
    fn Tune(&mut self, numWorkers: i32, wait: bool);
}

/// PoolOption is the Rust spelling of the Go `Option` interface.  The expanded
/// name avoids shadowing `std::option::Option` for wildcard API consumers.
/// 对应 Go 的 Option 接口；扩展命名避免与 std::option::Option 冲突。
pub trait PoolOption<T, R>
where
    T: TaskMayPanic + Send + 'static,
    R: Send + 'static,
{
    fn Apply(&self, pool: &mut WorkerPool<T, R>);
}

/// None marks a pool whose workers never produce results.
/// 标记无结果类型的池；此时不创建结果通道。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct None;

/// 类似 Go WaitGroup 的倒计时器，用于 Tune(wait=true) 等待 worker Close。
#[derive(Default)]
struct Countdown {
    pending: Mutex<usize>,
    changed: Condvar,
}

impl Countdown {
    fn Add(&self) {
        *self.pending.lock().unwrap() += 1;
    }

    fn Done(&self) {
        let mut pending = self.pending.lock().unwrap();
        debug_assert!(*pending > 0);
        *pending -= 1;
        if *pending == 0 {
            self.changed.notify_all();
        }
    }

    fn Wait(&self) {
        let mut pending = self.pending.lock().unwrap();
        while *pending != 0 {
            pending = self.changed.wait(pending).unwrap();
        }
    }
}

/// RAII：进入任务处理时 running +1，离开时 -1。
struct RunningGuard {
    running: Arc<AtomicI32>,
}

impl RunningGuard {
    fn new(running: Arc<AtomicI32>) -> Self {
        running.fetch_add(1, Ordering::SeqCst);
        Self { running }
    }
}

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.running.fetch_sub(1, Ordering::SeqCst);
    }
}

type WorkerFactory<T, R> = dyn Fn() -> Option<Box<dyn Worker<T, R>>> + Send + Sync + 'static;

/// WorkerPool is the Rust implementation of TiDB's generic Go worker pool.
/// TiDB 通用 Go Worker 池的 Rust 实现。
pub struct WorkerPool<T, R>
where
    T: TaskMayPanic + Send + 'static,
    R: Send + 'static,
{
    /// 调用方（operator）上下文，错误写入此处。
    wctx: Option<Context>,
    /// worker 子上下文，取消时打断任务循环。
    ctx: Option<Context>,
    name: String,
    numWorkers: i32,
    originWorkers: i32,
    runningTask: Arc<AtomicI32>,
    taskChan: Option<Channel<T>>,
    resChan: Option<Channel<R>>,
    quitSender: Sender<Arc<Countdown>>,
    quitReceiver: Receiver<Arc<Countdown>>,
    workers: Vec<JoinHandle<()>>,
    createWorker: Arc<WorkerFactory<T, R>>,
    lastTuneTs: SystemTime,
    started: bool,
}

impl<T, R> WorkerPool<T, R>
where
    T: TaskMayPanic + Send + 'static,
    R: Send + 'static,
{
    /// NewWorkerPool creates a pool and clamps non-positive concurrency to one.
    /// The component is generic because it is metadata ignored by the Go pool.
    /// 创建池并将非正并发度钳制为 1；component 为元数据，与 Go 一样被忽略。
    pub fn NewWorkerPool<C, F, W>(
        name: impl Into<String>,
        _component: C,
        numWorkers: i32,
        createWorker: F,
    ) -> Self
    where
        F: Fn() -> W + Send + Sync + 'static,
        W: Worker<T, R> + 'static,
    {
        let factory = Arc::new(move || Some(Box::new(createWorker()) as Box<dyn Worker<T, R>>));
        Self::newWithFactory(name.into(), numWorkers, factory)
    }

    /// NewWorkerPoolWithOptions applies Go-style options in argument order.
    /// 按参数顺序应用 Go 风格选项。
    pub fn NewWorkerPoolWithOptions<C, F, W>(
        name: impl Into<String>,
        component: C,
        numWorkers: i32,
        createWorker: F,
        options: Vec<Box<dyn PoolOption<T, R>>>,
    ) -> Self
    where
        F: Fn() -> W + Send + Sync + 'static,
        W: Worker<T, R> + 'static,
    {
        let mut pool = Self::NewWorkerPool(name, component, numWorkers, createWorker);
        for option in options {
            option.Apply(&mut pool);
        }
        pool
    }

    /// A fallible factory preserves the Go branch where createWorker returns nil.
    /// 可失败工厂：对应 Go 中 createWorker 返回 nil 的分支。
    pub fn NewWorkerPoolWithFallibleFactory<C, F, W>(
        name: impl Into<String>,
        _component: C,
        numWorkers: i32,
        createWorker: F,
    ) -> Self
    where
        F: Fn() -> Option<W> + Send + Sync + 'static,
        W: Worker<T, R> + 'static,
    {
        let factory = Arc::new(move || {
            createWorker().map(|worker| Box::new(worker) as Box<dyn Worker<T, R>>)
        });
        Self::newWithFactory(name.into(), numWorkers, factory)
    }

    fn newWithFactory(
        name: String,
        mut numWorkers: i32,
        createWorker: Arc<WorkerFactory<T, R>>,
    ) -> Self {
        if numWorkers <= 0 {
            numWorkers = 1;
        }
        // Go's InjectCall is observational here because numWorkers is passed by
        // value.  `fail::eval` preserves configured pause/panic/log behavior.
        let _ = fail::eval("NewWorkerPool", |_| numWorkers);
        let (quitSender, quitReceiver) = crossbeam_channel::unbounded();
        Self {
            wctx: std::option::Option::None,
            ctx: std::option::Option::None,
            name,
            numWorkers,
            originWorkers: numWorkers,
            runningTask: Arc::new(AtomicI32::new(0)),
            taskChan: std::option::Option::None,
            resChan: std::option::Option::None,
            quitSender,
            quitReceiver,
            workers: Vec::new(),
            createWorker,
            lastTuneTs: UNIX_EPOCH,
            started: false,
        }
    }

    /// 自定义任务接收通道（须在 Start 前设置）。
    pub fn SetTaskReceiver(&mut self, receiver: Channel<T>) {
        self.taskChan = Some(receiver);
    }

    /// 自定义结果发送通道（须在 Start 前设置）。
    pub fn SetResultSender(&mut self, sender: Channel<R>) {
        self.resChan = Some(sender);
    }

    /// Start creates default unbuffered channels and starts the configured workers.
    /// 创建默认无缓冲通道并启动配置数量的 worker；只能调用一次。
    pub fn Start(&mut self, context: Context) {
        assert!(!self.started, "worker pool can only be started once");
        if self.taskChan.is_none() {
            self.taskChan = Some(Channel::bounded(0));
        }
        if self.resChan.is_none() && TypeId::of::<R>() != TypeId::of::<None>() {
            self.resChan = Some(Channel::bounded(0));
        }

        let child = NewContext(context.clone());
        self.wctx = Some(context);
        self.ctx = Some(child);
        for _ in 0..self.numWorkers {
            self.runAWorker();
        }
        self.started = true;
    }

    /// 工厂创建 worker 并 spawn 线程，循环 select 任务/关闭/退出/取消。
    fn runAWorker(&mut self) {
        let Some(mut worker) = (self.createWorker)() else {
            return;
        };
        let task_channel = self.taskChan.as_ref().expect("task channel").clone();
        let result_channel = self.resChan.clone();
        let quit_receiver = self.quitReceiver.clone();
        let context = self.ctx.as_ref().expect("worker context").clone();
        let worker_context = self.wctx.as_ref().expect("operator context").clone();
        let running = Arc::clone(&self.runningTask);
        let thread_name = format!("{}-worker-{}", self.name, self.workers.len());

        let handle = thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                let tasks = task_channel.data_receiver();
                let tasks_closed = task_channel.close_receiver();
                let done = context.done();
                let mut tune_config: Option<Arc<Countdown>> = std::option::Option::None;
                loop {
                    // A closed Go channel yields every buffered task before it
                    // reports ok=false. Prefer ready task data over our separate
                    // close signal to preserve that drain-before-exit contract.
                    crossbeam_channel::select_biased! {
                        recv(tasks) -> task => match task {
                            Ok(task) => Self::handleTaskWithRecover(
                                worker.as_mut(),
                                task,
                                result_channel.as_ref(),
                                &context,
                                &worker_context,
                                Arc::clone(&running),
                            ),
                            Err(_) => break,
                        },
                        recv(tasks_closed) -> _ => break,
                        recv(quit_receiver) -> config => {
                            if let Ok(config) = config {
                                tune_config = Some(config);
                            }
                            break;
                        },
                        recv(done) -> _ => break,
                    }
                }

                if let Err(error) = worker.Close() {
                    worker_context.OnError(error);
                }
                // 缩容等待：Close 完成后通知 Countdown。
                if let Some(config) = tune_config {
                    config.Done();
                }
            })
            .expect("failed to start worker thread");
        self.workers.push(handle);
    }

    /// 捕获 HandleTask 的返回错误与 panic，统一写入 operator 上下文。
    fn handleTaskWithRecover(
        worker: &mut dyn Worker<T, R>,
        task: T,
        result_channel: Option<&Channel<R>>,
        context: &Context,
        worker_context: &Context,
        running: Arc<AtomicI32>,
    ) {
        let _running = RunningGuard::new(running);
        let (label, func_info, recover_error) = task.RecoverArgs();
        let mut send_result = |result| {
            if let Some(channel) = result_channel {
                channel.send_or_cancel(result, context);
            }
        };

        match catch_unwind(AssertUnwindSafe(|| {
            worker.HandleTask(task, &mut send_result)
        })) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => worker_context.OnError(error),
            Err(_) => worker_context.OnError(recover_error.unwrap_or_else(|| {
                Error::new(format!("task panic: {label}, func info: {func_info}"))
            })),
        }
    }

    /// AddTask is the test-facing task sender from the Go implementation.
    /// False means cancellation or channel close won the send select.
    /// 测试向任务通道投递；取消或关闭抢先则返回 false。
    pub fn AddTask(&self, task: T) -> bool {
        let Some(channel) = &self.taskChan else {
            return false;
        };
        let Some(context) = &self.ctx else {
            return false;
        };
        channel.send_or_cancel(task, context)
    }

    /// 返回结果通道克隆（无结果类型时为 None）。
    pub fn GetResultChan(&self) -> Option<Channel<R>> {
        self.resChan.clone()
    }

    /// Tune grows by creating workers and shrinks by sending one quit request per
    /// removed worker.  The optional wait covers Worker::Close completion.
    /// 扩容直接创建 worker；缩容向每个被移除 worker 发送 quit，wait 时等待 Close。
    pub fn Tune(&mut self, mut numWorkers: i32, wait: bool) {
        if numWorkers <= 0 {
            numWorkers = 1;
        }
        self.lastTuneTs = SystemTime::now();
        log::info!(
            "tune worker pool from {} to {}",
            self.numWorkers,
            numWorkers
        );

        if !self.started {
            self.numWorkers = numWorkers;
            return;
        }

        let difference = numWorkers - self.numWorkers;
        if difference > 0 {
            for _ in 0..difference {
                self.runAWorker();
            }
        } else if difference < 0 {
            let countdown = Arc::new(Countdown::default());
            let context = self.ctx.as_ref().expect("worker context").clone();
            for _ in 0..-difference {
                countdown.Add();
                if context.IsCancelled() {
                    log::info!(
                        "context done when tuning worker pool from {} to {}",
                        self.numWorkers,
                        numWorkers
                    );
                    countdown.Done();
                    break;
                }
                let quit = self.quitSender.clone();
                let done = context.done();
                let config = Arc::clone(&countdown);
                crossbeam_channel::select! {
                    send(quit, config) -> result => {
                        if result.is_err() {
                            countdown.Done();
                            break;
                        }
                    },
                    recv(done) -> _ => {
                        countdown.Done();
                        break;
                    },
                }
            }
            if wait {
                countdown.Wait();
            }
        }
        self.numWorkers = numWorkers;
    }

    /// 最近一次 Tune 时间。
    pub fn LastTunerTs(&self) -> SystemTime {
        self.lastTuneTs
    }

    /// 当前配置的 worker 容量。
    pub fn Cap(&self) -> i32 {
        self.numWorkers
    }

    /// 正在处理任务的 worker 数。
    pub fn Running(&self) -> i32 {
        self.runningTask.load(Ordering::SeqCst)
    }

    /// 池名称。
    pub fn Name(&self) -> &str {
        &self.name
    }

    /// CloseAndWait cancels active work and waits for every worker to close.
    /// 取消活动工作并等待所有 worker 退出。
    pub fn CloseAndWait(&mut self) {
        if let Some(context) = &self.ctx {
            context.Cancel();
        }
        self.Release();
    }

    /// Release waits for input closure or cancellation, then closes results.
    /// join 全部 worker，取消上下文并关闭结果通道。
    pub fn Release(&mut self) {
        let worker_context = self.wctx.clone();
        for worker in std::mem::take(&mut self.workers) {
            if worker.join().is_err()
                && let Some(context) = &worker_context
            {
                context.OnError(Error::new("worker thread panicked outside task recovery"));
            }
        }
        if let Some(context) = &self.ctx {
            context.Cancel();
        }
        if let Some(results) = self.resChan.take() {
            results.close();
        }
    }

    /// 创建时的原始 worker 数量。
    pub fn GetOriginConcurrency(&self) -> i32 {
        self.originWorkers
    }
}

impl<T, R> Tuner for WorkerPool<T, R>
where
    T: TaskMayPanic + Send + 'static,
    R: Send + 'static,
{
    fn Tune(&mut self, numWorkers: i32, wait: bool) {
        WorkerPool::Tune(self, numWorkers, wait);
    }
}

impl<T, R> Drop for WorkerPool<T, R>
where
    T: TaskMayPanic + Send + 'static,
    R: Send + 'static,
{
    fn drop(&mut self) {
        if let Some(context) = &self.ctx {
            context.Cancel();
        }
        for worker in std::mem::take(&mut self.workers) {
            let _ = worker.join();
        }
        if let Some(results) = self.resChan.take() {
            results.close();
        }
    }
}
