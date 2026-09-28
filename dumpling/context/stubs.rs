// Copyright 2026 AsterSQL.
//! Local stand-in for Go `context` (stdlib) used by `dumpling/context`.
//!
//! Provides Background / WithCancel / Done / Err / CancelFunc without pulling
//! tokio or other heavy workspace deps (darwin arm64–safe).
//!
//! 这里实现的是 Go `context` 的最小子集，只覆盖 dumpling 当前真正依赖的
//! `Background/WithCancel/Done/Err/CancelFunc` 语义。
//! 它不尝试模拟 channel、deadline 或 value 传递，只保证取消传播和错误可见性。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Error returned by [`Context::Err`] when the context has been cancelled.
/// 错误文本保持 Go 原文，便于 parity test 直接比对。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Canceled;

impl std::fmt::Display for Canceled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("context canceled")
    }
}

impl std::error::Error for Canceled {}

/// Go `context.CancelFunc`. Call [`CancelFunc::call`] (Go: `cancel()`). Idempotent.
/// 显式 `call()` 对应 Go 的 `cancel()`，drop 本身不触发取消。
#[derive(Clone)]
pub struct CancelFunc {
    flag: Arc<AtomicBool>,
}

impl CancelFunc {
    /// Invoke cancellation (Go: `cancel()`). Idempotent.
    pub fn call(&self) {
        // 只翻转自己的取消标记，父子传播由 Context::Done 递归判断负责。
        self.flag.store(true, Ordering::SeqCst);
    }
}

/// Go `context.Context` subset used by dumpling (cancel / Done / Err).
/// 父节点通过 `Arc` 链接，子节点查询 Done 时会沿祖先链向上递归。
#[derive(Clone, Debug, Default)]
pub struct Context {
    /// Own cancel flag (set by the CancelFunc from WithCancel).
    cancelled: Arc<AtomicBool>,
    /// Optional parent; Done when self or any ancestor is cancelled.
    parent: Option<Arc<Context>>,
}

impl Context {
    /// Go `context.Background()`.
    pub fn Background() -> Self {
        // 根上下文没有父节点，也没有任何已取消标记。
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            parent: None,
        }
    }

    /// Go `context.TODO()`.
    pub fn TODO() -> Self {
        // 当前 dumpling 不区分 TODO 与 Background，两者都返回空根上下文。
        Self::Background()
    }

    /// Go `context.WithCancel(parent)`.
    pub fn WithCancel(parent: &Self) -> (Self, CancelFunc) {
        // 子节点自带独立取消位，同时保存父节点引用以实现传播。
        let child = Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            parent: Some(Arc::new(parent.clone())),
        };
        let cancel = CancelFunc {
            flag: child.cancelled.clone(),
        };
        (child, cancel)
    }

    /// Go `ctx.Done()` — true when cancelled (self or parent).
    pub fn Done(&self) -> bool {
        // 先看自己，再看祖先，模拟 Go 中父取消会传递到子级的效果。
        if self.cancelled.load(Ordering::SeqCst) {
            return true;
        }
        match &self.parent {
            Some(p) => p.Done(),
            None => false,
        }
    }

    /// Go `ctx.Err()` — `Some(Canceled)` when Done, else `None`.
    pub fn Err(&self) -> Option<Canceled> {
        // 这里不区分“自己取消”还是“父级传播”，统一暴露 Go 的 canceled 错误。
        if self.Done() { Some(Canceled) } else { None }
    }
}

/// Free function matching Go `context.Background()`.
pub fn Background() -> Context {
    Context::Background()
}

/// Free function matching Go `context.WithCancel(parent)`.
pub fn WithCancel(parent: Context) -> (Context, CancelFunc) {
    // 保留包级自由函数，是为了让调用方式更接近 Go 标准库。
    Context::WithCancel(&parent)
}
