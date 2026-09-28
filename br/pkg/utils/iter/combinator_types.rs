// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! 组合器内部类型，对应 Go `br/pkg/utils/iter` 中的 bufferedMapping / Filter 等实现。
//!
//! 本文件存放 `TryNextor` 的具体结构体；对外工厂函数在 `combinators.rs`。
//! TransformIter 用生产者线程 + WorkerPool 并发执行有副作用的 map，并通过 channel 回传结果。
//! 错误或取消时 Cancel 子 context，并尽快排空/结束，避免调用方永久阻塞。
//! WorkerPool 用自旋+yield 而非条件变量，实现简单但高负载时占 CPU。
//! JoinIter 在子迭代器错误时清空 inner，对应 Go 侧停止后续拼接。
//! FilterMap 的 skip 标志为 true 表示丢弃，与 FilterOut 谓词方向一致（真=丢）。
//! TakeIter 在 n 耗尽后直接 Done，不再向下游拉取。

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;

use crate::iter::{
    Context, Done, DoneBy, Emit, Indexed, IterResult, Throw, TryNextor, convertDoneOrErrResult,
};
use crate::source_types::empty;

/// 简易并发池：限制同时在飞任务数，对齐 Go workerpool 的配额语义。
/// 非公平调度：满员时调用方线程 yield，不保证 FIFO。
/// Clone 共享同一 inflight 计数器，可在多处提交任务。
#[derive(Clone)]
pub struct WorkerPool {
    limit: usize,
    inflight: Arc<AtomicUsize>,
}

impl WorkerPool {
    /// `n` 至少为 1；`_name` 保留与 Go 构造签名对齐。
    /// name 目前未用于指标标签，仅为移植兼容。
    pub fn new(n: u32, _name: &str) -> Self {
        Self {
            limit: n.max(1) as usize,
            inflight: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// 返回并发上限（i32 便于与 bufferSize 比较）。
    pub fn Limit(&self) -> i32 {
        self.limit as i32
    }

    /// 在配额内启动线程执行 `f`；满员时 yield 自旋等待。
    /// f 结束后自动 fetch_sub；panic 路径未捕获（与简化移植一致）。
    pub fn Apply(&self, f: impl FnOnce() + Send + 'static) {
        while self.inflight.load(Ordering::SeqCst) >= self.limit {
            thread::yield_now();
        }
        self.inflight.fetch_add(1, Ordering::SeqCst);
        let inflight = self.inflight.clone();
        thread::spawn(move || {
            f();
            inflight.fetch_sub(1, Ordering::SeqCst);
        });
    }
}

/// Transform 的缓冲/并发配置；quota 为空时由调用方或 start 填默认池。
/// bufferSize 同时约束 channel 反压的在飞任务数。
#[derive(Clone)]
pub struct BufferedMappingCfg {
    pub bufferSize: u32,
    pub quota: Option<WorkerPool>,
}

/// Concurrent impure map matching Go `bufferedMapping`.
/// 并发有副作用映射：首次 TryNext 时启动生产者，结果经 channel 异步送达。
/// started/finished 控制生命周期；pending 预留本地重排，当前多为直通。
/// producer JoinHandle 用于探测线程是否结束，避免无限 recv。
pub struct TransformIter<T, R> {
    cfg: BufferedMappingCfg,
    inner: Option<Box<dyn TryNextor<T>>>,
    mapper: Arc<dyn Fn(&Context, T) -> Result<R, String> + Send + Sync>,
    started: bool,
    finished: bool,
    /// 本地暂存队列（当前实现主要依赖 channel；字段保留扩展）。
    pending: VecDeque<IterResult<R>>,
    cancel: Option<crate::iter::CancelFunc>,
    results_rx: Option<mpsc::Receiver<IterResult<R>>>,
    outstanding: Option<Arc<AtomicUsize>>,
    producer: Option<thread::JoinHandle<()>>,
}

impl<T: Send + 'static, R: Send + 'static> TransformIter<T, R> {
    /// 包装上游迭代器与 mapper；真正的线程在首次 `TryNext` 懒启动。
    /// 懒启动确保配置与调用方 context 在第一次拉取时一并生效。
    pub fn new(
        inner: Box<dyn TryNextor<T>>,
        mapper: impl Fn(&Context, T) -> Result<R, String> + Send + Sync + 'static,
        cfg: BufferedMappingCfg,
    ) -> Self {
        Self {
            cfg,
            inner: Some(inner),
            mapper: Arc::new(mapper),
            started: false,
            finished: false,
            pending: VecDeque::new(),
            cancel: None,
            results_rx: None,
            outstanding: None,
            producer: None,
        }
    }

    /// 启动生产者：拉取上游 → 提交到 WorkerPool → 结果写入 channel。
    /// 子 context 与父联动：父取消或本地 Cancel 都会停止生产循环。
    fn start(&mut self, ctx: &Context) {
        self.started = true;
        // buffer 至少为 1，同时约束在飞任务上限。
        // 未配置 quota 时用 buffer 作为默认并发名 max-concurrency。
        let buffer = self.cfg.bufferSize.max(1) as usize;
        let quota = self
            .cfg
            .quota
            .clone()
            .unwrap_or_else(|| WorkerPool::new(buffer as u32, "max-concurrency"));
        let (res_tx, res_rx) = mpsc::channel::<IterResult<R>>();
        // 子 context：错误或外部取消时双向停止生产。
        // cancel 句柄同时存到 self，供消费者侧主动停止。
        let (child_ctx, cancel) = Context::WithCancel(ctx);
        self.cancel = Some(cancel.clone());
        self.results_rx = Some(res_rx);
        let mut inner = self.inner.take().unwrap();
        let mapper = self.mapper.clone();
        let outstanding = Arc::new(AtomicUsize::new(0));
        self.outstanding = Some(outstanding.clone());
        let active_workers = Arc::new(AtomicUsize::new(0));
        let max_out = buffer;

        self.producer = Some(thread::spawn(move || {
            'produce: loop {
                if child_ctx.Done() {
                    break;
                }
                // 反压：在飞结果达到 buffer 时等待消费者消化。
                // 取消检查放在等待循环内，防止取消后仍占着配额。
                while outstanding.load(Ordering::SeqCst) >= max_out {
                    if child_ctx.Done() {
                        break 'produce;
                    }
                    thread::yield_now();
                }
                outstanding.fetch_add(1, Ordering::SeqCst);
                let r = inner.TryNext(&child_ctx);
                if r.FinishedOrError() {
                    if r.Err.is_some() {
                        // 上游错误：转发并取消，停止后续调度。
                        // DoneBy 去掉 Item，只保留 Err/Finished 语义。
                        let _ = res_tx.send(DoneBy(r));
                        cancel.Cancel();
                    } else {
                        // 正常结束：冲销为反压预占的计数。
                        // 结束分支不再 Apply mapper，直接退出拉流循环。
                        outstanding.fetch_sub(1, Ordering::SeqCst);
                    }
                    break;
                }
                let item = r.Item.unwrap();
                let mapper2 = mapper.clone();
                let res_tx2 = res_tx.clone();
                let cancel2 = cancel.clone();
                let child2 = child_ctx.clone();
                let active_workers2 = active_workers.clone();
                active_workers.fetch_add(1, Ordering::SeqCst);
                quota.Apply(move || {
                    let out = match mapper2(&child2, item) {
                        Ok(v) => Emit(v),
                        Err(e) => {
                            // mapper 失败同样取消，避免并发继续产出。
                            // Throw 经 channel 送达后，消费者会标记 finished。
                            cancel2.Cancel();
                            Throw(e)
                        }
                    };
                    let _ = res_tx2.send(out);
                    active_workers2.fetch_sub(1, Ordering::SeqCst);
                });
            }
            // 等待在飞任务清空后再退出生产者线程。
            // 否则消费者可能在 channel 断开后丢失末尾错误。
            while active_workers.load(Ordering::SeqCst) > 0 {
                thread::yield_now();
            }
        }));
    }
}

impl<T: Send + 'static, R: Send + 'static> TryNextor<R> for TransformIter<T, R> {
    fn TryNext(&mut self, ctx: &Context) -> IterResult<R> {
        if !self.started {
            self.start(ctx);
        }
        // 优先吐本地 pending（若有）。
        // 错误结果出队时同步标记 finished。
        if let Some(p) = self.pending.pop_front() {
            self.outstanding
                .as_ref()
                .expect("started transform has outstanding counter")
                .fetch_sub(1, Ordering::SeqCst);
            if p.Err.is_some() {
                self.finished = true;
            }
            return p;
        }
        loop {
            if self.finished {
                return Done();
            }
            // 调用方 context 取消：主动 Cancel 生产者并抛出取消错误。
            // 返回 Throw(ctx.Err())，与 Go context 取消字符串一致。
            if ctx.Done() {
                if let Some(c) = &self.cancel {
                    c.Cancel();
                }
                self.finished = true;
                return Throw(ctx.Err());
            }
            match self
                .results_rx
                .as_ref()
                .unwrap()
                .recv_timeout(std::time::Duration::from_millis(5))
            {
                Ok(r) => {
                    self.outstanding
                        .as_ref()
                        .expect("started transform has outstanding counter")
                        .fetch_sub(1, Ordering::SeqCst);
                    if r.Err.is_some() {
                        if let Some(c) = &self.cancel {
                            c.Cancel();
                        }
                        self.finished = true;
                    }
                    return r;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // 生产者已结束时再 try_recv，避免假死等超时。
                    // 5ms 超时用于周期性检查取消与 producer 状态。
                    if self
                        .producer
                        .as_ref()
                        .map(|j| j.is_finished())
                        .unwrap_or(false)
                    {
                        match self.results_rx.as_ref().unwrap().try_recv() {
                            Ok(r) => {
                                self.outstanding
                                    .as_ref()
                                    .expect("started transform has outstanding counter")
                                    .fetch_sub(1, Ordering::SeqCst);
                                if r.Err.is_some() {
                                    self.finished = true;
                                }
                                return r;
                            }
                            Err(_) => {
                                self.finished = true;
                                return Done();
                            }
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.finished = true;
                    return Done();
                }
            }
        }
    }
}

/// 谓词为真则丢弃元素（FilterOut 语义）。
/// 错误与 Finished 立即透传，不做过滤。
pub struct FilterIter<T> {
    pub inner: Box<dyn TryNextor<T>>,
    pub filterOutIf: Box<dyn FnMut(&T) -> bool + Send>,
}

impl<T: Send> TryNextor<T> for FilterIter<T> {
    fn TryNext(&mut self, ctx: &Context) -> IterResult<T> {
        loop {
            let r = self.inner.TryNext(ctx);
            if r.Err.is_some() || r.Finished {
                return r;
            }
            // 谓词为假才保留；为真继续拉取下一项。
            // 可能连续跳过多项，直到命中或上游结束。
            if !(self.filterOutIf)(r.Item.as_ref().unwrap()) {
                return r;
            }
        }
    }
}

/// 最多再产出 `n` 个元素后返回 Done。
/// n==0 的初始状态表示不产出任何元素。
pub struct TakeIter<T> {
    pub n: u32,
    pub inner: Box<dyn TryNextor<T>>,
}

impl<T: Send> TryNextor<T> for TakeIter<T> {
    fn TryNext(&mut self, ctx: &Context) -> IterResult<T> {
        if self.n == 0 {
            return Done();
        }
        self.n -= 1;
        self.inner.TryNext(ctx)
    }
}

/// 纯函数映射，不引入并发；结束/错误用 DoneBy 换型。
/// mapper 不可失败；需要 Err 时用 TryMapIter。
pub struct PureMapIter<T, R> {
    pub inner: Box<dyn TryNextor<T>>,
    pub mapper: Box<dyn FnMut(T) -> R + Send>,
}

impl<T: Send, R: Send> TryNextor<R> for PureMapIter<T, R> {
    fn TryNext(&mut self, ctx: &Context) -> IterResult<R> {
        let r = self.inner.TryNext(ctx);
        if r.FinishedOrError() {
            return DoneBy(r);
        }
        Emit((self.mapper)(r.Item.unwrap()))
    }
}

/// Map+Filter：mapper 返回 `(值, skip)`，`skip=true` 时丢弃。
/// 与单独 Map+FilterOut 相比少一次中间分配。
pub struct FilterMapIter<T, R> {
    pub inner: Box<dyn TryNextor<T>>,
    pub mapper: Box<dyn FnMut(T) -> (R, bool) + Send>,
}

impl<T: Send, R: Send> TryNextor<R> for FilterMapIter<T, R> {
    fn TryNext(&mut self, ctx: &Context) -> IterResult<R> {
        loop {
            let r = self.inner.TryNext(ctx);
            if r.FinishedOrError() {
                return DoneBy(r);
            }
            let (res, skip) = (self.mapper)(r.Item.unwrap());
            if !skip {
                return Emit(res);
            }
        }
    }
}

/// Fallible map：Err 转为 Throw，不吞掉上游结束态。
/// 上游 FinishedOrError 优先于 mapper 调用。
pub struct TryMapIter<T, R> {
    pub inner: Box<dyn TryNextor<T>>,
    pub mapper: Box<dyn FnMut(T) -> Result<R, String> + Send>,
}

impl<T: Send, R: Send> TryNextor<R> for TryMapIter<T, R> {
    fn TryNext(&mut self, ctx: &Context) -> IterResult<R> {
        let r = self.inner.TryNext(ctx);
        if r.FinishedOrError() {
            return DoneBy(r);
        }
        match (self.mapper)(r.Item.unwrap()) {
            Ok(v) => Emit(v),
            Err(e) => Throw(e),
        }
    }
}

/// 展平嵌套迭代器：current 耗尽后从 inner 取下一个子迭代器。
/// FlatMap/ConcatAll 均构建在 JoinIter 之上。
pub struct JoinIter<T> {
    pub inner: Box<dyn TryNextor<Box<dyn TryNextor<T>>>>,
    pub current: Box<dyn TryNextor<T>>,
}

impl<T: Send + 'static> TryNextor<T> for JoinIter<T> {
    fn TryNext(&mut self, ctx: &Context) -> IterResult<T> {
        let r = self.current.TryNext(ctx);
        if r.Err.is_some() {
            // 子迭代器出错：清空后续 inner，防止继续产出部分结果。
            // empty() 使后续 TryNext 直接结束。
            self.inner = empty();
            return r;
        }
        if !r.Finished {
            return r;
        }
        let nr = self.inner.TryNext(ctx);
        if nr.FinishedOrError() {
            return DoneBy(nr);
        }
        self.current = nr.Item.unwrap();
        // 递归取下一项，跳过空子迭代器。
        // 深层嵌套时依赖栈；与 Go 循环版语义等价。
        self.TryNext(ctx)
    }
}

/// 为元素附加从 0 递增的 Index，供 Enumerate 使用。
/// Index 在成功 Emit 后自增，失败/结束不递增。
pub struct WithIndexIter<T> {
    pub inner: Box<dyn TryNextor<T>>,
    pub index: i32,
}

impl<T: Send> TryNextor<Indexed<T>> for WithIndexIter<T> {
    fn TryNext(&mut self, ctx: &Context) -> IterResult<Indexed<T>> {
        let r = self.inner.TryNext(ctx);
        if r.Finished || r.Err.is_some() {
            return convertDoneOrErrResult(r);
        }
        let res = Emit(Indexed {
            Index: self.index,
            Item: r.Item.unwrap(),
        });
        self.index += 1;
        res
    }
}
