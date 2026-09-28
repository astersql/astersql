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

// 任务表（TaskTable）、任务执行器（TaskExecutor）与扩展（Extension）的 mock。
//
// 覆盖子任务启停/失败/完成、检查点、meta 恢复，以及执行器 Cancel/Run 与
// 按任务获取 StepExecutor 的扩展钩子，对齐 GoMock API。

use std::any::Any;

use astersql_dxf_framework_proto as proto;
use astersql_dxf_framework_storage as storage;
use astersql_dxf_framework_taskexecutor_execute as execute;

use crate::Handler;

// 存储错误包装结果。
type MockResult<T> = Result<T, storage::Error>;
// 新 Session 内执行的回调。
type SessionCallback = Box<dyn FnOnce(storage::sessionctx::Context) -> MockResult<()> + Send>;

// 将同名 Handler 字段包装为公开方法。
macro_rules! mock_method {
    ($name:ident($($argument:ident: $argument_type:ty),* $(,)?) -> $return_type:ty) => {
        pub fn $name(&self, $($argument: $argument_type),*) -> $return_type {
            self.$name.invoke(concat!(stringify!($name), " called"), |handler| {
                handler($($argument),*)
            })
        }
    };
}

/// `GetTaskExecInfoByExecID` 返回的执行信息。
/// Execution information returned by `TaskTable.GetTaskExecInfoByExecID`.
///
/// 对齐 Go 侧 `TaskExecInfo`；当前 storage crate 尚未导出迁移后的对应类型。
/// This mirrors storage's Go `TaskExecInfo`; the current storage crate has not
/// yet exported its migrated counterpart.
pub struct TaskExecInfo {
    /// 任务基信息。
    pub TaskBase: proto::TaskBase,
    /// 子任务并发度。
    pub SubtaskConcurrency: i32,
}

/// 任务表 mock：执行器侧读写子任务/任务状态的存储接口替身。
#[derive(Default)]
pub struct MockTaskTable {
    /// 取消指定执行器上的子任务。
    pub CancelSubtask: Handler<dyn FnMut(storage::Context, String, i64) -> MockResult<()> + Send>,
    /// 标记子任务失败。
    pub FailSubtask:
        Handler<dyn FnMut(storage::Context, String, i64, storage::Error) -> MockResult<()> + Send>,
    /// 完成子任务并写入结果 meta。
    pub FinishSubtask:
        Handler<dyn FnMut(storage::Context, String, i64, Vec<u8>) -> MockResult<()> + Send>,
    /// 取首个处于给定状态集合的子任务。
    pub GetFirstSubtaskInStates: Handler<
        dyn FnMut(
                storage::Context,
                String,
                i64,
                proto::Step,
                Vec<proto::SubtaskState>,
            ) -> MockResult<Option<Box<proto::Subtask>>>
            + Send,
    >,
    /// 读取子任务检查点。
    pub GetSubtaskCheckpoint:
        Handler<dyn FnMut(storage::Context, i64) -> MockResult<String> + Send>,
    /// 按执行器、Step、状态过滤子任务列表。
    pub GetSubtasksByExecIDAndStepAndStates: Handler<
        dyn FnMut(
                storage::Context,
                String,
                i64,
                proto::Step,
                Vec<proto::SubtaskState>,
            ) -> MockResult<Vec<Box<proto::Subtask>>>
            + Send,
    >,
    /// 按 ID 取 TaskBase。
    pub GetTaskBaseByID: Handler<
        dyn FnMut(storage::Context, i64) -> MockResult<Option<Box<proto::TaskBase>>> + Send,
    >,
    /// 按 ID 取完整 Task。
    pub GetTaskByID:
        Handler<dyn FnMut(storage::Context, i64) -> MockResult<Option<Box<proto::Task>>> + Send>,
    /// 按执行器 ID 取任务执行信息列表。
    pub GetTaskExecInfoByExecID:
        Handler<dyn FnMut(storage::Context, String) -> MockResult<Vec<Box<TaskExecInfo>>> + Send>,
    /// 按状态集合查询任务。
    pub GetTasksInStates: Handler<
        dyn FnMut(storage::Context, Vec<Box<dyn Any + Send>>) -> MockResult<Vec<Box<proto::Task>>>
            + Send,
    >,
    /// 初始化执行器 meta（exec_id / role）。
    pub InitMeta: Handler<dyn FnMut(storage::Context, String, String) -> MockResult<()> + Send>,
    /// 暂停某任务在指定执行器上的子任务。
    pub PauseSubtasks: Handler<dyn FnMut(storage::Context, String, i64) -> MockResult<()> + Send>,
    /// 恢复执行器 meta（故障恢复场景）。
    pub RecoverMeta: Handler<dyn FnMut(storage::Context, String, String) -> MockResult<()> + Send>,
    /// 将 running 子任务回退为 pending（如执行器重启）。
    pub RunningSubtasksBack2Pending:
        Handler<dyn FnMut(storage::Context, Vec<Box<proto::SubtaskBase>>) -> MockResult<()> + Send>,
    /// 启动子任务。
    pub StartSubtask: Handler<dyn FnMut(storage::Context, i64, String) -> MockResult<()> + Send>,
    /// 更新子任务检查点（值以 Any 装箱）。
    pub UpdateSubtaskCheckpoint:
        Handler<dyn FnMut(storage::Context, i64, Box<dyn Any + Send>) -> MockResult<()> + Send>,
    /// 更新子任务状态与错误。
    pub UpdateSubtaskStateAndError: Handler<
        dyn FnMut(
                storage::Context,
                String,
                i64,
                proto::SubtaskState,
                storage::Error,
            ) -> MockResult<()>
            + Send,
    >,
    /// 在新 Session 中执行回调。
    pub WithNewSession: Handler<dyn FnMut(SessionCallback) -> MockResult<()> + Send>,
}

/// TaskTable 期望记录器别名。
pub type MockTaskTableMockRecorder = MockTaskTable;

/// EXPECT / ISGOMOCK 与方法派发。
impl MockTaskTable {
    /// 返回期望记录器。
    pub fn EXPECT(&mut self) -> &mut MockTaskTableMockRecorder {
        self
    }

    /// GoMock 标记占位。
    pub fn ISGOMOCK(&self) {}

    mock_method!(CancelSubtask(context: storage::Context, exec_id: String, subtask_id: i64) -> MockResult<()>);
    mock_method!(FailSubtask(context: storage::Context, exec_id: String, subtask_id: i64, error: storage::Error) -> MockResult<()>);
    mock_method!(FinishSubtask(context: storage::Context, exec_id: String, subtask_id: i64, meta: Vec<u8>) -> MockResult<()>);
    mock_method!(GetFirstSubtaskInStates(context: storage::Context, exec_id: String, task_id: i64, step: proto::Step, states: Vec<proto::SubtaskState>) -> MockResult<Option<Box<proto::Subtask>>>);
    mock_method!(GetSubtaskCheckpoint(context: storage::Context, subtask_id: i64) -> MockResult<String>);
    mock_method!(GetSubtasksByExecIDAndStepAndStates(context: storage::Context, exec_id: String, task_id: i64, step: proto::Step, states: Vec<proto::SubtaskState>) -> MockResult<Vec<Box<proto::Subtask>>>);
    mock_method!(GetTaskBaseByID(context: storage::Context, task_id: i64) -> MockResult<Option<Box<proto::TaskBase>>>);
    mock_method!(GetTaskByID(context: storage::Context, task_id: i64) -> MockResult<Option<Box<proto::Task>>>);
    mock_method!(GetTaskExecInfoByExecID(context: storage::Context, exec_id: String) -> MockResult<Vec<Box<TaskExecInfo>>>);
    mock_method!(GetTasksInStates(context: storage::Context, states: Vec<Box<dyn Any + Send>>) -> MockResult<Vec<Box<proto::Task>>>);
    mock_method!(InitMeta(context: storage::Context, exec_id: String, role: String) -> MockResult<()>);
    mock_method!(PauseSubtasks(context: storage::Context, exec_id: String, task_id: i64) -> MockResult<()>);
    mock_method!(RecoverMeta(context: storage::Context, exec_id: String, role: String) -> MockResult<()>);
    mock_method!(RunningSubtasksBack2Pending(context: storage::Context, subtasks: Vec<Box<proto::SubtaskBase>>) -> MockResult<()>);
    mock_method!(StartSubtask(context: storage::Context, subtask_id: i64, exec_id: String) -> MockResult<()>);
    mock_method!(UpdateSubtaskCheckpoint(context: storage::Context, subtask_id: i64, checkpoint: Box<dyn Any + Send>) -> MockResult<()>);
    mock_method!(UpdateSubtaskStateAndError(context: storage::Context, exec_id: String, subtask_id: i64, state: proto::SubtaskState, error: storage::Error) -> MockResult<()>);
    mock_method!(WithNewSession(callback: SessionCallback) -> MockResult<()>);
}

/// 构造空期望 MockTaskTable。
pub fn NewMockTaskTable<C: ?Sized>(_controller: &C) -> MockTaskTable {
    MockTaskTable::default()
}

/// 任务执行器 mock：控制 Run/Cancel/Init 等生命周期。
#[derive(Default)]
pub struct MockTaskExecutor {
    /// 取消整个执行器。
    pub Cancel: Handler<dyn FnMut() + Send>,
    /// 仅取消当前正在跑的子任务。
    pub CancelRunningSubtask: Handler<dyn FnMut() + Send>,
    /// 关闭执行器。
    pub Close: Handler<dyn FnMut() + Send>,
    /// 获取绑定的 TaskBase。
    pub GetTaskBase: Handler<dyn FnMut() -> Option<Box<proto::TaskBase>> + Send>,
    /// 初始化执行器。
    pub Init: Handler<dyn FnMut(execute::Context) -> MockResult<()> + Send>,
    /// 判断错误是否可重试。
    pub IsRetryableError: Handler<dyn FnMut(storage::Error) -> bool + Send>,
    /// 启动执行循环。
    pub Run: Handler<dyn FnMut() + Send>,
}

/// TaskExecutor 期望记录器别名。
pub type MockTaskExecutorMockRecorder = MockTaskExecutor;

/// GoMock 风格 API。
impl MockTaskExecutor {
    /// 返回期望记录器。
    pub fn EXPECT(&mut self) -> &mut MockTaskExecutorMockRecorder {
        self
    }

    /// GoMock 标记占位。
    pub fn ISGOMOCK(&self) {}

    mock_method!(Cancel() -> ());
    mock_method!(CancelRunningSubtask() -> ());
    mock_method!(Close() -> ());
    mock_method!(GetTaskBase() -> Option<Box<proto::TaskBase>>);
    mock_method!(Init(context: execute::Context) -> MockResult<()>);
    mock_method!(IsRetryableError(error: storage::Error) -> bool);
    mock_method!(Run() -> ());
}

/// 构造空期望 MockTaskExecutor。
pub fn NewMockTaskExecutor<C: ?Sized>(_controller: &C) -> MockTaskExecutor {
    MockTaskExecutor::default()
}

/// 执行扩展 mock：按任务提供 StepExecutor，并声明幂等/可重试策略。
#[derive(Default)]
pub struct MockExtension {
    /// 为给定任务构造步骤执行器。
    pub GetStepExecutor:
        Handler<dyn FnMut(&proto::Task) -> MockResult<Box<dyn execute::StepExecutor>> + Send>,
    /// 子任务是否幂等（可安全重跑）。
    pub IsIdempotent: Handler<dyn FnMut(&proto::Subtask) -> bool + Send>,
    /// 错误是否可重试。
    pub IsRetryableError: Handler<dyn FnMut(storage::Error) -> bool + Send>,
}

/// Extension 期望记录器别名。
pub type MockExtensionMockRecorder = MockExtension;

/// GoMock 风格 API。
impl MockExtension {
    /// 返回期望记录器。
    pub fn EXPECT(&mut self) -> &mut MockExtensionMockRecorder {
        self
    }

    /// GoMock 标记占位。
    pub fn ISGOMOCK(&self) {}

    mock_method!(GetStepExecutor(task: &proto::Task) -> MockResult<Box<dyn execute::StepExecutor>>);
    mock_method!(IsIdempotent(subtask: &proto::Subtask) -> bool);
    mock_method!(IsRetryableError(error: storage::Error) -> bool);
}

/// 构造空期望 MockExtension。
pub fn NewMockExtension<C: ?Sized>(_controller: &C) -> MockExtension {
    MockExtension::default()
}
