// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! 迭代器核心类型：Context / IterResult / TryNextor，对应 Go `br/pkg/utils/iter`。
//!
//! Context 模仿 Go context：子取消不上传父；父取消经 parent 链使子 Done。
//! IterResult 三态：Emit(item) / Throw(err) / Done；CollectAll 遇错不返回部分结果。
//! Tap/WithEmitSizeTrace 放在本文件，因只依赖核心 trait，不依赖组合器类型。
//! CancelFunc 可 Clone：多处持有同一取消闭包，便于超时线程与调用方共享。

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// 迭代器错误类型；当前用 String 承载，对齐 Go error.Error() 字符串比较。
pub type IterError = String;

/// 可取消上下文；`cancelled` 仅表示本节点，祖先取消通过 parent 递归查询。
/// Default 与 background 等价：未取消且无父。
#[derive(Clone, Default)]
pub struct Context {
    cancelled: Arc<AtomicBool>,
    /// Ancestor context; Done when self or any ancestor is cancelled (Go semantics).
    /// 祖先取消会沿 parent 传播到子；子取消不影响父。
    /// parent 以 Box 持有，Clone 时深拷贝祖先链上的 Arc 标志共享。
    parent: Option<Box<Context>>,
}

impl Context {
    /// 永不取消的根上下文（Go `context.Background`）。
    pub fn background() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            parent: None,
        }
    }

    /// 自身或任一祖先已取消则为 true。
    pub fn Done(&self) -> bool {
        if self.cancelled.load(Ordering::SeqCst) {
            return true;
        }
        self.parent.as_ref().is_some_and(|p| p.Done())
    }

    /// 取消时的标准错误文案，与 Go `context.Canceled` 字符串一致。
    pub fn Err(&self) -> IterError {
        "context canceled".into()
    }

    /// Go `context.WithCancel`: cancel closes only the child; parent cancellation
    /// propagates into the child via [`Done`], never the reverse.
    /// 返回子 context 与仅取消该子的 CancelFunc。
    /// 子持有父的克隆，因此父 Cancel 后子 Done() 为真。
    pub fn WithCancel(ctx: &Context) -> (Context, CancelFunc) {
        let child = Context {
            cancelled: Arc::new(AtomicBool::new(false)),
            parent: Some(Box::new(ctx.clone())),
        };
        let flag = child.cancelled.clone();
        (
            child,
            CancelFunc {
                cancel: Arc::new(move || {
                    flag.store(true, Ordering::SeqCst);
                }),
            },
        )
    }
}

/// 取消函数句柄；可 Clone，多次 Cancel 幂等（写 true）。
#[derive(Clone)]
pub struct CancelFunc {
    cancel: Arc<dyn Fn() + Send + Sync>,
}

impl CancelFunc {
    /// 标记关联 Context 已取消。
    pub fn Cancel(&self) {
        (self.cancel)();
    }
}

/// 单步迭代结果：Item / Err / Finished 互斥组合（Emit/Throw/Done）。
/// 非法组合（如 Finished 且带 Item）不应由构造函数产生。
#[derive(Debug)]
pub struct IterResult<T> {
    pub Item: Option<T>,
    pub Err: Option<IterError>,
    pub Finished: bool,
}

impl<T: fmt::Debug> fmt::Display for IterResult<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(err) = &self.Err {
            return write!(f, "IterResult.Throw({err})");
        }
        if self.Finished {
            return write!(f, "IterResult.Done()");
        }
        write!(f, "IterResult.Emit({:?})", self.Item.as_ref().unwrap())
    }
}

/// 保留 Err/Finished，丢弃 Item，用于换元素类型时传递结束态。
pub fn convertDoneOrErrResult<T, R>(r: IterResult<T>) -> IterResult<R> {
    IterResult {
        Item: None,
        Err: r.Err,
        Finished: r.Finished,
    }
}

/// Enumerate 产出的带下标元素。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Indexed<T> {
    pub Index: i32,
    pub Item: T,
}

/// 可拉取下一元素的迭代器 trait；实现必须 Send 以便 Transform 跨线程移动。
pub trait TryNextor<T>: Send {
    fn TryNext(&mut self, ctx: &Context) -> IterResult<T>;
}

impl<T> IterResult<T> {
    /// 已结束或出错，调用方不应再期望 Item。
    pub fn FinishedOrError(&self) -> bool {
        self.Err.is_some() || self.Finished
    }
}

/// 从另一类型结果拷贝结束/错误态（Item 置空）。
pub fn DoneBy<T, O>(r: IterResult<O>) -> IterResult<T> {
    IterResult {
        Item: None,
        Err: r.Err,
        Finished: r.Finished,
    }
}

/// 构造正常结束结果。
pub fn Done<T>() -> IterResult<T> {
    IterResult {
        Item: None,
        Err: None,
        Finished: true,
    }
}

/// 构造产出一个元素的结果。
pub fn Emit<T>(t: T) -> IterResult<T> {
    IterResult {
        Item: Some(t),
        Err: None,
        Finished: false,
    }
}

/// 构造错误结果（Finished=false，与 Go Throw 一致）。
pub fn Throw<T>(err: IterError) -> IterResult<T> {
    IterResult {
        Item: None,
        Err: Some(err),
        Finished: false,
    }
}

/// 拉取直至 Done；中途 Throw 则丢弃已收集项，只返回错误态。
pub fn CollectAll<T>(ctx: &Context, it: &mut dyn TryNextor<T>) -> IterResult<Vec<T>> {
    let mut items = Vec::new();
    loop {
        let ir = it.TryNext(ctx);
        if ir.Finished {
            break;
        }
        if ir.Err.is_some() {
            return DoneBy(ir);
        }
        items.push(ir.Item.unwrap());
    }
    // 成功收集完成时 Finished=false，Item=Some(vec)，对齐 Go CollectAll。
    // 若需“流结束”语义，调用方应看上游 Done，而非本结果的 Finished。
    IterResult {
        Item: Some(items),
        Err: None,
        Finished: false,
    }
}

/// Rust pull-iterator adapter for Go's `iter.Seq2[error, T]` contract.
///
/// An emitted item becomes `Ok(T)`, an iterator error becomes `Err(String)`, and
/// `Done` ends the sequence. Like Go `AsSeq`, an error does not itself exhaust
/// the source: a consumer that asks for another item continues advancing it.
pub struct AsSeqIter<T> {
    ctx: Context,
    inner: Box<dyn TryNextor<T>>,
    finished: bool,
}

impl<T> Iterator for AsSeqIter<T> {
    type Item = Result<T, IterError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }

        let result = self.inner.TryNext(&self.ctx);
        if let Some(err) = result.Err {
            return Some(Err(err));
        }
        if result.Finished {
            self.finished = true;
            return None;
        }
        Some(Ok(result.Item.expect("Emit must contain an item")))
    }
}

/// Adapt an impure iterator to Rust's standard pull-iterator interface.
pub fn AsSeq<T>(ctx: &Context, i: Box<dyn TryNextor<T>>) -> AsSeqIter<T> {
    AsSeqIter {
        ctx: ctx.clone(),
        inner: i,
        finished: false,
    }
}

/// 内部 Tap 包装：在 Emit 路径上对元素执行副作用闭包。
struct Tap<T> {
    inner: Box<dyn TryNextor<T>>,
    tapper: Box<dyn FnMut(&T) + Send>,
}

impl<T> TryNextor<T> for Tap<T> {
    fn TryNext(&mut self, ctx: &Context) -> IterResult<T> {
        let n = self.inner.TryNext(ctx);
        if n.FinishedOrError() {
            return n;
        }
        let item = n.Item.unwrap();
        (self.tapper)(&item);
        Emit(item)
    }
}

/// 观察每个产出元素而不改变流内容；常用于统计/日志。
/// 闭包在 Emit 路径调用；Finished/Err 不触发。
pub fn Tap<T: 'static>(
    i: Box<dyn TryNextor<T>>,
    with: impl FnMut(&T) + Send + 'static,
) -> Box<dyn TryNextor<T>> {
    Box::new(Tap {
        inner: i,
        tapper: Box::new(with),
    })
}

/// 元素可报告 Size，供 WithEmitSizeTrace 累加。
pub trait HasSize {
    fn Size(&self) -> i32;
}

/// 每 Emit 一次调用 counter(Size)，用于字节/行数类指标。
pub fn WithEmitSizeTrace<T: HasSize + 'static>(
    it: Box<dyn TryNextor<T>>,
    mut counter: impl FnMut(f64) + Send + 'static,
) -> Box<dyn TryNextor<T>> {
    Tap(it, move |t| counter(t.Size() as f64))
}
