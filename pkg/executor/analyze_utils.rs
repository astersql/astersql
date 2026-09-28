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

// ANALYZE 执行共用工具：错误类型、自适应并发、通知通道与 WaitGroup 包装。
//
// DistSQL 指下推到 TiKV（分布式 KV 存储）的扫描请求；本模块还提供
// 会话变量读取与 panic/OOM 错误规范化等辅助能力。

#![allow(non_camel_case_types, non_snake_case)]

use std::any::Any;
use std::collections::VecDeque;
use std::fmt::{Display, Formatter};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

/// DistSQL（下推到 TiKV 的分布式扫描）ANALYZE 默认并发度。
pub const DEF_ANALYZE_DIST_SQL_SCAN_CONCURRENCY: usize = 15;
/// 推导采样率时参考的默认行数规模。
pub const DEF_ROWS_FOR_SAMPLE_RATE: i64 = 110_000;
/// 全局 ANALYZE 内存超限时 panic 载荷的固定文案。
pub const GLOBAL_PANIC_ANALYZE_MEMORY_EXCEED: &str = "Out Of Global Analyze Memory Limit!";
/// 构建统计信息并发度的会话/全局变量名。
pub const TIDB_BUILD_STATS_CONCURRENCY: &str = "tidb_build_stats_concurrency";
/// 采样构建统计信息并发度的会话/全局变量名。
pub const TIDB_BUILD_SAMPLING_STATS_CONCURRENCY: &str = "tidb_build_sampling_stats_concurrency";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// ANALYZE 错误分类：worker panic、OOM、取消、超时等。
pub enum AnalyzeErrorKind {
    AnalyzeWorkerPanic,
    AnalyzeOutOfMemory,
    Canceled,
    DeadlineExceeded,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 带种类与消息的 ANALYZE 错误。
pub struct AnalyzeError {
    pub kind: AnalyzeErrorKind,
    pub message: String,
}

impl AnalyzeError {
    /// 构造指定种类的错误。
    pub fn new(kind: AnalyzeErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// 构造 `Other` 种类错误。
    pub fn other(message: impl Into<String>) -> Self {
        Self::new(AnalyzeErrorKind::Other, message)
    }

    /// 构造取消（Canceled）错误。
    pub fn canceled(message: impl Into<String>) -> Self {
        Self::new(AnalyzeErrorKind::Canceled, message)
    }

    /// 构造超时（DeadlineExceeded）错误。
    pub fn deadline_exceeded(message: impl Into<String>) -> Self {
        Self::new(AnalyzeErrorKind::DeadlineExceeded, message)
    }
}

impl Display for AnalyzeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AnalyzeError {}

/// 返回标准的 analyze worker panic 错误。
pub fn errAnalyzeWorkerPanic() -> AnalyzeError {
    AnalyzeError::new(AnalyzeErrorKind::AnalyzeWorkerPanic, "analyze worker panic")
}

/// 返回因内存配额超限导致的 ANALYZE OOM 错误。
pub fn errAnalyzeOOM() -> AnalyzeError {
    AnalyzeError::new(
        AnalyzeErrorKind::AnalyzeOutOfMemory,
        format!(
            "analyze panic due to memory quota exceeds, please try with smaller samplerate(refer to {DEF_ROWS_FOR_SAMPLE_RATE}/count)"
        ),
    )
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 工具函数使用的轻量上下文，可携带因果错误（cause）。
pub struct AnalyzeContext {
    pub cause: Option<AnalyzeError>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 查询 TiKV Store（存储节点）状态失败的原因。
pub enum StoreStatusError {
    PdHttpClient(AnalyzeError),
    GetStores(AnalyzeError),
}

/// Boundary used by analyze helpers to access session and TiKV state.
/// Implementations must explicitly provide every operation; failures are never
/// treated as successful lookups.
pub trait AnalyzeSessionContext {
    fn analyze_dist_sql_scan_concurrency(&self) -> i64;
    fn get_session_or_global_system_var(&self, name: &str) -> Result<String, AnalyzeError>;
    fn tikv_store_count(&self, ctx: &AnalyzeContext) -> Result<Option<usize>, StoreStatusError>;
    fn warn(&self, message: &str, error: Option<&AnalyzeError>);
}

/// 按 TiKV store 数量自适应选择 DistSQL 扫描并发；配置 >0 时优先生效。
pub fn adaptiveAnlayzeDistSQLConcurrency<C: AnalyzeSessionContext>(
    ctx: &AnalyzeContext,
    sctx: &C,
) -> usize {
    let configured = sctx.analyze_dist_sql_scan_concurrency();
    if configured > 0 {
        return usize::try_from(configured).unwrap_or(DEF_ANALYZE_DIST_SQL_SCAN_CONCURRENCY);
    }

    let count = match sctx.tikv_store_count(ctx) {
        Ok(Some(count)) => count,
        Ok(None) => {
            sctx.warn(
                "Information about TiKV store status can be gotten only when the storage is TiKV",
                None,
            );
            return DEF_ANALYZE_DIST_SQL_SCAN_CONCURRENCY;
        }
        Err(StoreStatusError::PdHttpClient(error)) => {
            sctx.warn("fail to TryGetPDHTTPClient", Some(&error));
            return DEF_ANALYZE_DIST_SQL_SCAN_CONCURRENCY;
        }
        Err(StoreStatusError::GetStores(error)) => {
            sctx.warn("fail to get stores info", Some(&error));
            return DEF_ANALYZE_DIST_SQL_SCAN_CONCURRENCY;
        }
    };

    match count {
        0..=5 => DEF_ANALYZE_DIST_SQL_SCAN_CONCURRENCY,
        6..=10 => count,
        11..=20 => count.saturating_mul(2),
        21..=50 => count.saturating_mul(3),
        _ => count.saturating_mul(4),
    }
}

/// 读取会话/全局变量并解析为整数。
pub fn getIntFromSessionVars<C: AnalyzeSessionContext>(
    ctx: &C,
    name: &str,
) -> Result<i64, AnalyzeError> {
    let value = ctx.get_session_or_global_system_var(name)?;
    value.parse::<i64>().map_err(|error| {
        AnalyzeError::other(format!(
            "cannot parse session variable {name} value {value:?} as an integer: {error}"
        ))
    })
}

/// 读取构建统计并发度配置。
pub fn getBuildStatsConcurrency<C: AnalyzeSessionContext>(ctx: &C) -> Result<i64, AnalyzeError> {
    getIntFromSessionVars(ctx, TIDB_BUILD_STATS_CONCURRENCY)
}

/// 读取采样构建统计并发度配置。
pub fn getBuildSamplingStatsConcurrency<C: AnalyzeSessionContext>(
    ctx: &C,
) -> Result<i64, AnalyzeError> {
    getIntFromSessionVars(ctx, TIDB_BUILD_SAMPLING_STATS_CONCURRENCY)
}

/// 判断错误是否为 worker panic 或 ANALYZE OOM。
pub fn isAnalyzeWorkerPanic(error: &AnalyzeError) -> bool {
    matches!(
        error.kind,
        AnalyzeErrorKind::AnalyzeWorkerPanic | AnalyzeErrorKind::AnalyzeOutOfMemory
    )
}

/// 将 catch_unwind 载荷映射为具体 ANALYZE 错误（含 OOM 文案识别）。
pub fn getAnalyzePanicErr(value: &(dyn Any + Send)) -> AnalyzeError {
    if let Some(message) = value.downcast_ref::<String>() {
        if message == GLOBAL_PANIC_ANALYZE_MEMORY_EXCEED {
            return errAnalyzeOOM();
        }
    }
    if let Some(message) = value.downcast_ref::<&'static str>() {
        if *message == GLOBAL_PANIC_ANALYZE_MEMORY_EXCEED {
            return errAnalyzeOOM();
        }
    }
    if let Some(error) = value.downcast_ref::<AnalyzeError>() {
        if error.message == GLOBAL_PANIC_ANALYZE_MEMORY_EXCEED {
            return errAnalyzeOOM();
        }
        return error.clone();
    }
    errAnalyzeWorkerPanic()
}

/// 取消/超时时若上下文有 cause，则用 cause 替换表面错误。
pub fn normalizeCtxErrWithCause(
    ctx: &AnalyzeContext,
    error: Option<AnalyzeError>,
) -> Option<AnalyzeError> {
    let error = error?;
    if matches!(
        error.kind,
        AnalyzeErrorKind::Canceled | AnalyzeErrorKind::DeadlineExceeded
    ) {
        if let Some(cause) = &ctx.cause {
            return Some(cause.clone());
        }
    }
    Some(error)
}

/// 通知通道内部队列状态。
struct NotifyState<T> {
    queue: VecDeque<T>,
    closed: bool,
}

/// 带 Condvar 的通知通道共享状态。
struct NotifyInner<T> {
    state: Mutex<NotifyState<T>>,
    available: Condvar,
}

/// 可关闭的多生产者/单消费者风格通知通道（基于 Mutex + Condvar）。
pub struct NotifyChannel<T> {
    inner: Arc<NotifyInner<T>>,
}

impl<T> Clone for NotifyChannel<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> Default for NotifyChannel<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> NotifyChannel<T> {
    /// 创建空的通知通道。
    pub fn new() -> Self {
        Self {
            inner: Arc::new(NotifyInner {
                state: Mutex::new(NotifyState {
                    queue: VecDeque::new(),
                    closed: false,
                }),
                available: Condvar::new(),
            }),
        }
    }

    /// 发送一项；通道已关闭则原样退回值。
    pub fn send(&self, value: T) -> Result<(), T> {
        let mut state = self.inner.state.lock().expect("notify channel poisoned");
        if state.closed {
            return Err(value);
        }
        state.queue.push_back(value);
        self.inner.available.notify_one();
        Ok(())
    }

    /// 阻塞接收；关闭且队列空时返回 `None`。
    pub fn recv(&self) -> Option<T> {
        let mut state = self.inner.state.lock().expect("notify channel poisoned");
        loop {
            if let Some(value) = state.queue.pop_front() {
                return Some(value);
            }
            if state.closed {
                return None;
            }
            state = self
                .inner
                .available
                .wait(state)
                .expect("notify channel poisoned");
        }
    }

    /// 关闭通道并唤醒所有等待者。
    pub fn close(&self) {
        let mut state = self.inner.state.lock().expect("notify channel poisoned");
        state.closed = true;
        self.inner.available.notify_all();
    }

    /// 查询通道是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.inner
            .state
            .lock()
            .expect("notify channel poisoned")
            .closed
    }
}

#[derive(Default)]
/// 类似 Go `sync.WaitGroup` 的计数等待组。
pub struct WaitGroup {
    pending: Mutex<usize>,
    completed: Condvar,
}

impl WaitGroup {
    /// 增减待完成计数；减至 0 时唤醒 Wait。
    pub fn Add(&self, delta: isize) {
        let mut pending = self.pending.lock().expect("wait group poisoned");
        if delta >= 0 {
            *pending = pending
                .checked_add(delta as usize)
                .expect("wait group counter overflow");
        } else {
            *pending = pending
                .checked_sub(delta.unsigned_abs())
                .expect("negative wait group counter");
            if *pending == 0 {
                self.completed.notify_all();
            }
        }
    }

    /// 等价于 `Add(-1)`。
    pub fn Done(&self) {
        self.Add(-1);
    }

    /// 阻塞直到计数归零。
    pub fn Wait(&self) {
        let mut pending = self.pending.lock().expect("wait group poisoned");
        while *pending != 0 {
            pending = self.completed.wait(pending).expect("wait group poisoned");
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 通过通知通道传递的 ANALYZE 结果（此处仅携带错误）。
pub struct AnalyzeResults {
    pub error: Option<AnalyzeError>,
}

/// 任务完成时自动 Done，首个任务还可在全部完成后关闭通道。
struct Completion<T> {
    wait_group: Arc<WaitGroup>,
    notify: NotifyChannel<T>,
    closes_channel: bool,
}

impl<T> Drop for Completion<T> {
    fn drop(&mut self) {
        self.wait_group.Done();
        if self.closes_channel {
            self.wait_group.Wait();
            self.notify.close();
        }
    }
}

/// All task counts must be registered with `Add` before `Run`, matching the Go
/// wrapper. The first registered task closes `notify` only after every task has
/// called `Done`.
pub struct analyzeResultsNotifyWaitGroupWrapper {
    pub WaitGroup: Arc<WaitGroup>,
    pub notify: NotifyChannel<AnalyzeResults>,
    cnt: AtomicU64,
}

/// 构造结果通知 WaitGroup 包装器。
pub fn NewAnalyzeResultsNotifyWaitGroupWrapper(
    notify: NotifyChannel<AnalyzeResults>,
) -> Box<analyzeResultsNotifyWaitGroupWrapper> {
    Box::new(analyzeResultsNotifyWaitGroupWrapper {
        WaitGroup: Arc::new(WaitGroup::default()),
        notify,
        cnt: AtomicU64::new(0),
    })
}

impl analyzeResultsNotifyWaitGroupWrapper {
    /// 登记即将启动的结果 worker 数量。
    pub fn Add(&self, count: usize) {
        self.WaitGroup
            .Add(isize::try_from(count).expect("analyze result worker count exceeds isize::MAX"));
    }

    /// 启动线程执行任务；首个任务负责最终关闭 notify 通道。
    pub fn Run<F>(&self, exec: F) -> JoinHandle<()>
    where
        F: FnOnce() + Send + 'static,
    {
        let old = self.cnt.fetch_add(1, Ordering::SeqCst);
        let completion = Completion {
            wait_group: Arc::clone(&self.WaitGroup),
            notify: self.notify.clone(),
            closes_channel: old == 0,
        };
        thread::spawn(move || {
            let _completion = completion;
            exec();
        })
    }
}

/// 可提交后台任务的线程池抽象。
pub trait WorkerPool: Send + Sync + 'static {
    fn spawn(&self, task: Box<dyn FnOnce() + Send + 'static>);
}

/// 直接 `thread::spawn` 的简单线程池实现。
pub struct ThreadWorkerPool;

impl WorkerPool for ThreadWorkerPool {
    fn spawn(&self, task: Box<dyn FnOnce() + Send + 'static>) {
        thread::spawn(task);
    }
}

/// 在 worker 池上运行任务，并通过通道汇总错误的 WaitGroup 包装。
pub struct notifyErrorWaitGroupWrapper<P: WorkerPool> {
    pub WaitGroupPool: Arc<P>,
    pub WaitGroup: Arc<WaitGroup>,
    pub notify: NotifyChannel<AnalyzeError>,
    cnt: AtomicU64,
}

/// 构造错误通知 WaitGroup 包装器。
pub fn newNotifyErrorWaitGroupWrapper<P: WorkerPool>(
    pool: Arc<P>,
    notify: NotifyChannel<AnalyzeError>,
) -> Box<notifyErrorWaitGroupWrapper<P>> {
    Box::new(notifyErrorWaitGroupWrapper {
        WaitGroupPool: pool,
        WaitGroup: Arc::new(WaitGroup::default()),
        notify,
        cnt: AtomicU64::new(0),
    })
}

impl<P: WorkerPool> notifyErrorWaitGroupWrapper<P> {
    pub fn Add(&self, count: usize) {
        self.WaitGroup
            .Add(isize::try_from(count).expect("analyze error worker count exceeds isize::MAX"));
    }

    pub fn Run<F>(&self, exec: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let old = self.cnt.fetch_add(1, Ordering::SeqCst);
        let completion = Completion {
            wait_group: Arc::clone(&self.WaitGroup),
            notify: self.notify.clone(),
            closes_channel: old == 0,
        };
        self.WaitGroupPool.spawn(Box::new(move || {
            let _completion = completion;
            exec();
        }));
    }
}

/// 协作式取消信号：置位后 check 返回 canceled。
pub struct KillSignal {
    killed: AtomicBool,
}

impl Default for KillSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl KillSignal {
    /// 创建未触发的取消信号。
    pub const fn new() -> Self {
        Self {
            killed: AtomicBool::new(false),
        }
    }

    /// 标记已取消。
    pub fn kill(&self) {
        self.killed.store(true, Ordering::SeqCst);
    }

    /// 若已取消则返回错误，否则成功。
    pub fn check(&self) -> Result<(), AnalyzeError> {
        if self.killed.load(Ordering::SeqCst) {
            Err(AnalyzeError::canceled("analyze killed"))
        } else {
            Ok(())
        }
    }
}
