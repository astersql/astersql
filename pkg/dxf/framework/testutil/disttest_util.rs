// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 分布式任务测试用的执行器扩展与提交流程辅助。
//
// 提供可注入的 `StepExecutor` / `TaskExecutorExtension` / 清理例程，
// 以及向 DXF 运行时提交任务并等待完成（或暂停）的便捷函数。

use std::sync::Arc;

use crate::context::{DxfError, Step, Subtask, Task, TaskBase, TaskState, TestContext};
use crate::scheduler_util::SchedulerExtension;
use crate::task_util::getTaskKS;

/// Go `proto.TaskTypeExample` 的公开字符串值。
pub const TASK_TYPE_EXAMPLE: &str = "Example";

/// 子任务执行回调类型。
pub type RunSubtaskFn = dyn Fn(&Subtask) -> Result<(), DxfError> + Send + Sync;
/// 按任务构造步骤执行器的工厂回调。
pub type GetStepExecutorFn = dyn Fn(&Task) -> Result<StepExecutor, DxfError> + Send + Sync;
/// 判断错误是否可重试的回调。
pub type RetryableErrorFn = dyn Fn(&DxfError) -> bool + Send + Sync;

#[derive(Clone)]
/// 测试用步骤执行器：绑定固定 step 与 run 回调。
pub struct StepExecutor {
    /// 本执行器对应的步骤。
    step: Step,
    /// 实际执行子任务的回调。
    run_subtask: Arc<RunSubtaskFn>,
}

impl StepExecutor {
    /// 初始化（测试桩恒成功）。
    pub fn init(&self) -> Result<(), DxfError> {
        Ok(())
    }

    /// 调用注入的 run 回调执行子任务。
    pub fn run_subtask(&self, subtask: &Subtask) -> Result<(), DxfError> {
        (self.run_subtask)(subtask)
    }

    /// 返回绑定的步骤。
    pub fn step(&self) -> Step {
        self.step
    }

    /// 清理（测试桩恒成功）。
    pub fn cleanup(&self) -> Result<(), DxfError> {
        Ok(())
    }

    /// 实时摘要（测试桩不提供）。
    pub fn realtime_summary(&self) -> Option<Vec<u8>> {
        None
    }
}

#[derive(Clone)]
/// 任务执行器扩展：提供步骤执行器工厂与可重试错误判断。
pub struct TaskExecutorExtension {
    /// 步骤执行器工厂。
    get_step_executor: Arc<GetStepExecutorFn>,
    /// 错误可重试判定。
    is_retryable_error: Arc<RetryableErrorFn>,
}

impl TaskExecutorExtension {
    /// 子任务是否幂等（idempotent：重复执行结果一致）；测试默认恒为 true。
    pub fn is_idempotent(&self, _subtask: &Subtask) -> bool {
        true
    }

    /// 按任务取得步骤执行器。
    pub fn get_step_executor(&self, task: &Task) -> Result<StepExecutor, DxfError> {
        (self.get_step_executor)(task)
    }

    /// 委托注入回调判断错误是否可重试。
    pub fn is_retryable_error(&self, error: &DxfError) -> bool {
        (self.is_retryable_error)(error)
    }
}

#[derive(Clone)]
/// 任务级清理例程包装。
pub struct CleanUpRoutine(Arc<dyn Fn(i64) -> Result<(), DxfError> + Send + Sync>);

impl CleanUpRoutine {
    /// 按任务 ID 执行清理。
    pub fn cleanup(&self, task_id: i64) -> Result<(), DxfError> {
        (self.0)(task_id)
    }
}

#[allow(non_snake_case)]
/// 构造默认「错误不可重试」的通用执行器扩展。
pub fn GetCommonTaskExecutorExt(
    get_step_executor: Arc<GetStepExecutorFn>,
) -> TaskExecutorExtension {
    GetTaskExecutorExt(get_step_executor, Arc::new(|_| false))
}

#[allow(non_snake_case)]
/// 用自定义工厂与可重试判定构造执行器扩展。
pub fn GetTaskExecutorExt(
    get_step_executor: Arc<GetStepExecutorFn>,
    is_retryable_error: Arc<RetryableErrorFn>,
) -> TaskExecutorExtension {
    TaskExecutorExtension {
        get_step_executor,
        is_retryable_error,
    }
}

#[allow(non_snake_case)]
/// 构造绑定指定 step 与 run 回调的步骤执行器。
pub fn GetCommonStepExecutor(step: Step, run_subtask: Arc<RunSubtaskFn>) -> StepExecutor {
    StepExecutor { step, run_subtask }
}

#[allow(non_snake_case)]
/// 空操作清理例程。
pub fn GetCommonCleanUpRoutine() -> CleanUpRoutine {
    CleanUpRoutine(Arc::new(|_| Ok(())))
}

/// 任务类型注册表：可同时登记调度器、清理与执行器。
pub trait TaskTypeRegistry: Send + Sync {
    /// 注册调度器扩展。
    fn register_scheduler(
        &self,
        task_type: &str,
        extension: SchedulerExtension,
    ) -> Result<(), DxfError>;
    /// 注册清理例程。
    fn register_cleanup(&self, task_type: &str, cleanup: CleanUpRoutine) -> Result<(), DxfError>;
    /// 注册执行器扩展。
    fn register_executor(
        &self,
        task_type: &str,
        extension: TaskExecutorExtension,
    ) -> Result<(), DxfError>;
    /// 清空全部注册。
    fn clear_registrations(&self);
}

/// RAII 守卫：Drop 时清空注册表。
pub struct RegistrationGuard {
    /// 持有的注册表。
    registry: Arc<dyn TaskTypeRegistry>,
}

impl Drop for RegistrationGuard {
    fn drop(&mut self) {
        self.registry.clear_registrations();
    }
}

#[allow(non_snake_case)]
/// 注册名为 `example` 的完整任务类型（调度+执行+清理）。
pub fn RegisterExampleTask(
    registry: Arc<dyn TaskTypeRegistry>,
    scheduler: SchedulerExtension,
    executor: TaskExecutorExtension,
    cleanup: CleanUpRoutine,
) -> Result<RegistrationGuard, DxfError> {
    RegisterTaskType(registry, TASK_TYPE_EXAMPLE, scheduler, executor, cleanup)
}

#[allow(non_snake_case)]
/// 按给定类型名注册调度器、清理与执行器；失败则清空回滚。
pub fn RegisterTaskType(
    registry: Arc<dyn TaskTypeRegistry>,
    task_type: &str,
    scheduler: SchedulerExtension,
    executor: TaskExecutorExtension,
    cleanup: CleanUpRoutine,
) -> Result<RegistrationGuard, DxfError> {
    // 顺序注册；任一步失败则 clear 并返回错误。
    let register_result = (|| {
        registry.register_scheduler(task_type, scheduler)?;
        registry.register_cleanup(task_type, cleanup)?;
        registry.register_executor(task_type, executor)
    })();
    if let Err(error) = register_result {
        registry.clear_registrations();
        return Err(error);
    }
    Ok(RegistrationGuard { registry })
}

#[allow(non_snake_case)]
/// 为回滚测试注册 example 任务：执行时把子任务记入 TestContext。
pub fn RegisterTaskTypeForRollback(
    registry: Arc<dyn TaskTypeRegistry>,
    scheduler: SchedulerExtension,
    test_context: Arc<TestContext>,
) -> Result<RegistrationGuard, DxfError> {
    // 子任务执行仅收集到测试上下文，不产生真实副作用。
    let run = Arc::new(move |subtask: &Subtask| {
        test_context.CollectSubtask(subtask);
        Ok(())
    });
    let executor = GetCommonTaskExecutorExt(Arc::new(move |task: &Task| {
        Ok(GetCommonStepExecutor(task.base.step, run.clone()))
    }));
    RegisterExampleTask(registry, scheduler, executor, GetCommonCleanUpRoutine())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 等待任务结束时的条件。
pub enum WaitCondition {
    /// 完成或暂停均可返回。
    DoneOrPaused,
    /// 仅在真正结束（成功/失败）时返回。
    Done,
}

/// 分布式任务运行时：提交、查询与等待任务。
pub trait DistributedTaskRuntime {
    /// 提交新任务，返回任务 ID。
    fn submit_task(
        &self,
        task_key: &str,
        task_type: &str,
        keyspace: &str,
        concurrency: usize,
        target_scope: &str,
        meta: &[u8],
    ) -> Result<i64, DxfError>;
    /// 按 key 查任务（含历史表）。
    fn get_task_by_key_with_history(&self, task_key: &str) -> Result<TaskBase, DxfError>;
    /// 阻塞等待任务满足条件。
    fn wait_task(&self, task_id: i64, condition: WaitCondition) -> Result<TaskBase, DxfError>;
}

#[allow(non_snake_case)]
/// 提交 example 任务并等待其 DoneOrPaused。
pub fn SubmitAndWaitTask(
    runtime: &dyn DistributedTaskRuntime,
    task_key: &str,
    target_scope: &str,
    concurrency: usize,
    next_generation_kernel: bool,
) -> Result<TaskBase, DxfError> {
    runtime.submit_task(
        task_key,
        TASK_TYPE_EXAMPLE,
        getTaskKS(next_generation_kernel),
        concurrency,
        target_scope,
        &[],
    )?;
    WaitTaskDoneOrPaused(runtime, task_key)
}

#[allow(non_snake_case)]
/// 等待任务完成或暂停。
pub fn WaitTaskDoneOrPaused(
    runtime: &dyn DistributedTaskRuntime,
    task_key: &str,
) -> Result<TaskBase, DxfError> {
    waitTaskUntil(runtime, task_key, WaitCondition::DoneOrPaused)
}

#[allow(non_snake_case)]
/// 等待任务真正结束。
pub fn WaitTaskDone(
    runtime: &dyn DistributedTaskRuntime,
    task_key: &str,
) -> Result<TaskBase, DxfError> {
    waitTaskUntil(runtime, task_key, WaitCondition::Done)
}

#[allow(non_snake_case)]
/// 按条件等待任务；内部先取历史再委托 runtime.wait_task。
pub fn waitTaskUntil(
    runtime: &dyn DistributedTaskRuntime,
    task_key: &str,
    condition: WaitCondition,
) -> Result<TaskBase, DxfError> {
    let task = runtime.get_task_by_key_with_history(task_key)?;
    // 与 Go 一致：Paused 不视为 Done，继续交给 wait_task 等待。
    if condition == WaitCondition::Done && task.state == TaskState::Paused {
        // Preserve the Go behavior: delegate waiting rather than treating paused as done.
    }
    runtime.wait_task(task.id, condition)
}
