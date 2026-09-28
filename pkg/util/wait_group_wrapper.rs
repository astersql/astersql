// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// WaitGroup 封装：并发任务计数等待、增强标签追踪、线程池与错误组。
//
// 对齐 Go `pkg/util` 中 WaitGroupWrapper / Enhanced / Pool / ErrorGroupWithRecover：
// 在 `Add`/`Done`/`Wait` 之上提供 `Run`/`RunWithRecover`，以及退出检查与 panic 恢复。

use std::any::Any;
use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Result, anyhow};
use crossbeam_channel::{Receiver, Sender, unbounded};
use tokio_util::sync::CancellationToken;

/// catch_unwind 捕获的 panic 载荷类型别名。
pub type PanicPayload = Box<dyn Any + Send + 'static>;

/// WaitGroup 内部计数与条件变量状态。
#[derive(Default)]
struct WaitState {
    count: Mutex<usize>,
    done: Condvar,
}

/// 类似 Go `sync.WaitGroup`：计数待完成任务并阻塞等待归零。
#[derive(Clone, Default)]
pub struct WaitGroup {
    state: Arc<WaitState>,
}

impl WaitGroup {
    /// 将计数增加 `delta`（溢出则 panic）。
    pub fn Add(&self, delta: usize) {
        let mut count = self.state.count.lock().expect("wait group mutex poisoned");
        *count = count
            .checked_add(delta)
            .expect("wait group counter overflow");
    }

    /// 完成一个任务：计数减一，归零时唤醒所有 Wait 者。
    pub fn Done(&self) {
        let mut count = self.state.count.lock().expect("wait group mutex poisoned");
        assert!(*count > 0, "negative WaitGroup counter");
        *count -= 1;
        if *count == 0 {
            self.state.done.notify_all();
        }
    }

    /// 阻塞直到计数变为 0。
    pub fn Wait(&self) {
        let mut count = self.state.count.lock().expect("wait group mutex poisoned");
        while *count != 0 {
            count = self
                .state
                .done
                .wait(count)
                .expect("wait group mutex poisoned");
        }
    }
}

/// RAII：析构时调用 `Done`，保证线程退出路径也会减计数。
struct DoneGuard(WaitGroup);

impl Drop for DoneGuard {
    fn drop(&mut self) {
        self.0.Done();
    }
}

/// 在独立线程中运行闭包，并用 WaitGroup 等待全部结束。
#[derive(Clone, Default)]
pub struct WaitGroupWrapper {
    wait_group: WaitGroup,
}

impl WaitGroupWrapper {
    /// 启动线程执行 `exec`，计数 +1，结束时自动 Done。
    pub fn Run<F>(&self, exec: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.wait_group.Add(1);
        let wait_group = self.wait_group.clone();
        thread::spawn(move || {
            let _done = DoneGuard(wait_group);
            exec();
        });
    }

    /// 同 `Run`，但用 `catch_unwind` 捕获 panic 并写 error 日志。
    pub fn RunWithLog<F>(&self, exec: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.wait_group.Add(1);
        let wait_group = self.wait_group.clone();
        thread::spawn(move || {
            let _done = DoneGuard(wait_group);
            if let Err(payload) = catch_unwind(AssertUnwindSafe(exec)) {
                log::error!("panic in the wait group: {}", panic_message(&payload));
            }
        });
    }

    /// 同 `Run`，panic 后可选调用 `recover_fn`，并记录错误日志。
    pub fn RunWithRecover<F, R>(&self, exec: F, recover_fn: Option<R>)
    where
        F: FnOnce() + Send + 'static,
        R: FnOnce(Option<PanicPayload>) + Send + 'static,
    {
        self.wait_group.Add(1);
        let wait_group = self.wait_group.clone();
        thread::spawn(move || {
            let _done = DoneGuard(wait_group);
            let result = catch_unwind(AssertUnwindSafe(exec));
            let had_panic = result.is_err();
            let payload = result.err();
            if let Some(recover_fn) = recover_fn {
                recover_fn(payload);
            }
            if had_panic {
                log::error!("panic in the wait group");
            }
        });
    }

    /// 等待所有通过本 wrapper 启动的任务结束。
    pub fn Wait(&self) {
        self.wait_group.Wait();
    }
}

/// 带进程标签注册的增强 WaitGroup：可在退出信号后检查未退出后台任务。
pub struct WaitGroupEnhancedWrapper {
    wait_group: WaitGroup,
    source: String,
    register_process: Mutex<HashSet<String>>,
}

/// 增强版 Done：退出时从注册表移除标签并 Done。
struct EnhancedDoneGuard {
    wrapper: Arc<WaitGroupEnhancedWrapper>,
    label: String,
}

impl Drop for EnhancedDoneGuard {
    fn drop(&mut self) {
        self.wrapper.on_exit(&self.label);
        self.wrapper.wait_group.Done();
    }
}

/// 构造增强 WaitGroup；`exited_check` 为真时启动后台线程监控退出通道。
pub fn NewWaitGroupEnhancedWrapper(
    source: String,
    exit: Option<Receiver<()>>,
    exited_check: bool,
) -> Arc<WaitGroupEnhancedWrapper> {
    let wrapper = Arc::new(WaitGroupEnhancedWrapper {
        wait_group: WaitGroup::default(),
        source,
        register_process: Mutex::new(HashSet::new()),
    });
    if exited_check {
        // 收到 exit 信号后周期性检查是否仍有未注销的后台进程标签。
        let exit = exit.expect("exit receiver is required when exit checking is enabled");
        wrapper.wait_group.Add(1);
        let cloned = Arc::clone(&wrapper);
        thread::spawn(move || cloned.check_unexited_process(exit));
    }
    wrapper
}

impl WaitGroupEnhancedWrapper {
    /// 阻塞等待 exit 信号，之后轮询未退出进程直到注册表清空。
    fn check_unexited_process(self: Arc<Self>, exit: Receiver<()>) {
        let _done = DoneGuard(self.wait_group.clone());
        log::info!(
            "waitGroupWrapper enable exit-checking; source={}",
            self.source
        );
        let _ = exit.recv();
        log::info!(
            "waitGroupWrapper start exit-checking; source={}",
            self.source
        );
        while self.check() {
            thread::sleep(Duration::from_secs(2));
        }
        log::info!(
            "waitGroupWrapper exit-checking exited; source={}",
            self.source
        );
    }

    /// 若仍有注册进程则告警并返回 true；否则返回 false 结束检查循环。
    pub fn check(&self) -> bool {
        let processes = self
            .register_process
            .lock()
            .expect("enhanced wait group mutex poisoned");
        if processes.is_empty() {
            log::info!(
                "waitGroupWrapper finish checking unexited process; source={}",
                self.source
            );
            false
        } else {
            log::warn!(
                "background process unexited while received exited signal; process={:?}; source={}",
                *processes,
                self.source
            );
            true
        }
    }

    /// 注册后台进程标签；重复标签视为编程错误。
    fn on_start(&self, label: &str) {
        let mut processes = self
            .register_process
            .lock()
            .expect("enhanced wait group mutex poisoned");
        assert!(
            processes.insert(label.to_owned()),
            "WaitGroupEnhancedWrapper received duplicated source process: {label}"
        );
        log::info!(
            "background process started; source={}; process={label}",
            self.source
        );
    }

    /// 进程结束时从注册表移除标签。
    fn on_exit(&self, label: &str) {
        self.register_process
            .lock()
            .expect("enhanced wait group mutex poisoned")
            .remove(label);
        log::info!(
            "background process exited; source={}; process={label}",
            self.source
        );
    }

    /// 带标签启动后台任务；退出时自动注销并 Done。
    pub fn Run<F>(self: &Arc<Self>, exec: F, label: String)
    where
        F: FnOnce() + Send + 'static,
    {
        self.on_start(&label);
        self.wait_group.Add(1);
        let wrapper = Arc::clone(self);
        thread::spawn(move || {
            let _done = EnhancedDoneGuard { wrapper, label };
            exec();
        });
    }

    /// 带标签启动；panic 时可选 recover，并保证标签注销。
    pub fn RunWithRecover<F, R>(self: &Arc<Self>, exec: F, recover_fn: Option<R>, label: String)
    where
        F: FnOnce() + Send + 'static,
        R: FnOnce(PanicPayload) + Send + 'static,
    {
        self.on_start(&label);
        self.wait_group.Add(1);
        let wrapper = Arc::clone(self);
        thread::spawn(move || {
            let _done = EnhancedDoneGuard {
                wrapper: Arc::clone(&wrapper),
                label: label.clone(),
            };
            if let Err(payload) = catch_unwind(AssertUnwindSafe(exec)) {
                log::info!("WaitGroupEnhancedWrapper exec panic recovered; process={label}");
                if let Some(recover_fn) = recover_fn {
                    recover_fn(payload);
                }
            }
        });
    }

    /// 等待全部增强任务（含退出检查线程）结束。
    pub fn Wait(&self) {
        self.wait_group.Wait();
    }
}

/// 在给定线程池上执行任务，并用 WaitGroup 同步完成。
pub struct WaitGroupPool {
    wait_group: WaitGroup,
    pool: threadpool::ThreadPool,
}

/// 用已有 `ThreadPool` 构造池化 WaitGroup。
pub fn NewWaitGroupPool(pool: threadpool::ThreadPool) -> WaitGroupPool {
    WaitGroupPool {
        wait_group: WaitGroup::default(),
        pool,
    }
}

impl WaitGroupPool {
    /// 向线程池提交任务，计数 +1，结束时 Done。
    pub fn Run<F>(&self, exec: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.wait_group.Add(1);
        let wait_group = self.wait_group.clone();
        self.pool.execute(move || {
            let _done = DoneGuard(wait_group);
            exec();
        });
    }

    /// 等待池中本 wrapper 提交的全部任务完成。
    pub fn Wait(&self) {
        self.wait_group.Wait();
    }
}

/// 错误组：收集多个返回 `Result` 的线程，首错可取消关联 token。
pub struct ErrorGroupWithRecover {
    handles: Mutex<Vec<JoinHandle<()>>>,
    results_tx: Sender<Result<()>>,
    results_rx: Receiver<Result<()>>,
    cancellation: Option<CancellationToken>,
}

impl Default for ErrorGroupWithRecover {
    fn default() -> Self {
        let (results_tx, results_rx) = unbounded();
        Self {
            handles: Mutex::new(Vec::new()),
            results_tx,
            results_rx,
            cancellation: None,
        }
    }
}

/// 构造无取消上下文的错误组。
pub fn NewErrorGroupWithRecover() -> ErrorGroupWithRecover {
    ErrorGroupWithRecover::default()
}

/// 基于父 `CancellationToken` 创建子 token，错误时取消子树。
pub fn NewErrorGroupWithRecoverWithCtx(
    context: CancellationToken,
) -> (ErrorGroupWithRecover, CancellationToken) {
    let child = context.child_token();
    let (results_tx, results_rx) = unbounded();
    (
        ErrorGroupWithRecover {
            handles: Mutex::new(Vec::new()),
            results_tx,
            results_rx,
            cancellation: Some(child.clone()),
        },
        child,
    )
}

impl ErrorGroupWithRecover {
    /// 启动线程执行可能失败的闭包；失败时取消关联 token。
    pub fn Go<F>(&self, function: F)
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        let cancellation = self.cancellation.clone();
        let results = self.results_tx.clone();
        let handle = thread::spawn(move || {
            // panic 转为 anyhow 错误；普通 Err 同样触发 cancel。
            let result = catch_unwind(AssertUnwindSafe(function))
                .map_err(|payload| anyhow!(panic_message(&payload)))
                .and_then(|result| result);
            if result.is_err() {
                if let Some(cancellation) = cancellation {
                    cancellation.cancel();
                }
            }
            results
                .send(result)
                .expect("error group result receiver dropped unexpectedly");
        });
        self.handles
            .lock()
            .expect("error group mutex poisoned")
            .push(handle);
    }

    /// 等待全部句柄；按完成时序返回第一个错误（其余仍会 join 完）。
    pub fn Wait(&self) -> Result<()> {
        let handles =
            std::mem::take(&mut *self.handles.lock().expect("error group mutex poisoned"));
        let mut first_error = None;
        for _ in 0..handles.len() {
            let result = self
                .results_rx
                .recv()
                .expect("error group worker exited without reporting a result");
            if first_error.is_none() {
                first_error = result.err();
            }
        }
        for handle in handles {
            handle
                .join()
                .map_err(|payload| anyhow!(panic_message(&payload)))?;
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// 从 panic payload 提取可读消息（`&str`/`String`，否则占位文案）。
fn panic_message(payload: &PanicPayload) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic with non-string payload".to_owned())
}
