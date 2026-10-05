// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// 单任务执行器基类 `BaseTaskExecutor`。
//
// 负责在执行节点上刷新任务状态、按需交换 CPU slot、拉取 Pending/Running 子任务，
// 并委托 `StepExecutor` 实际执行；同时支持 Meta/资源动态变更与进度检查点上报。

use crate::*;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use std::time::{Duration, Instant};
/// 子任务轮询基础间隔（纳秒），默认 300ms。
static subtaskCheckIntervalNanos: AtomicU64 = AtomicU64::new(300_000_000);
/// 子任务轮询最大间隔（纳秒），默认 2s。
static maxSubtaskCheckIntervalNanos: AtomicU64 = AtomicU64::new(2_000_000_000);
/// SubtaskCheckInterval is the interval to check whether there are subtasks
/// to run. Mirrors Go's mutable package-level `var`; kept as an atomically
/// stored duration (instead of a `const`) so tests can shrink it the same
/// way Go's `ReduceCheckInterval` reassigns the variable.
/// 读取子任务检查间隔。
pub fn SubtaskCheckInterval() -> Duration {
    Duration::from_nanos(subtaskCheckIntervalNanos.load(Ordering::Acquire))
}
/// MaxSubtaskCheckInterval is the max interval to check whether there are
/// subtasks to run.
/// 读取子任务检查间隔上限。
pub fn MaxSubtaskCheckInterval() -> Duration {
    Duration::from_nanos(maxSubtaskCheckIntervalNanos.load(Ordering::Acquire))
}
/// Mirrors Go directly reassigning the public package-level
/// `SubtaskCheckInterval`/`MaxSubtaskCheckInterval` vars from a test in
/// another package; kept as a regular (not test-gated) function so
/// downstream crates' tests (e.g. `astersql-dxf-example`) can shrink it the
/// same way.
/// 测试用：同时设置基础/最大间隔并返回旧值。
pub fn SetSubtaskCheckIntervalForTest(
    interval: Duration,
    max_interval: Duration,
) -> (Duration, Duration) {
    let old = (SubtaskCheckInterval(), MaxSubtaskCheckInterval());
    subtaskCheckIntervalNanos.store(interval.as_nanos() as u64, Ordering::Release);
    maxSubtaskCheckIntervalNanos.store(max_interval.as_nanos() as u64, Ordering::Release);
    old
}
/// 检测任务参数（Meta/Slots）变更的轮询间隔。
pub const DetectParamModifyInterval: Duration = Duration::from_secs(5);
/// 连续无子任务时最多空转检查次数，之后退出 Run 循环。
pub const maxChecksWhenNoSubtask: i32 = 7;
/// Expected cancellation, including when StepExecutor propagates the context cause.
/// 取消子任务时使用的哨兵错误。
pub fn ErrCancelSubtask() -> ExecutorError {
    ExecutorError("cancel subtasks".into())
}
/// Running 且非幂等子任务不可安全重跑时的错误。
pub fn ErrNonIdempotentSubtask() -> ExecutorError {
    ExecutorError("subtask in running state and is not idempotent".into())
}
/// 任务 keyspace runtime 的最小校验接口。
pub trait TaskRuntime: Send + Sync {
    /// Expose concrete runtime capabilities to the task-type factory.
    fn AsAny(&self) -> Option<&dyn std::any::Any> {
        None
    }
    /// Release the holder after executor Close, including failed startup.
    fn Release(&self) {}

    /// 校验 runtime 是否仍绑定任务目标 keyspace。
    fn CheckTaskKeyspace(&self, keyspace: &str) -> Result<()>;
}
#[derive(Clone)]
/// 构造/运行 `BaseTaskExecutor` 所需的依赖注入参数。
pub struct Param {
    /// 任务表。
    pub taskTable: Arc<dyn TaskTable>,
    /// 节点 slot 管理器。
    pub slotMgr: Arc<slotManager>,
    /// 节点资源。
    pub nodeRc: NodeResource,
    /// 本执行节点 ID。
    pub execID: String,
    /// 任务类型扩展点。
    pub Extension: Arc<dyn Extension>,
    /// 任务 keyspace runtime；Executor 只借用，不负责释放。
    pub TaskRuntime: Option<Arc<dyn TaskRuntime>>,
}
/// 测试用便捷构造 `Param`。
pub fn NewParamForTest(
    table: Arc<dyn TaskTable>,
    slots: Arc<slotManager>,
    node: NodeResource,
    id: impl Into<String>,
    extension: Arc<dyn Extension>,
) -> Param {
    Param {
        taskTable: table,
        slotMgr: slots,
        nodeRc: node,
        execID: id.into(),
        Extension: extension,
        TaskRuntime: None,
    }
}
/// 任务执行器基类：刷新任务、拉子任务、驱动 StepExecutor。
pub struct BaseTaskExecutor {
    /// 依赖参数。
    pub Param: Param,
    /// 当前任务快照（可被 Meta/Slots 变更更新）。
    task: RwLock<Task>,
    /// 执行器取消上下文。
    ctx: Context,
    /// 当前步骤执行器（懒创建）。
    stepExec: Mutex<Option<Arc<dyn StepExecutor>>>,
    /// 当前步骤执行器对应的步骤。
    stepExecStep: Mutex<Option<Step>>,
    /// 当前步骤上下文；CancelRunningSubtask 通过它取消正在运行的步骤。
    stepCtx: Mutex<Option<Context>>,
    /// 用于把当前执行器安全地交给子任务监控线程。
    selfRef: OnceLock<Weak<BaseTaskExecutor>>,
    /// Per-executor log sink, shared with the node logging configuration.
    pub sampleLogger: RwLock<astersql_lightning_log::log::Logger>,
    /// 当前正在执行的子任务 ID。
    currSubtaskID: AtomicI64,
}
/// 创建基类执行器。
pub fn NewBaseTaskExecutor(ctx: Context, task: Task, param: Param) -> Arc<BaseTaskExecutor> {
    let executor = Arc::new(BaseTaskExecutor {
        Param: param,
        task: RwLock::new(task),
        ctx,
        stepExec: Mutex::new(None),
        stepExecStep: Mutex::new(None),
        stepCtx: Mutex::new(None),
        selfRef: OnceLock::new(),
        currSubtaskID: AtomicI64::new(0),
        sampleLogger: RwLock::new(astersql_lightning_log::log::L()),
    });
    let _ = executor.selfRef.set(Arc::downgrade(&executor));
    executor
}

/// 以短片段休眠，保证父上下文取消后不会被整段退避阻塞。
fn wait_or_cancel(ctx: &Context, duration: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < duration {
        if ctx.Done() {
            return false;
        }
        std::thread::sleep((duration - start.elapsed()).min(Duration::from_millis(10)));
    }
    true
}
impl BaseTaskExecutor {
    /// 负载均衡检查：取消已调度走的当前子任务，并处理本节点额外的 Running 子任务。
    pub fn checkBalanceSubtask(&self, ctx: &Context, cancel: &Context) {
        if ctx.Done() {
            return;
        }
        let task = self.GetTaskBase();
        let Ok(subtasks) = self.Param.taskTable.GetSubtasksByExecIDAndStepAndStates(
            ctx,
            &self.Param.execID,
            task.ID,
            task.Step,
            &[SubtaskState::Running],
        ) else {
            return;
        };
        if subtasks.is_empty() {
            cancel.Cancel();
            return;
        }

        let current_id = self.currSubtaskID.load(Ordering::Acquire);
        let mut extra = Vec::new();
        for subtask in subtasks {
            if subtask.SubtaskBase.ID == current_id {
                continue;
            }
            if !self.Param.Extension.IsIdempotent(&subtask) {
                let error = ErrNonIdempotentSubtask();
                if self
                    .updateSubtaskStateAndErrorImpl(
                        ctx,
                        &subtask.SubtaskBase.ExecID,
                        subtask.SubtaskBase.ID,
                        SubtaskState::Failed,
                        Some(&error),
                    )
                    .is_ok()
                {
                    return;
                }
                continue;
            }
            extra.push(subtask.SubtaskBase);
        }
        if !extra.is_empty() {
            let _ = self
                .Param
                .taskTable
                .RunningSubtasksBack2Pending(ctx, &extra);
        }
    }
    /// 周期把 StepExecutor 实时进度写入检查点。
    pub fn updateSubtaskSummaryLoop(&self, ctx: &Context, subtask_id: i64) {
        while !ctx.Done() {
            if !self.ctx.Done() {
                std::thread::sleep(Duration::from_millis(100));
            } else {
                break;
            }
            let Some(summary) = self
                .stepExec
                .lock()
                .expect("step lock poisoned")
                .as_ref()
                .and_then(|s| s.RealtimeSummary())
            else {
                continue;
            };
            let _ =
                self.Param
                    .taskTable
                    .UpdateSubtaskCheckpoint(ctx, subtask_id, &summary.RowCount);
        }
    }
    /// 初始化并校验任务 keyspace runtime。
    pub fn Init(&self, _: &Context) -> Result<()> {
        let keyspace = self.GetTaskBase().Keyspace;
        if let Some(runtime) = &self.Param.TaskRuntime {
            runtime.CheckTaskKeyspace(&keyspace)
        } else if keyspace.is_empty() {
            Ok(())
        } else {
            Err(ExecutorError("task runtime is unavailable".into()))
        }
    }
    /// 克隆执行器上下文。
    pub fn Ctx(&self) -> Context {
        self.ctx.clone()
    }
    /// 主循环：刷新任务、交换 slot、拉取并执行子任务。
    pub fn Run(&self) {
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| self.runLoop())) {
            let message = payload
                .downcast_ref::<&str>()
                .map(|message| (*message).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "task executor panicked".to_owned());
            let task_id = self.GetTaskBase().ID;
            self.failOneSubtask(&self.ctx, task_id, &ExecutorError(message));
        }
        self.cleanStepExecutor();
    }

    /// `Run` 的可恢复主体；panic 边界和统一清理由外层负责。
    fn runLoop(&self) {
        let mut empty = 0;
        while !self.ctx.Done() {
            let old = self.task.read().expect("task lock poisoned").clone();
            let new_task = match self.Param.taskTable.GetTaskByID(&self.ctx, old.TaskBase.ID) {
                Ok(task) => task,
                Err(error) if error.0 == "task not found" => break,
                Err(_) => continue,
            };
            if old.TaskBase.Step == new_task.TaskBase.Step
                && old.Meta != new_task.Meta
                && let Some(step) = self.stepExec.lock().expect("step lock poisoned").as_ref()
                && step
                    .TaskMetaModified(&self.currentStepContext(), &new_task.Meta)
                    .is_err()
            {
                self.cleanStepExecutor();
                continue;
            }
            // RequiredSlots 变更时先尝试 exchange；失败则退出。
            if new_task.TaskBase.RequiredSlots != old.TaskBase.RequiredSlots
                && !self.Param.slotMgr.exchange(&new_task.TaskBase)
            {
                break;
            }
            *self.task.write().expect("task lock poisoned") = new_task.clone();
            if !matches!(
                new_task.TaskBase.State,
                TaskState::Running | TaskState::Modifying
            ) {
                break;
            }
            let Ok(subtask) = self.Param.taskTable.GetFirstSubtaskInStates(
                &self.ctx,
                &self.Param.execID,
                new_task.TaskBase.ID,
                new_task.TaskBase.Step,
                &[SubtaskState::Pending, SubtaskState::Running],
            ) else {
                continue;
            };
            // 暂无子任务：指数退避休眠，超过阈值则退出。
            let Some(mut subtask) = subtask else {
                if empty >= maxChecksWhenNoSubtask {
                    break;
                }
                let delay = (SubtaskCheckInterval() * 2u32.pow(empty.min(3) as u32))
                    .min(MaxSubtaskCheckInterval());
                empty += 1;
                if !wait_or_cancel(&self.ctx, delay) {
                    break;
                }
                continue;
            };
            empty = 0;
            if self
                .stepExecStep
                .lock()
                .expect("step lock poisoned")
                .is_some_and(|step| step != new_task.TaskBase.Step)
            {
                self.cleanStepExecutor();
            }
            if self.createStepExecutor().is_err() {
                continue;
            }
            if self.currentStepContext().Done() {
                continue;
            }
            if let Err(error) = self.runSubtask(&mut subtask) {
                let logger = self.sampleLogger.read().expect("logger lock poisoned");
                // Context cancellation and the explicit cancel cause are expected
                // during shutdown/reverting; keep actual failures at error level.
                if error == ErrCancelSubtask() || error.0 == "context canceled" {
                    logger.Info(
                        "subtask run canceled",
                        [astersql_lightning_log::log::ShortError(Some(&error))],
                    );
                } else {
                    logger.Error(
                        "run subtask failed",
                        [astersql_lightning_log::filter::Field::string(
                            "error", error.0,
                        )],
                    );
                }
            }
        }
    }
    /// 懒创建并 Init 当前步骤的 StepExecutor。
    pub fn createStepExecutor(&self) -> Result<()> {
        let task = self.task.read().expect("task lock poisoned").clone();
        if self
            .stepExecStep
            .lock()
            .expect("step lock poisoned")
            .is_some_and(|step| step == task.TaskBase.Step)
        {
            return Ok(());
        }
        if self.stepExec.lock().expect("step lock poisoned").is_some() {
            self.cleanStepExecutor();
        }
        let executor = match self.Param.Extension.GetStepExecutor(&task) {
            Ok(executor) => executor,
            Err(error) => {
                self.failOneSubtask(&self.ctx, task.TaskBase.ID, &error);
                return Err(error);
            }
        };
        if let Err(error) = executor.Init(&self.ctx) {
            if !self.IsRetryableError(&error) {
                self.failOneSubtask(&self.ctx, task.TaskBase.ID, &error);
            }
            return Err(error);
        }
        *self.stepExec.lock().expect("step lock poisoned") = Some(executor);
        *self.stepExecStep.lock().expect("step lock poisoned") = Some(task.TaskBase.Step);
        *self.stepCtx.lock().expect("step lock poisoned") = Some(Context::Child(&self.ctx));
        Ok(())
    }
    /// Cleanup 并丢弃当前 StepExecutor。
    pub fn cleanStepExecutor(&self) {
        if let Some(ctx) = self.stepCtx.lock().expect("step lock poisoned").take() {
            ctx.Cancel();
        }
        self.stepExecStep.lock().expect("step lock poisoned").take();
        if let Some(executor) = self.stepExec.lock().expect("step lock poisoned").take() {
            let _ = executor.Cleanup(&self.ctx);
        }
    }
    /// 返回当前 step 上下文；未初始化时创建一个继承任务上下文的临时子上下文。
    pub fn currentStepContext(&self) -> Context {
        self.stepCtx
            .lock()
            .expect("step lock poisoned")
            .as_ref()
            .cloned()
            .unwrap_or_else(|| Context::Child(&self.ctx))
    }
    /// 执行单个子任务：幂等校验、Start、RunSubtask、Finish/失败处理。
    pub fn runSubtask(&self, subtask: &mut Subtask) -> Result<()> {
        let step_ctx = self.currentStepContext();
        // Running 且非幂等：不可安全重试，直接 Failed。
        if subtask.SubtaskBase.State == SubtaskState::Running
            && !self.Param.Extension.IsIdempotent(subtask)
        {
            let error = ErrNonIdempotentSubtask();
            self.updateSubtaskStateAndErrorImpl(
                &self.ctx,
                &subtask.SubtaskBase.ExecID,
                subtask.SubtaskBase.ID,
                SubtaskState::Failed,
                Some(&error),
            )?;
            return Err(error);
        }
        if subtask.SubtaskBase.State == SubtaskState::Pending {
            self.startSubtask(&step_ctx, subtask.SubtaskBase.ID)?
        }
        self.currSubtaskID
            .store(subtask.SubtaskBase.ID, Ordering::Release);
        let subtask_ctx = Context::Child(&step_ctx);
        let balance_ctx = Context::Child(&subtask_ctx);
        let subtask_cancel = subtask_ctx.clone();
        let mut monitors = Vec::new();
        if let Some(executor) = self.selfRef.get().and_then(Weak::upgrade) {
            monitors.push(std::thread::spawn({
                let monitor_ctx = balance_ctx.clone();
                let cancel = subtask_cancel.clone();
                move || {
                    while wait_or_cancel(&monitor_ctx, Duration::from_secs(2)) {
                        executor.checkBalanceSubtask(&monitor_ctx, &cancel);
                        if cancel.Done() {
                            break;
                        }
                    }
                }
            }));
        }
        let param_ctx = Context::Child(&subtask_ctx);
        if let Some(executor) = self.selfRef.get().and_then(Weak::upgrade) {
            let monitor_ctx = param_ctx.clone();
            monitors.push(std::thread::spawn(move || {
                executor.detectAndHandleParamModifyLoop(&monitor_ctx)
            }));
        }
        let has_summary = self
            .stepExec
            .lock()
            .expect("step lock poisoned")
            .as_ref()
            .is_some_and(|step| self.hasRealtimeSummary(step.as_ref()));
        if has_summary {
            if let Some(step) = self.stepExec.lock().expect("step lock poisoned").as_ref() {
                step.ResetSummary();
            }
            if let Some(executor) = self.selfRef.get().and_then(Weak::upgrade) {
                let summary_ctx = Context::Child(&subtask_ctx);
                let subtask_id = subtask.SubtaskBase.ID;
                monitors.push(std::thread::spawn(move || {
                    executor.updateSubtaskSummaryLoop(&summary_ctx, subtask_id)
                }));
            }
        }
        let step_executor = self
            .stepExec
            .lock()
            .expect("step lock poisoned")
            .as_ref()
            .cloned()
            .ok_or_else(|| ExecutorError("step executor is not initialized".into()))?;
        let result = step_executor.RunSubtask(&subtask_ctx, subtask);
        match result {
            Ok(()) => {
                subtask_ctx.Cancel();
                for monitor in monitors {
                    let _ = monitor.join();
                }
                self.finishSubtask(&step_ctx, subtask)
            }
            Err(error) => {
                let state_result = self.markSubTaskCanceledOrFailed(&subtask_ctx, subtask, &error);
                subtask_ctx.Cancel();
                for monitor in monitors {
                    let _ = monitor.join();
                }
                state_result?;
                Err(error)
            }
        }
    }
    /// 是否支持实时进度摘要。
    pub fn hasRealtimeSummary(&self, step: &dyn StepExecutor) -> bool {
        step.RealtimeSummary().is_some()
    }
    /// 周期检测任务参数变更。
    pub fn detectAndHandleParamModifyLoop(&self, ctx: &Context) {
        while ctx.Done() == false {
            if !wait_or_cancel(ctx, DetectParamModifyInterval) {
                break;
            }
            let _ = self.detectAndHandleParamModify(ctx);
        }
    }
    /// 处理 RequiredSlots / Meta 变更。
    pub fn detectAndHandleParamModify(&self, ctx: &Context) -> Result<()> {
        let old = self.task.read().expect("task lock poisoned").clone();
        let latest = self.Param.taskTable.GetTaskByID(ctx, old.TaskBase.ID)?;
        self.tryModifyTaskRequiredSlots(ctx, &old, &latest);
        if old.Meta != latest.Meta {
            self.stepExec
                .lock()
                .expect("step lock poisoned")
                .as_ref()
                .ok_or_else(|| ExecutorError("step executor is not initialized".into()))?
                .TaskMetaModified(ctx, &latest.Meta)?;
            self.metaModifyApplied(latest.Meta)
        }
        Ok(())
    }
    /// 调整运行时 slot：缩容先通知 Step 再 exchange；扩容先 exchange 再通知，失败则回滚。
    pub fn tryModifyTaskRequiredSlots(&self, ctx: &Context, old: &Task, latest: &Task) {
        if old.TaskBase.RequiredSlots == latest.TaskBase.RequiredSlots {
            return;
        }
        // 缩容路径。
        if latest.TaskBase.RequiredSlots < old.TaskBase.RequiredSlots {
            if let Some(step) = self.stepExec.lock().expect("step lock poisoned").as_ref() {
                if step
                    .ResourceModified(ctx, &self.Param.nodeRc.GetStepResource(&latest.TaskBase))
                    .is_err()
                {
                    return;
                }
            }
            if self.Param.slotMgr.exchange(&latest.TaskBase) {
                self.requiredSlotsModifyApplied(latest.TaskBase.RequiredSlots)
            }
        } else if self.Param.slotMgr.exchange(&latest.TaskBase) {
            let result = self
                .stepExec
                .lock()
                .expect("step lock poisoned")
                .as_ref()
                .map(|s| {
                    s.ResourceModified(ctx, &self.Param.nodeRc.GetStepResource(&latest.TaskBase))
                });
            if result.is_some_and(|r| r.is_err()) {
                let _ = self.Param.slotMgr.exchange(&old.TaskBase);
            } else {
                self.requiredSlotsModifyApplied(latest.TaskBase.RequiredSlots)
            }
        }
    }
    /// 将新的 RequiredSlots 写回本地任务快照。
    pub fn requiredSlotsModifyApplied(&self, new_slots: i32) {
        self.task
            .write()
            .expect("task lock poisoned")
            .TaskBase
            .RequiredSlots = new_slots
    }
    /// 将新 Meta 写回本地任务快照。
    pub fn metaModifyApplied(&self, meta: Vec<u8>) {
        self.task.write().expect("task lock poisoned").Meta = meta
    }
    /// 返回当前任务基础信息副本。
    pub fn GetTaskBase(&self) -> TaskBase {
        self.task
            .read()
            .expect("task lock poisoned")
            .TaskBase
            .clone()
    }
    /// Returns the executor node identifier (normally `IP:port`).
    pub fn GetExecID(&self) -> &str {
        &self.Param.execID
    }
    /// 标记取消当前子任务步骤。
    pub fn CancelRunningSubtask(&self) {
        self.cancelRunStepWith(ErrCancelSubtask())
    }
    /// 取消整个执行器上下文。
    pub fn Cancel(&self) {
        self.ctx.Cancel()
    }
    /// 取消并清理 StepExecutor。
    pub fn Close(&self) {
        self.Cancel();
        self.cleanStepExecutor()
    }
    /// 以指定原因取消当前 step 上下文。
    pub fn cancelRunStepWith(&self, cause: ExecutorError) {
        if let Some(ctx) = self.stepCtx.lock().expect("step lock poisoned").as_ref() {
            ctx.CancelWithCause(cause);
        }
    }
    /// 带重试地更新子任务状态/错误。
    pub fn updateSubtaskStateAndErrorImpl(
        &self,
        ctx: &Context,
        exec_id: &str,
        id: i64,
        state: SubtaskState,
        error: Option<&ExecutorError>,
    ) -> Result<()> {
        self.retry(|| {
            self.Param
                .taskTable
                .UpdateSubtaskStateAndError(ctx, exec_id, id, state, error)
        })
    }
    /// 将 Pending 子任务标记为开始。
    pub fn startSubtask(&self, ctx: &Context, id: i64) -> Result<()> {
        self.retry(|| {
            self.Param
                .taskTable
                .StartSubtask(ctx, id, &self.Param.execID)
        })
    }
    /// 完成子任务并持久化 Meta。
    pub fn finishSubtask(&self, ctx: &Context, subtask: &Subtask) -> Result<()> {
        let summary = self
            .stepExec
            .lock()
            .expect("step lock poisoned")
            .as_ref()
            .and_then(|step| step.RealtimeSummaryJSON());
        if let Some(summary) = summary {
            self.retry(|| {
                self.Param
                    .taskTable
                    .UpdateSubtaskSummaryJSON(ctx, subtask.SubtaskBase.ID, &summary)
            })?;
        }
        self.retry(|| {
            self.Param.taskTable.FinishSubtask(
                ctx,
                &subtask.SubtaskBase.ExecID,
                subtask.SubtaskBase.ID,
                &subtask.Meta,
            )
        })
    }
    /// 按取消标志/可重试性将子任务标为 Canceled、忽略或 Failed。
    pub fn markSubTaskCanceledOrFailed(
        &self,
        ctx: &Context,
        subtask: &Subtask,
        error: &ExecutorError,
    ) -> Result<()> {
        if ctx.Cause().is_some_and(|cause| cause == ErrCancelSubtask()) {
            self.updateSubtaskStateAndErrorImpl(
                &self.ctx,
                &subtask.SubtaskBase.ExecID,
                subtask.SubtaskBase.ID,
                SubtaskState::Canceled,
                None,
            )
        } else if ctx.Done() {
            // Manager/task cancellation is graceful shutdown. The scheduler
            // will decide how to handle a subtask left in Running state.
            Ok(())
        } else if self.IsRetryableError(error) {
            Ok(())
        } else {
            self.updateSubtaskStateAndErrorImpl(
                ctx,
                &subtask.SubtaskBase.ExecID,
                subtask.SubtaskBase.ID,
                SubtaskState::Failed,
                Some(error),
            )
        }
    }
    /// 将任务下某一子任务标记失败。
    pub fn failOneSubtask(&self, ctx: &Context, task_id: i64, error: &ExecutorError) {
        let _ = self.retry(|| {
            self.Param
                .taskTable
                .FailSubtask(ctx, &self.Param.execID, task_id, error)
        });
    }
    /// 返回任务表句柄。
    pub fn GetTaskTable(&self) -> Arc<dyn TaskTable> {
        self.Param.taskTable.clone()
    }
    /// 委托 Extension 判断是否可重试。
    pub fn IsRetryableError(&self, error: &ExecutorError) -> bool {
        self.Param.Extension.IsRetryableError(error)
    }
    /// 最多 3 次指数退避重试。
    fn retry(&self, mut action: impl FnMut() -> Result<()>) -> Result<()> {
        let mut last = Ok(());
        for attempt in 0..3 {
            last = action();
            if last.is_ok() {
                return last;
            }
            if self.ctx.Done() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10 * 2u64.pow(attempt)));
        }
        last
    }
}
#[cfg(test)]
impl BaseTaskExecutor {
    /// Injects a step executor directly, mirroring Go's white-box
    /// `taskExecutor.stepExec = env.stepExecutor` test assignments.
    /// 测试用：直接注入 StepExecutor。
    pub fn SetStepExecutorForTest(&self, executor: Arc<dyn StepExecutor>) {
        *self.stepExec.lock().expect("step lock poisoned") = Some(executor);
    }
}
/// 将基类方法适配到 `TaskExecutor` trait。
impl TaskExecutor for BaseTaskExecutor {
    fn Init(&self, c: &Context) -> Result<()> {
        BaseTaskExecutor::Init(self, c)
    }
    fn Run(&self) {
        BaseTaskExecutor::Run(self)
    }
    fn GetTaskBase(&self) -> TaskBase {
        BaseTaskExecutor::GetTaskBase(self)
    }
    fn CancelRunningSubtask(&self) {
        BaseTaskExecutor::CancelRunningSubtask(self)
    }
    fn Cancel(&self) {
        BaseTaskExecutor::Cancel(self)
    }
    fn Close(&self) {
        BaseTaskExecutor::Close(self)
    }
    fn IsRetryableError(&self, e: &ExecutorError) -> bool {
        BaseTaskExecutor::IsRetryableError(self, e)
    }
}
