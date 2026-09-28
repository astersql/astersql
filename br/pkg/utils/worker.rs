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

//! Worker/panic helpers ported from `br/pkg/utils/worker.go`.
//!
//! 提供 panic 捕获、异步流与工作令牌通道，支撑 BR 多 worker 并发调度。
//! Panic 转为 ErrUnknown 或仅记日志，避免单任务 panic 拖垮进程。
//! 令牌通道预填充容量，用条件变量背压限制同时进行的 worker 数。
//! AsyncStreamBy 把阻塞生成器移出调用线程，错误以末帧形式送达接收端。

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;

use astersql_br_pkg_errors::ErrUnknown;
use astersql_br_pkg_logutil::{Field, ShortError, log};
use astersql_errors::{Annotate, SharedError};

/// 默认令牌通道容量；size==0 时回退到此值。
pub const DefaultWorkerTokenChannelSize: u32 = 128;
/// 令牌通道上限，防止异常配置耗尽内存。
pub const MaxWorkerTokenChannelSize: u32 = 30 * 1024 * 1024;

// 将 panic payload 尽量还原为可读字符串（&str / String / Debug）。
fn panic_message(payload: Box<dyn Any + Send>) -> String {
    match payload.downcast::<&'static str>() {
        Ok(message) => (*message).to_string(),
        Err(payload) => match payload.downcast::<String>() {
            Ok(message) => (*message).clone(),
            Err(payload) => format!("{payload:?}"),
        },
    }
}

// panic → Annotated ErrUnknown，文案前缀与 Go PanicToErr 对齐。
fn panic_into_shared_error(payload: Box<dyn Any + Send>) -> SharedError {
    let message = panic_message(payload);
    Annotate(
        Some(SharedError::new((*ErrUnknown).clone())),
        format!("panicked when executing, message: {message}"),
    )
    .expect("annotate panic")
}

/// Runs `f` and converts panics into `ErrUnknown`, matching Go `defer PanicToErr(&err)`.
/// 恢复后打 Warn，调用方可继续上报错误而不中止进程。
/// 使用 AssertUnwindSafe：调用方需保证 f 在 unwind 后状态可接受。
pub fn PanicToErr<F, T>(f: F) -> std::result::Result<T, SharedError>
where
    F: FnOnce() -> T + panic::UnwindSafe,
{
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => Ok(value),
        Err(payload) => {
            let err = panic_into_shared_error(payload);
            log::Warn(
                "PanicToErr: panicked, recovering and returning error",
                [ShortError(Some(&err))],
            );
            Err(err)
        }
    }
}

/// Runs `f` and logs panics without propagating them.
/// 适合「尽力而为」路径：失败只记日志，返回 None。
pub fn CatchAndLogPanic<F, T>(f: F) -> Option<T>
where
    F: FnOnce() -> T + panic::UnwindSafe,
{
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => Some(value),
        Err(payload) => {
            log::Warn(
                "CatchAndLogPanic: panicked, but ignored.",
                [Field::string("panic", &panic_message(payload))],
            );
            None
        }
    }
}

/// 异步流元素：成功带 Item，失败带 Err（Item 为 Default）。
/// 字段名保持 Go 风格 `Err`/`Item`，便于对照移植代码。
#[derive(Clone, Debug)]
pub struct Result<T> {
    pub Err: Option<SharedError>,
    pub Item: T,
}

/// Streams items produced by `generator` from a background thread.
/// 生成器返回 Err 时发送错误帧并结束；接收端关闭则后台线程退出。
pub fn AsyncStreamBy<T, F>(mut generator: F) -> mpsc::Receiver<Result<T>>
where
    T: Default + Send + 'static,
    F: FnMut() -> std::result::Result<T, SharedError> + Send + 'static,
{
    // Go uses an unbuffered channel here: each produced frame must be consumed
    // before the generator advances, bounding memory and preserving backpressure.
    let (out_tx, out_rx) = mpsc::sync_channel(0);
    // 后台线程独占 generator，调用方只读 Receiver。
    thread::spawn(move || {
        loop {
            match generator() {
                Ok(item) => {
                    if out_tx
                        .send(Result {
                            Err: None,
                            Item: item,
                        })
                        .is_err()
                    {
                        // 接收端已丢弃，停止生产避免堆积。
                        return;
                    }
                }
                Err(err) => {
                    // 错误帧后不再继续，对齐 Go 流终止语义。
                    // Item 填 Default，避免 Option 包裹破坏 Go 形态。
                    let _ = out_tx.send(Result {
                        Err: Some(err),
                        Item: T::default(),
                    });
                    return;
                }
            }
        }
    });
    out_rx
}

#[derive(Debug)]
struct WorkerTokenState {
    available: u32,
    capacity: u32,
}

/// A cloneable, bounded token channel with the same take/return semantics as a
/// prefilled Go `chan struct{}`.
#[derive(Clone, Debug)]
pub struct WorkerTokenChannel {
    state: Arc<(Mutex<WorkerTokenState>, Condvar)>,
}

impl WorkerTokenChannel {
    /// Blocks until a token is available, then removes it from the channel.
    pub fn acquire(&self) {
        let (state, changed) = &*self.state;
        let mut guard = state.lock().unwrap();
        while guard.available == 0 {
            guard = changed.wait(guard).unwrap();
        }
        guard.available -= 1;
        changed.notify_all();
    }

    /// Removes a token without blocking, returning whether one was available.
    pub fn try_acquire(&self) -> bool {
        let (state, changed) = &*self.state;
        let mut guard = state.lock().unwrap();
        if guard.available == 0 {
            return false;
        }
        guard.available -= 1;
        changed.notify_all();
        true
    }

    /// Returns a token, blocking while the bounded channel is full.
    pub fn release(&self) {
        let (state, changed) = &*self.state;
        let mut guard = state.lock().unwrap();
        while guard.available == guard.capacity {
            guard = changed.wait(guard).unwrap();
        }
        guard.available += 1;
        changed.notify_all();
    }

    pub fn available(&self) -> u32 {
        self.state.0.lock().unwrap().available
    }

    pub fn capacity(&self) -> u32 {
        self.state.0.lock().unwrap().capacity
    }
}

/// Builds a bounded token channel prefilled with `size` tokens.
pub fn BuildWorkerTokenChannel(mut size: u32) -> WorkerTokenChannel {
    if size == 0 {
        // 0 视为未配置，回退默认并告警。
        size = DefaultWorkerTokenChannelSize;
        log::Warn(
            "build worker token channel: size is 0, set to default value",
            [Field::uint64("new size", size as u64)],
        );
    }
    if size > MaxWorkerTokenChannelSize {
        // 钳制到上限，避免超大缓冲拖垮进程。
        size = MaxWorkerTokenChannelSize;
        log::Warn(
            "build worker token channel: size is greater than max value, set to max value",
            [Field::uint64("new size", size as u64)],
        );
    }
    WorkerTokenChannel {
        state: Arc::new((
            Mutex::new(WorkerTokenState {
                available: size,
                capacity: size,
            }),
            Condvar::new(),
        )),
    }
}
