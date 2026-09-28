// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// TTL worker 通用后台线程抽象：生命周期、取消令牌与消息通道。
//
// `BaseWorker` 在独立线程中运行用户提供的循环函数，支持 start/stop、
// panic 捕获、条件变量等待停止，以及通过 `mpsc` 向循环投递类型擦除消息。

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Worker 线程的生命周期状态。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WorkerStatus {
    /// 已创建但尚未 `start`。
    #[default]
    Created,
    /// 循环线程正在运行。
    Running,
    /// 已请求停止，等待循环退出。
    Stopping,
    /// 已完全停止。
    Stopped,
}

/// Worker 运行期错误分类。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerError {
    /// 循环函数返回的业务/逻辑错误。
    Loop(String),
    /// 循环内发生 panic。
    Panic,
    /// 等待停止超时。
    Timeout,
    /// 调用方取消令牌已取消。
    Canceled,
    /// 消息通道已关闭（通常因 worker 已停止）。
    ChannelClosed,
}

/// 可跨线程共享的取消标志，对应 Go 的 context cancel。
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    /// 置位取消标志（Release 语义）。
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// 查询是否已取消（Acquire 语义）。
    pub fn is_canceled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// 投递给 worker 循环的类型擦除消息。
pub type WorkerMessage = Box<dyn Any + Send>;
/// 在独立线程中执行的循环闭包类型。
type LoopFunction = Box<
    dyn FnOnce(CancellationToken, mpsc::Receiver<WorkerMessage>) -> Result<(), WorkerError> + Send,
>;

/// 受互斥锁保护的可变运行状态。
#[derive(Debug)]
struct WorkerState {
    status: WorkerStatus,
    error: Option<WorkerError>,
    join_handle: Option<JoinHandle<()>>,
}

/// `BaseWorker` 的共享内部实现。
struct WorkerInner {
    state: Mutex<WorkerState>,
    /// 状态变为 Stopped 时唤醒所有等待者。
    stopped: Condvar,
    cancellation: CancellationToken,
    sender: Mutex<Option<mpsc::SyncSender<WorkerMessage>>>,
    receiver: Mutex<Option<mpsc::Receiver<WorkerMessage>>>,
    loop_function: Mutex<Option<LoopFunction>>,
}

/// 可克隆的 worker 句柄；多个克隆共享同一底层线程与通道。
#[derive(Clone)]
pub struct BaseWorker {
    inner: Arc<WorkerInner>,
}

/// Worker 对外能力：启停、状态查询、发消息与等待停止。
pub trait Worker {
    /// 若仍为 Created，则启动循环线程。
    fn start(&self);
    /// 请求停止；Created 直接标为 Stopped，Running 进入 Stopping。
    fn stop(&self);
    /// 当前状态快照。
    fn status(&self) -> WorkerStatus;
    /// 停止后的错误（若有）。
    fn error(&self) -> Option<WorkerError>;
    /// 向运行中循环发送消息。
    fn send(&self, message: WorkerMessage) -> Result<(), WorkerError>;
    /// 在超时内等待进入 Stopped；同时响应调用方取消令牌。
    fn wait_stopped(
        &self,
        context: &CancellationToken,
        timeout: Duration,
    ) -> Result<(), WorkerError>;
}

impl BaseWorker {
    /// 构造 worker：准备通道并将循环闭包存入待启动槽位。
    pub fn new<F>(loop_function: F) -> Self
    where
        F: FnOnce(CancellationToken, mpsc::Receiver<WorkerMessage>) -> Result<(), WorkerError>
            + Send
            + 'static,
    {
        // Go's `chan any` is unbuffered: preserve its rendezvous/backpressure semantics.
        let (sender, receiver) = mpsc::sync_channel(0);
        Self {
            inner: Arc::new(WorkerInner {
                state: Mutex::new(WorkerState {
                    status: WorkerStatus::Created,
                    error: None,
                    join_handle: None,
                }),
                stopped: Condvar::new(),
                cancellation: CancellationToken::default(),
                sender: Mutex::new(Some(sender)),
                receiver: Mutex::new(Some(receiver)),
                loop_function: Mutex::new(Some(Box::new(loop_function))),
            }),
        }
    }

    /// 获取状态锁；若锁被 poison 则恢复内层数据继续使用。
    fn state(&self) -> MutexGuard<'_, WorkerState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 标记 Stopped、记录错误、丢弃 sender 并唤醒等待者。
    fn transition_to_stopped(inner: &WorkerInner, error: Option<WorkerError>) {
        let mut state = inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.status = WorkerStatus::Stopped;
        state.error = error;
        drop(
            inner
                .sender
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take(),
        );
        inner.stopped.notify_all();
    }

    /// 返回与循环共享的取消令牌克隆。
    pub fn cancellation_token(&self) -> CancellationToken {
        self.inner.cancellation.clone()
    }
}

impl Worker for BaseWorker {
    fn start(&self) {
        let mut state = self.state();
        if state.status != WorkerStatus::Created {
            return;
        }
        let Some(loop_function) = self
            .inner
            .loop_function
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        else {
            return;
        };
        let Some(receiver) = self
            .inner
            .receiver
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        else {
            return;
        };
        state.status = WorkerStatus::Running;
        let inner = Arc::clone(&self.inner);
        let cancellation = inner.cancellation.clone();
        // 在独立线程运行循环；panic 转为 WorkerError::Panic。
        state.join_handle = Some(thread::spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| loop_function(cancellation, receiver)));
            let error = match result {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error),
                Err(_) => Some(WorkerError::Panic),
            };
            Self::transition_to_stopped(&inner, error);
        }));
    }

    fn stop(&self) {
        let mut state = self.state();
        match state.status {
            WorkerStatus::Created => {
                // 尚未启动：直接取消并关闭通道。
                self.inner.cancellation.cancel();
                state.status = WorkerStatus::Stopped;
                drop(
                    self.inner
                        .sender
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take(),
                );
                self.inner.stopped.notify_all();
            }
            WorkerStatus::Running => {
                self.inner.cancellation.cancel();
                state.status = WorkerStatus::Stopping;
            }
            WorkerStatus::Stopping | WorkerStatus::Stopped => {}
        }
    }

    fn status(&self) -> WorkerStatus {
        self.state().status
    }

    fn error(&self) -> Option<WorkerError> {
        self.state().error.clone()
    }

    fn send(&self, message: WorkerMessage) -> Result<(), WorkerError> {
        let sender = self
            .inner
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or(WorkerError::ChannelClosed)?;
        sender.send(message).map_err(|_| WorkerError::ChannelClosed)
    }

    fn wait_stopped(
        &self,
        context: &CancellationToken,
        timeout: Duration,
    ) -> Result<(), WorkerError> {
        let mut state = self.state();
        // Go returns success when both worker and caller context have already stopped.
        // 已停止则 join 线程句柄后立即成功返回。
        if state.status == WorkerStatus::Stopped {
            if let Some(handle) = state.join_handle.take() {
                drop(state);
                let _ = handle.join();
            }
            return Ok(());
        }
        let deadline = Instant::now() + timeout;
        // 短切片等待，以便周期性检查调用方取消与超时。
        loop {
            if context.is_canceled() {
                return Err(WorkerError::Canceled);
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(WorkerError::Timeout);
            }
            let slice = deadline
                .saturating_duration_since(now)
                .min(Duration::from_millis(10));
            let waited = self
                .inner
                .stopped
                .wait_timeout(state, slice)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = waited.0;
            if state.status == WorkerStatus::Stopped {
                if let Some(handle) = state.join_handle.take() {
                    drop(state);
                    let _ = handle.join();
                }
                return Ok(());
            }
        }
    }
}
