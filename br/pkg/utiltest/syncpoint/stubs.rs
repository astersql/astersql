// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! syncpoint 本地 context 桩：对齐 Go `context.Context` / `AfterFunc` 的取消语义。
//! 仅覆盖 BeginSeq 取消监听所需能力，不接入 kv/domain/kvproto/grpcio。
//! 选择本地桩是为了在 darwin arm64 上避开重型依赖重建路径。
//! 错误文案与 deadline 字符串刻意贴近 Go，便于 parity 断言对照。
//! CancelHandle 与 Context 共享 inner，取消对所有 Clone 立即可见。
//! after_func 是 BeginSeq 取消监听的唯一依赖，不实现完整 context 树。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

/// 可取消上下文：对应 Go `context.Context` 在 syncpoint 中的最小子集。
/// Clone 共享同一 `ContextInner`，保证取消对所有持有者可见。
#[derive(Clone)]
pub struct Context {
    inner: Arc<ContextInner>,
}

/// 内部状态：取消标志、首个错误文案、以及 Condvar 等待对。
struct ContextInner {
    cancelled: AtomicBool,
    err: Mutex<Option<String>>,
    pair: (Mutex<()>, Condvar),
}

impl Default for Context {
    fn default() -> Self {
        Self::background()
    }
}

impl Context {
    /// 永不自动取消的背景上下文，对应 Go `context.Background`。
    pub fn background() -> Self {
        Self {
            inner: Arc::new(ContextInner {
                cancelled: AtomicBool::new(false),
                err: Mutex::new(None),
                pair: (Mutex::new(()), Condvar::new()),
            }),
        }
    }

    /// 返回上下文与取消句柄；句柄可跨线程触发取消。
    pub fn with_cancel() -> (Self, CancelHandle) {
        let ctx = Self::background();
        let handle = CancelHandle {
            inner: Arc::clone(&ctx.inner),
        };
        (ctx, handle)
    }

    /// Rough equivalent of `context.WithTimeout`.
    /// 超时后后台线程写入 `context deadline exceeded`，与 Go 文案对齐。
    pub fn with_timeout(timeout: Duration) -> (Self, CancelHandle) {
        let (ctx, handle) = Self::with_cancel();
        let handle_bg = handle.clone();
        thread::spawn(move || {
            thread::sleep(timeout);
            handle_bg.cancel_with("context deadline exceeded");
        });
        (ctx, handle)
    }

    /// 是否已取消；供调用方轮询，不阻塞。
    pub fn is_done(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// 读取首次取消时记录的错误文案；未取消则为 None。
    pub fn err_message(&self) -> Option<String> {
        self.inner.err.lock().unwrap().clone()
    }

    /// 阻塞直到取消；对应 Go 侧对 `Done` channel 的等待。
    pub fn wait_cancelled(&self) {
        let (lock, cv) = &self.inner.pair;
        let mut guard = lock.lock().unwrap();
        while !self.inner.cancelled.load(Ordering::SeqCst) {
            guard = cv.wait(guard).unwrap();
        }
    }
}

/// 取消句柄：与 Context 共享 inner，可在任意线程调用 cancel。
#[derive(Clone)]
pub struct CancelHandle {
    inner: Arc<ContextInner>,
}

impl CancelHandle {
    /// 使用默认文案 `context canceled` 取消，对齐 Go `cancel()`。
    pub fn cancel(&self) {
        self.cancel_with("context canceled");
    }

    /// 写入首个错误后置位取消并唤醒所有等待者；后续取消不覆盖文案。
    pub fn cancel_with(&self, msg: impl Into<String>) {
        {
            let mut err = self.inner.err.lock().unwrap();
            // 仅记录首次原因，对齐 Go context 首错保留。
            if err.is_none() {
                *err = Some(msg.into());
            }
        }
        // 先写错后置位，避免等待方看到取消却读到空 err。
        self.inner.cancelled.store(true, Ordering::SeqCst);
        self.inner.pair.1.notify_all();
    }
}

/// Stop handle for [`after_func`], matching Go `context.AfterFunc` return value.
/// `stop` 原子标志用于与回调竞态：谁先 swap 到 true 谁赢得执行权或停止权。
pub struct StopWatch {
    stop: Arc<AtomicBool>,
}

impl StopWatch {
    /// Stops the watch. Returns true if the callback had not yet run (Go `Stop`).
    /// 返回 true 表示回调尚未执行（与 Go `Stop` 布尔语义一致）。
    pub fn stop(&self) -> bool {
        !self.stop.swap(true, Ordering::SeqCst)
    }
}

/// Registers `f` to run once when `ctx` is cancelled, unless [`StopWatch::stop`] wins.
///
/// Matches Go `context.AfterFunc(ctx, f)`.
/// 后台线程 Condvar 等待取消；`stop` 与取消竞态时仅一方执行回调。
/// 短超时 wait 仅为避免永久阻塞，不改变“取消后至多执行一次”的契约。
pub fn after_func(ctx: Context, f: impl FnOnce() + Send + 'static) -> StopWatch {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_watch = Arc::clone(&stop);
    thread::spawn(move || {
        let (lock, cv) = &ctx.inner.pair;
        let mut guard = lock.lock().unwrap();
        loop {
            if stop_watch.load(Ordering::SeqCst) {
                return;
            }
            if ctx.inner.cancelled.load(Ordering::SeqCst) {
                drop(guard);
                // Only run if we are the first to claim the slot (not already stopped).
                // swap 已为 true 说明 Stop 抢先，必须跳过回调以免双重执行。
                if stop_watch.swap(true, Ordering::SeqCst) {
                    return;
                }
                f();
                return;
            }
            let (g, _) = cv
                .wait_timeout(guard, Duration::from_millis(20))
                .expect("context wait poisoned");
            guard = g;
        }
    });
    StopWatch { stop }
}
