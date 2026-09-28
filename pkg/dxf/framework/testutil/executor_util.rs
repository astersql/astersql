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

// 任务执行器（task executor）注册测试辅助。
//
// 将示例任务类型挂到执行器注册表，并在 Drop 时清理，避免测试间互相污染。
// DXF 中每个任务类型对应一套 step executor（按任务步骤执行子任务的组件）。

use std::sync::Arc;

use crate::context::{DxfError, Subtask, Task};
use crate::disttest_util::{
    GetCommonStepExecutor, GetCommonTaskExecutorExt, RunSubtaskFn, TASK_TYPE_EXAMPLE,
    TaskExecutorExtension,
};

/// 测试用的任务执行器注册表抽象。
///
/// 对应 DXF 进程内按任务类型登记 `TaskExecutorExtension` 的能力。
pub trait TaskExecutorRegistry: Send + Sync {
    /// 为指定任务类型注册执行器扩展。
    fn register_executor(
        &self,
        task_type: &str,
        extension: TaskExecutorExtension,
    ) -> Result<(), DxfError>;
    /// 清空全部已注册执行器。
    fn clear_executors(&self);
}

/// RAII 守卫：离开作用域时自动 `clear_executors`。
pub struct ExecutorRegistrationGuard {
    /// 持有的注册表引用。
    registry: Arc<dyn TaskExecutorRegistry>,
}

impl Drop for ExecutorRegistrationGuard {
    fn drop(&mut self) {
        self.registry.clear_executors();
    }
}

#[allow(non_snake_case)]
/// 注册名为 `example` 的通用 step executor，并返回清理守卫。
///
/// `run_subtask` 是子任务实际执行回调；失败时会先清空注册再返回错误。
pub fn InitTaskExecutor(
    registry: Arc<dyn TaskExecutorRegistry>,
    run_subtask: Arc<RunSubtaskFn>,
) -> Result<ExecutorRegistrationGuard, DxfError> {
    // 按任务当前 step 构造通用 StepExecutor，注入调用方提供的 run 回调。
    let extension = GetCommonTaskExecutorExt(Arc::new(move |task: &Task| {
        Ok(GetCommonStepExecutor(task.base.step, run_subtask.clone()))
    }));
    // 注册失败时回滚，避免半注册状态残留。
    if let Err(error) = registry.register_executor(TASK_TYPE_EXAMPLE, extension) {
        registry.clear_executors();
        return Err(error);
    }
    Ok(ExecutorRegistrationGuard { registry })
}

/// 通过扩展取出当前步骤的执行器，依次执行 init / run / cleanup。
///
/// cleanup 错误会与 run 结果合并（`and`），保证清理仍被尝试。
pub fn run_registered_subtask(
    extension: &TaskExecutorExtension,
    task: &Task,
    subtask: &Subtask,
) -> Result<(), DxfError> {
    // 获取步骤执行器后按 init → run_subtask → cleanup 生命周期跑一遍。
    let executor = extension.get_step_executor(task)?;
    executor.init()?;
    let result = executor.run_subtask(subtask);
    let cleanup = executor.cleanup();
    result.and(cleanup)
}
