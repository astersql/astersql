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

//! CRR（跨区域复制）检查点计算服务：驱动内层 Calculator 循环，并挂接状态与 resume 持久化。
//! 对应 Go `br/pkg/stream/crr/service/service.go`。
//! 外层循环负责：加载/保存 resume、推进 checkpoint、无进展时 watch PD、失败重试与关机 flush。
//! Calculator 内部已通过 Observer 上报的失败不再由服务层重复计数（见 `RunOnceError` 区分）。
//! HTTP 健康检查与 `/status` 由同包 `http` 模块注册；本文件只维护可查询的 `StatusStore`。
//! `Deps::State` 可选：未配置时跳过 resume 持久化，仍可完成纯计算与 watch 路径。

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use astersql_br_pkg_stream_crr_internal_checkpoint::{
    Calculator, CalculatorDeps, CheckpointCalculatorConfig, CheckpointEvent, Context, Error,
    EventType, FileExistenceChecker, NewCalculator,
    NewExistenceSyncChecker as NewExistenceSyncCheckerInner, ObjectSyncChecker, PDMetaReader,
    PersistentState, UpstreamStorageReader,
};

use crate::status::{StatusObserver, StatusSnapshot, StatusStore, new_status_store};

/// 外层循环默认重试间隔；`RetryInterval<=0` 时回落到此值。
pub const DefaultRetryInterval: Duration = Duration::from_secs(1);

/// 控制外层 worker 循环与内嵌 checkpoint calculator 行为。
#[derive(Clone, Debug)]
pub struct Config {
    /// 透传给内层 `NewCalculator` 的任务名、扫描窗口等参数。
    pub CalculatorConfig: CheckpointCalculatorConfig,
    /// `run_once` 失败后的休眠间隔；亦用作关机 flush 的超时上界。
    pub RetryInterval: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            CalculatorConfig: CheckpointCalculatorConfig::default(),
            RetryInterval: DefaultRetryInterval,
        }
    }
}

/// 等待上游全局 checkpoint 相对 `current` 前进；无进展时阻塞直到 PD 通知或 ctx 取消。
pub trait UpstreamCheckpointWaiter: Send + Sync {
    fn WaitGlobalCheckpointAdvance(
        &self,
        ctx: &Context,
        taskName: &str,
        current: u64,
    ) -> Result<(), Error>;
}

/// 持久化/恢复 calculator 进度，供进程重启后续跑。
pub trait ResumeStateStore: Send + Sync {
    fn LoadState(&self, ctx: &Context) -> Result<Option<PersistentState>, Error>;
    fn SaveState(&self, ctx: &Context, state: PersistentState) -> Result<(), Error>;
}

pub type ObjectSyncCheckerAlias = dyn ObjectSyncChecker;

/// 把裸文件存在性检查适配为 `ObjectSyncChecker`，与 Go `NewExistenceSyncChecker` 对齐。
pub fn NewExistenceSyncChecker<C: FileExistenceChecker + 'static>(
    checker: C,
) -> Box<dyn ObjectSyncChecker> {
    Box::new(NewExistenceSyncCheckerInner(checker))
}

/// 构造服务所需外部依赖：PD 元数据、checkpoint watcher、上游存储、同步检查与可选 resume 存储。
pub struct Deps {
    /// 读 store 列表与相关元信息，供 calculator 规划轮次。
    pub PD: Box<dyn PDMetaReader>,
    /// 与 PD 分离的 watch 接口，便于测试注入失败/延迟。
    pub Watcher: Box<dyn UpstreamCheckpointWaiter>,
    /// 读取上游 meta/log 对象。
    pub Upstream: Box<dyn UpstreamStorageReader>,
    /// 判断下游对象是否已安全同步。
    pub Sync: Box<dyn ObjectSyncChecker>,
    /// 可选 resume 存储；`None` 时不读写 PersistentState。
    pub State: Option<Box<dyn ResumeStateStore>>,
}

/// 内层 calculator 配置别名，保持与 Go `CalculatorConfig` 类型别名一致。
pub type CalculatorConfig = CheckpointCalculatorConfig;

/// 包裹 CRR checkpoint calculator，附带状态跟踪；HTTP 暴露由同包 `http` 模块挂接。
pub struct Service {
    /// 内层计算器；`Mutex` 保护因 `Run` 与状态查询可能并发。
    pub(crate) calc: Mutex<Calculator>,
    /// 对外快照源；`Status()` / HTTP 读取此仓。
    pub(crate) status: StatusStore,
    /// 同时实现 checkpoint `Observer`，把事件灌入 `status`。
    pub(crate) observer: StatusObserver,
    /// 无进展时阻塞等待的上游 checkpoint watcher。
    pub(crate) pd: Box<dyn UpstreamCheckpointWaiter>,
    pub(crate) state: Option<Box<dyn ResumeStateStore>>,
    pub(crate) cfg: Config,
    /// 首次成功加载（或确认无状态）后置 true，避免每轮重复 Load。
    pub(crate) resume_state_initialized: Mutex<bool>,
    /// 已推进但尚未成功 Save 的快照；关机时尽量 flush。
    pub(crate) pending_resume_state: Mutex<Option<PersistentState>>,
}

/// 创建服务并挂接 calculator observer；非法重试间隔会被纠正为默认值。
pub fn New(deps: Deps, mut cfg: Config) -> Result<Service, Error> {
    // 非正间隔视为未配置，回落默认 1s，避免忙等。
    if cfg.RetryInterval <= Duration::ZERO {
        cfg.RetryInterval = DefaultRetryInterval;
    }

    let (status, observer) = new_status_store(cfg.CalculatorConfig.TaskName.clone());
    let calc = NewCalculator(
        CalculatorDeps {
            PD: deps.PD,
            Upstream: deps.Upstream,
            Sync: deps.Sync,
        },
        cfg.CalculatorConfig.clone(),
        // observer 同时驱动 StatusStore，供 /status 与 metrics 读取。
        Some(Box::new(observer.clone())),
    )?;

    Ok(Service {
        calc: Mutex::new(calc),
        status,
        observer,
        // Watcher 从 Deps 拆出，与 PDMetaReader 角色分离。
        pd: deps.Watcher,
        state: deps.State,
        cfg,
        resume_state_initialized: Mutex::new(false),
        pending_resume_state: Mutex::new(None),
    })
}

impl Service {
    /// 启动检查点计算循环，直到 context 取消；返回时状态会经 `RunStopGuard` 置为 stopped。
    pub fn Run(&self, ctx: &Context) -> Result<(), Error> {
        self.status.start();
        // Drop 时 flush pending resume 并 stop status，对齐 Go defer。
        let stop_guard = RunStopGuard::new(self);
        loop {
            // 合作式取消：每轮开头检查，避免长时间卡在计算里仍忽略取消。
            if ctx.Err().is_some() {
                return Ok(());
            }
            if let Err(err) = self.run_once(ctx) {
                // false 表示应结束循环（取消或休眠被打断）。
                if !self.retry_after_run_error(ctx, err) {
                    return Ok(());
                }
            }
        }
    }

    /// 单轮：准备 resume → 计算下一 checkpoint → 入队/落盘 → 无进展则 watch。
    fn run_once(&self, ctx: &Context) -> Result<(), RunOnceError> {
        self.prepare_resume_state(ctx)?;

        self.observer.BeginCalculationRound();
        let last_checkpoint = self.calc.lock().expect("calculator lock").LastCheckpoint();
        let next_checkpoint = self
            .calc
            .lock()
            .expect("calculator lock")
            .ComputeNextCheckpoint(ctx)
            // calculator 失败已由 observer 计入，标记为 Observed 以免服务层再记一次。
            .map_err(RunOnceError::ObservedCalculator)?;

        self.queue_resume_state_save(last_checkpoint, next_checkpoint);
        self.flush_pending_resume_state(ctx)?;

        // 无推进时阻塞等待上游全局 checkpoint，避免空转烧 CPU。
        if next_checkpoint == last_checkpoint {
            return self
                .wait_checkpoint_advance(ctx, last_checkpoint)
                .map_err(RunOnceError::Other);
        }
        Ok(())
    }

    /// 委托 Watcher 阻塞至全局 checkpoint > current 或 ctx 结束。
    fn wait_checkpoint_advance(&self, ctx: &Context, current: u64) -> Result<(), Error> {
        self.pd
            .WaitGlobalCheckpointAdvance(ctx, &self.cfg.CalculatorConfig.TaskName, current)
    }

    /// 上报发生在 `ComputeNextCheckpoint` 之外的失败（watch/resume 等）。
    fn record_service_failure(&self, err: Error) {
        self.observer.OnCheckpointEvent(CheckpointEvent {
            Type: EventType::EventCalculationFailed,
            Time: Some(SystemTimeNow()),
            TaskName: self.cfg.CalculatorConfig.TaskName.clone(),
            Err: Some(err),
            ..Default::default()
        });
    }

    /// 决定是否休眠后重试；ctx 取消类错误直接结束循环。
    fn retry_after_run_error(&self, ctx: &Context, err: RunOnceError) -> bool {
        if should_stop(ctx, &err) {
            return false;
        }
        // ObservedCalculator 已在 calculator 路径上报，此处只记 Other。
        if let RunOnceError::Other(err) = err {
            self.record_service_failure(err);
        }
        sleep_context(ctx, self.cfg.RetryInterval).is_ok()
    }

    /// 返回当前服务状态的一致性快照。
    pub fn Status(&self) -> StatusSnapshot {
        self.status.snapshot_copy()
    }

    /// 首轮加载 resume；之后每轮优先 flush 尚未落盘的 pending。
    fn prepare_resume_state(&self, ctx: &Context) -> Result<(), RunOnceError> {
        let initialized = *self
            .resume_state_initialized
            .lock()
            .expect("resume init lock");
        if !initialized {
            return self.initialize_resume_state(ctx);
        }
        // 已初始化后每轮先尝试刷掉上一轮未落盘的 pending。
        self.flush_pending_resume_state(ctx)
    }

    /// 首次进入循环时从存储加载 PersistentState 并恢复 calculator / status。
    fn initialize_resume_state(&self, ctx: &Context) -> Result<(), RunOnceError> {
        if let Some(state_store) = &self.state {
            let state = state_store.LoadState(ctx).map_err(|err| {
                RunOnceError::Other(Error::new(format!("load resume state: {err}")))
            })?;
            if let Some(state) = state {
                self.calc
                    .lock()
                    .expect("calculator lock")
                    .RestorePersistentState(state.clone())
                    .map_err(RunOnceError::Other)?;
                self.status.set_persistent_state(state);
            }
        }
        self.status.clear_failure();
        *self
            .resume_state_initialized
            .lock()
            .expect("resume init lock") = true;
        Ok(())
    }

    /// 仅在 checkpoint 真正前进且配置了 State 时入队快照，避免无意义写盘。
    fn queue_resume_state_save(&self, last_checkpoint: u64, next_checkpoint: u64) {
        if self.state.is_none() || next_checkpoint <= last_checkpoint {
            return;
        }
        let state = self.calc.lock().expect("calculator lock").StateSnapshot();
        *self
            .pending_resume_state
            .lock()
            .expect("pending resume lock") = Some(state);
    }

    /// 将 pending 写入 ResumeStateStore；成功后同步到 status 并清空失败标记。
    fn flush_pending_resume_state(&self, ctx: &Context) -> Result<(), RunOnceError> {
        let pending = self
            .pending_resume_state
            .lock()
            .expect("pending resume lock")
            .clone();
        let Some(pending) = pending else {
            return Ok(());
        };
        let Some(state_store) = &self.state else {
            return Ok(());
        };
        state_store
            .SaveState(ctx, pending.clone())
            .map_err(|err| RunOnceError::Other(Error::new(format!("save resume state: {err}"))))?;
        self.status.set_persistent_state(pending);
        *self
            .pending_resume_state
            .lock()
            .expect("pending resume lock") = None;
        self.status.clear_failure();
        Ok(())
    }

    /// 关机路径：用独立短超时 context 尽力落盘，失败只打日志不阻断退出。
    fn flush_pending_resume_state_on_shutdown(&self) {
        let has_pending = self
            .pending_resume_state
            .lock()
            .expect("pending resume lock")
            .is_some();
        if !has_pending || self.state.is_none() {
            return;
        }
        let (persist_ctx, _cancel) =
            Context::WithTimeout(&Context::Background(), self.cfg.RetryInterval);
        if let Err(err) = self.flush_pending_resume_state(&persist_ctx) {
            eprintln!(
                "failed to flush pending CRR resume state during shutdown: task={} err={err}",
                self.cfg.CalculatorConfig.TaskName
            );
        }
    }
}

/// `Run` 退出时的清理守卫：先 flush pending resume，再把 status 标为 stopped。
struct RunStopGuard<'a> {
    service: &'a Service,
}

impl<'a> RunStopGuard<'a> {
    fn new(service: &'a Service) -> Self {
        Self { service }
    }
}

impl Drop for RunStopGuard<'_> {
    fn drop(&mut self) {
        self.service.flush_pending_resume_state_on_shutdown();
        self.service.status.stop();
    }
}

/// 区分 calculator 已观察错误与服务层其他错误，避免失败计数双计。
#[derive(Debug)]
enum RunOnceError {
    /// calculator 路径已通过 Observer 上报。
    ObservedCalculator(Error),
    /// watch/resume 等服务层错误，需由 `record_service_failure` 补报。
    Other(Error),
}

impl std::fmt::Display for RunOnceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ObservedCalculator(err) | Self::Other(err) => write!(f, "{err}"),
        }
    }
}

impl RunOnceError {
    /// 取出内层 Error 引用，供 `should_stop` 检查取消文案。
    fn as_error(&self) -> &Error {
        match self {
            Self::ObservedCalculator(err) | Self::Other(err) => err,
        }
    }
}

/// context 已取消且错误信息表明为取消/超时，则结束外层循环而非重试。
fn should_stop(ctx: &Context, err: &RunOnceError) -> bool {
    if ctx.Err().is_none() {
        return false;
    }
    let message = err.as_error().message();
    message.contains("context canceled") || message.contains("context deadline exceeded")
}

/// 可中断休眠：按小步长 sleep，期间检查 ctx，对齐 Go `sleepContext`。
fn sleep_context(ctx: &Context, delay: Duration) -> Result<(), Error> {
    let step = Duration::from_millis(5).min(delay);
    let mut slept = Duration::ZERO;
    while slept < delay {
        if ctx.Err().is_some() {
            return Err(ctx.Err().expect("context error"));
        }
        let remaining = delay - slept;
        thread::sleep(step.min(remaining));
        slept += step.min(remaining);
    }
    Ok(())
}

/// 事件时间戳来源；单测可通过替换调用点注入固定时间（当前直接取系统时钟）。
fn SystemTimeNow() -> std::time::SystemTime {
    std::time::SystemTime::now()
}
