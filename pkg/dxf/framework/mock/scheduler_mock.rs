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

// 调度器（Scheduler）、清理例程与任务管理器（TaskManager）的 mock。
//
// 用 `Handler` + `mock_method!` 宏生成 GoMock 风格方法派发，覆盖任务生命周期
//（Init/OnPrepare/OnNextSubtasksBatch/OnDone）、状态切换与会话/事务包装。
// Scheduler 负责任务在各 Step 间推进；TaskManager 持久化任务/子任务状态。
// limitations under the License.

use std::any::Any;
use std::collections::HashMap;

use astersql_dxf_framework_proto as proto;
use astersql_dxf_framework_storage as storage;
use astersql_dxf_framework_taskexecutor_execute as execute;

use crate::Handler;

// 存储层错误包装的结果类型。
type MockResult<T> = Result<T, storage::Error>;
// 在新 Session/Txn 内执行的一次性回调。
type SessionCallback = Box<dyn FnOnce(storage::sessionctx::Context) -> MockResult<()> + Send>;

// 将同名 Handler 字段包装为公开方法并派发。
macro_rules! mock_method {
    ($name:ident($($argument:ident: $argument_type:ty),* $(,)?) -> $return_type:ty) => {
        pub fn $name(&self, $($argument: $argument_type),*) -> $return_type {
            self.$name.invoke(concat!(stringify!($name), " called"), |handler| {
                handler($($argument),*)
            })
        }
    };
}

/// 与 GoMock 兼容的调度器替身，由可配置闭包驱动。
/// GoMock-compatible scheduler double backed by configurable Rust closures.
///
/// `H` 为具体任务句柄类型：Go 可直接携带 interface，而 Rust 的 TaskHandle 非对象安全。
/// `H` is the concrete task-handle type. The Go interface can carry an
/// interface value directly, while Rust's `TaskHandle` is not object-safe.
pub struct MockScheduler<H> {
    /// 关闭调度器。
    pub Close: Handler<dyn FnMut() + Send>,
    /// 查询可运行该任务的执行实例 ID 列表。
    pub GetEligibleInstances:
        Handler<dyn FnMut(storage::Context, &proto::Task) -> MockResult<Vec<String>> + Send>,
    /// 根据任务基信息计算下一步 Step。
    pub GetNextStep: Handler<dyn FnMut(&proto::TaskBase) -> proto::Step + Send>,
    /// 获取当前绑定任务。
    pub GetTask: Handler<dyn FnMut() -> Option<Box<proto::Task>> + Send>,
    /// 初始化调度器。
    pub Init: Handler<dyn FnMut() -> MockResult<()> + Send>,
    /// 判断错误是否可重试。
    pub IsRetryableErr: Handler<dyn FnMut(storage::Error) -> bool + Send>,
    /// 按修改列表改写任务 meta 字节。
    pub ModifyMeta:
        Handler<dyn FnMut(Vec<u8>, Vec<proto::Modification>) -> MockResult<Vec<u8>> + Send>,
    /// 任务完成回调。
    pub OnDone: Handler<dyn FnMut(storage::Context, H, &proto::Task) -> MockResult<()> + Send>,
    /// 为下一 Step 批量生成子任务 meta。
    pub OnNextSubtasksBatch: Handler<
        dyn FnMut(
                storage::Context,
                H,
                &proto::Task,
                Vec<String>,
                proto::Step,
            ) -> MockResult<Vec<Vec<u8>>>
            + Send,
    >,
    /// 进入下一步前的准备阶段。
    pub OnPrepare:
        Handler<dyn FnMut(storage::Context, H, &mut proto::Task) -> MockResult<()> + Send>,
    /// 周期性心跳/巡检回调。
    pub OnTick: Handler<dyn FnMut(storage::Context, &proto::Task) + Send>,
    /// 触发一次调度。
    pub ScheduleTask: Handler<dyn FnMut() + Send>,
}

/// 全部 Handler 置空。
impl<H> Default for MockScheduler<H> {
    fn default() -> Self {
        Self {
            Close: Handler::default(),
            GetEligibleInstances: Handler::default(),
            GetNextStep: Handler::default(),
            GetTask: Handler::default(),
            Init: Handler::default(),
            IsRetryableErr: Handler::default(),
            ModifyMeta: Handler::default(),
            OnDone: Handler::default(),
            OnNextSubtasksBatch: Handler::default(),
            OnPrepare: Handler::default(),
            OnTick: Handler::default(),
            ScheduleTask: Handler::default(),
        }
    }
}

/// 期望记录器别名。
pub type MockSchedulerMockRecorder<H> = MockScheduler<H>;

/// EXPECT / ISGOMOCK 与各方法派发。
impl<H> MockScheduler<H> {
    /// 返回期望记录器。
    pub fn EXPECT(&mut self) -> &mut MockSchedulerMockRecorder<H> {
        self
    }

    /// GoMock 标记占位。
    pub fn ISGOMOCK(&self) {}

    mock_method!(Close() -> ());
    mock_method!(GetEligibleInstances(context: storage::Context, task: &proto::Task) -> MockResult<Vec<String>>);
    mock_method!(GetNextStep(task: &proto::TaskBase) -> proto::Step);
    mock_method!(GetTask() -> Option<Box<proto::Task>>);
    mock_method!(Init() -> MockResult<()>);
    mock_method!(IsRetryableErr(error: storage::Error) -> bool);
    mock_method!(ModifyMeta(meta: Vec<u8>, modifications: Vec<proto::Modification>) -> MockResult<Vec<u8>>);
    mock_method!(OnDone(context: storage::Context, handle: H, task: &proto::Task) -> MockResult<()>);
    mock_method!(OnNextSubtasksBatch(
        context: storage::Context,
        handle: H,
        task: &proto::Task,
        exec_ids: Vec<String>,
        next_step: proto::Step,
    ) -> MockResult<Vec<Vec<u8>>>);
    mock_method!(OnPrepare(context: storage::Context, handle: H, task: &mut proto::Task) -> MockResult<()>);
    mock_method!(OnTick(context: storage::Context, task: &proto::Task) -> ());
    mock_method!(ScheduleTask() -> ());
}

/// 构造空期望 MockScheduler。
pub fn NewMockScheduler<H, C: ?Sized>(_controller: &C) -> MockScheduler<H> {
    MockScheduler::default()
}

/// 任务后置清理例程 mock（如历史表迁移、中间文件清理）。
#[derive(Default)]
pub struct MockCleanUpRoutine {
    /// 执行清理。
    pub CleanUp: Handler<dyn FnMut(storage::Context, &mut proto::Task) -> MockResult<()> + Send>,
}

/// CleanUpRoutine 期望记录器别名。
pub type MockCleanUpRoutineMockRecorder = MockCleanUpRoutine;

/// GoMock 风格 API。
impl MockCleanUpRoutine {
    /// 返回期望记录器。
    pub fn EXPECT(&mut self) -> &mut MockCleanUpRoutineMockRecorder {
        self
    }

    /// GoMock 标记占位。
    pub fn ISGOMOCK(&self) {}

    mock_method!(CleanUp(context: storage::Context, task: &mut proto::Task) -> MockResult<()>);
}

/// 构造空期望 MockCleanUpRoutine。
pub fn NewMockCleanUpRoutine<C: ?Sized>(_controller: &C) -> MockCleanUpRoutine {
    MockCleanUpRoutine::default()
}

/// 任务管理器 mock：查询/推进任务与子任务状态、暂停/回滚/成功等。
#[derive(Default)]
pub struct MockTaskManager {
    /// 将任务标为 awaiting-resolution（等待人工/外部解决）。
    pub AwaitingResolveTask: Handler<
        dyn FnMut(storage::Context, i64, proto::TaskState, storage::Error) -> MockResult<()> + Send,
    >,
    /// 取消任务。
    pub CancelTask: Handler<dyn FnMut(storage::Context, i64) -> MockResult<()> + Send>,
    /// 删除已失效节点记录。
    pub DeleteDeadNodes: Handler<dyn FnMut(storage::Context, Vec<String>) -> MockResult<()> + Send>,
    /// 标记任务失败。
    pub FailTask: Handler<
        dyn FnMut(storage::Context, i64, proto::TaskState, storage::Error) -> MockResult<()> + Send,
    >,
    /// 垃圾回收过期子任务。
    pub GCSubtasks: Handler<dyn FnMut(storage::Context) -> MockResult<()> + Send>,
    /// 获取任务下活跃子任务。
    pub GetActiveSubtasks: Handler<
        dyn FnMut(storage::Context, i64) -> MockResult<Vec<Box<proto::SubtaskBase>>> + Send,
    >,
    /// 列出所有托管节点（ManagedNode）。
    pub GetAllNodes:
        Handler<dyn FnMut(storage::Context) -> MockResult<Vec<proto::ManagedNode>> + Send>,
    /// 按 Step 汇总子任务执行摘要。
    pub GetAllSubtaskSummaryByStep: Handler<
        dyn FnMut(
                storage::Context,
                i64,
                proto::Step,
            ) -> MockResult<Vec<Box<execute::SubtaskSummary>>>
            + Send,
    >,
    /// 列出全部子任务基信息。
    pub GetAllSubtasks:
        Handler<dyn FnMut(storage::Context) -> MockResult<Vec<Box<proto::SubtaskBase>>> + Send>,
    /// 按 Step 与子任务状态过滤。
    pub GetAllSubtasksByStepAndState: Handler<
        dyn FnMut(
                storage::Context,
                i64,
                proto::Step,
                proto::SubtaskState,
            ) -> MockResult<Vec<Box<proto::Subtask>>>
            + Send,
    >,
    /// 列出全部任务基信息。
    pub GetAllTasks:
        Handler<dyn FnMut(storage::Context) -> MockResult<Vec<Box<proto::TaskBase>>> + Send>,
    /// 按状态分组统计子任务数。
    pub GetSubtaskCntGroupByStates: Handler<
        dyn FnMut(
                storage::Context,
                i64,
                proto::Step,
            ) -> MockResult<HashMap<proto::SubtaskState, i64>>
            + Send,
    >,
    /// 某 Step 下状态计数与错误列表。
    pub GetSubtaskStateCntAndErrorsByStep: Handler<
        dyn FnMut(
                storage::Context,
                i64,
                proto::Step,
            )
                -> MockResult<(HashMap<proto::SubtaskState, i64>, Vec<storage::Error>)>
            + Send,
    >,
    /// 任务相关子任务错误集合。
    pub GetSubtaskErrors:
        Handler<dyn FnMut(storage::Context, i64) -> MockResult<Vec<storage::Error>> + Send>,
    /// 按 ID 取 TaskBase。
    pub GetTaskBaseByID: Handler<
        dyn FnMut(storage::Context, i64) -> MockResult<Option<Box<proto::TaskBase>>> + Send,
    >,
    /// 按 ID 取完整 Task。
    pub GetTaskByID:
        Handler<dyn FnMut(storage::Context, i64) -> MockResult<Option<Box<proto::Task>>> + Send>,
    /// 按状态集合查询任务（状态以 Any 装箱以兼容 Go 可变参）。
    pub GetTasksInStates: Handler<
        dyn FnMut(storage::Context, Vec<Box<dyn Any + Send>>) -> MockResult<Vec<Box<proto::Task>>>
            + Send,
    >,
    /// 取出不需额外资源的高优先级任务。
    pub GetTopNoNeedResourceTasks:
        Handler<dyn FnMut(storage::Context) -> MockResult<Vec<Box<proto::TaskBase>>> + Send>,
    /// 取出未完成的高优先级任务。
    pub GetTopUnfinishedTasks:
        Handler<dyn FnMut(storage::Context) -> MockResult<Vec<Box<proto::TaskBase>>> + Send>,
    /// 各节点已占用 slot 数。
    pub GetUsedSlotsOnNodes:
        Handler<dyn FnMut(storage::Context) -> MockResult<HashMap<String, i32>> + Send>,
    /// 任务被修改后的落库/通知。
    pub ModifiedTask:
        Handler<dyn FnMut(storage::Context, Box<proto::Task>) -> MockResult<()> + Send>,
    /// 按任务键暂停。
    pub PauseTask: Handler<dyn FnMut(storage::Context, String) -> MockResult<bool> + Send>,
    /// 因错误在指定状态/Step 暂停。
    pub PauseTaskOnError: Handler<
        dyn FnMut(
                storage::Context,
                i64,
                proto::TaskState,
                proto::Step,
                storage::Error,
            ) -> MockResult<()>
            + Send,
    >,
    /// 确认任务已暂停。
    pub PausedTask: Handler<dyn FnMut(storage::Context, i64) -> MockResult<()> + Send>,
    /// 恢复子任务。
    pub ResumeSubtasks: Handler<dyn FnMut(storage::Context, i64) -> MockResult<()> + Send>,
    /// 确认任务已恢复。
    pub ResumedTask: Handler<dyn FnMut(storage::Context, i64) -> MockResult<()> + Send>,
    /// 启动任务回滚。
    pub RevertTask: Handler<
        dyn FnMut(storage::Context, i64, proto::TaskState, storage::Error) -> MockResult<()> + Send,
    >,
    /// 确认回滚完成。
    pub RevertedTask: Handler<dyn FnMut(storage::Context, i64) -> MockResult<()> + Send>,
    /// 标记任务成功。
    pub SucceedTask: Handler<dyn FnMut(storage::Context, i64) -> MockResult<()> + Send>,
    /// Prepare 完成后尝试切换 Step。
    pub SwitchTaskStepAfterPrepare:
        Handler<dyn FnMut(storage::Context, Box<proto::Task>) -> MockResult<bool> + Send>,
    /// 切换任务状态与 Step，并附带新子任务。
    pub SwitchTaskStep: Handler<
        dyn FnMut(
                storage::Context,
                Box<proto::Task>,
                proto::TaskState,
                proto::Step,
                Vec<Box<proto::Subtask>>,
            ) -> MockResult<()>
            + Send,
    >,
    /// 批量方式切换 Step（大批量子任务场景）。
    pub SwitchTaskStepInBatch: Handler<
        dyn FnMut(
                storage::Context,
                Box<proto::Task>,
                proto::TaskState,
                proto::Step,
                Vec<Box<proto::Subtask>>,
            ) -> MockResult<()>
            + Send,
    >,
    /// 将任务迁入历史表。
    pub TransferTasks2History:
        Handler<dyn FnMut(storage::Context, Vec<Box<proto::Task>>) -> MockResult<()> + Send>,
    /// 更新子任务绑定的执行器 ID。
    pub UpdateSubtasksExecIDs:
        Handler<dyn FnMut(storage::Context, Vec<Box<proto::SubtaskBase>>) -> MockResult<()> + Send>,
    /// 在新 Session 中执行回调。
    pub WithNewSession: Handler<dyn FnMut(SessionCallback) -> MockResult<()> + Send>,
    /// 在新事务（Txn）中执行回调。
    pub WithNewTxn: Handler<dyn FnMut(storage::Context, SessionCallback) -> MockResult<()> + Send>,
}

/// TaskManager 期望记录器别名。
pub type MockTaskManagerMockRecorder = MockTaskManager;

/// EXPECT / ISGOMOCK 与全部方法派发。
impl MockTaskManager {
    /// 返回期望记录器。
    pub fn EXPECT(&mut self) -> &mut MockTaskManagerMockRecorder {
        self
    }

    /// GoMock 标记占位。
    pub fn ISGOMOCK(&self) {}

    mock_method!(AwaitingResolveTask(context: storage::Context, task_id: i64, state: proto::TaskState, error: storage::Error) -> MockResult<()>);
    mock_method!(CancelTask(context: storage::Context, task_id: i64) -> MockResult<()>);
    mock_method!(DeleteDeadNodes(context: storage::Context, nodes: Vec<String>) -> MockResult<()>);
    mock_method!(FailTask(context: storage::Context, task_id: i64, state: proto::TaskState, error: storage::Error) -> MockResult<()>);
    mock_method!(GCSubtasks(context: storage::Context) -> MockResult<()>);
    mock_method!(GetActiveSubtasks(context: storage::Context, task_id: i64) -> MockResult<Vec<Box<proto::SubtaskBase>>>);
    mock_method!(GetAllNodes(context: storage::Context) -> MockResult<Vec<proto::ManagedNode>>);
    mock_method!(GetAllSubtaskSummaryByStep(context: storage::Context, task_id: i64, step: proto::Step) -> MockResult<Vec<Box<execute::SubtaskSummary>>>);
    mock_method!(GetAllSubtasks(context: storage::Context) -> MockResult<Vec<Box<proto::SubtaskBase>>>);
    mock_method!(GetAllSubtasksByStepAndState(context: storage::Context, task_id: i64, step: proto::Step, state: proto::SubtaskState) -> MockResult<Vec<Box<proto::Subtask>>>);
    mock_method!(GetAllTasks(context: storage::Context) -> MockResult<Vec<Box<proto::TaskBase>>>);
    mock_method!(GetSubtaskCntGroupByStates(context: storage::Context, task_id: i64, step: proto::Step) -> MockResult<HashMap<proto::SubtaskState, i64>>);
    mock_method!(GetSubtaskStateCntAndErrorsByStep(context: storage::Context, task_id: i64, step: proto::Step) -> MockResult<(HashMap<proto::SubtaskState, i64>, Vec<storage::Error>)>);
    mock_method!(GetSubtaskErrors(context: storage::Context, task_id: i64) -> MockResult<Vec<storage::Error>>);
    mock_method!(GetTaskBaseByID(context: storage::Context, task_id: i64) -> MockResult<Option<Box<proto::TaskBase>>>);
    mock_method!(GetTaskByID(context: storage::Context, task_id: i64) -> MockResult<Option<Box<proto::Task>>>);
    mock_method!(GetTasksInStates(context: storage::Context, states: Vec<Box<dyn Any + Send>>) -> MockResult<Vec<Box<proto::Task>>>);
    mock_method!(GetTopNoNeedResourceTasks(context: storage::Context) -> MockResult<Vec<Box<proto::TaskBase>>>);
    mock_method!(GetTopUnfinishedTasks(context: storage::Context) -> MockResult<Vec<Box<proto::TaskBase>>>);
    mock_method!(GetUsedSlotsOnNodes(context: storage::Context) -> MockResult<HashMap<String, i32>>);
    mock_method!(ModifiedTask(context: storage::Context, task: Box<proto::Task>) -> MockResult<()>);
    mock_method!(PauseTask(context: storage::Context, task_key: String) -> MockResult<bool>);
    mock_method!(PauseTaskOnError(context: storage::Context, task_id: i64, state: proto::TaskState, step: proto::Step, error: storage::Error) -> MockResult<()>);
    mock_method!(PausedTask(context: storage::Context, task_id: i64) -> MockResult<()>);
    mock_method!(ResumeSubtasks(context: storage::Context, task_id: i64) -> MockResult<()>);
    mock_method!(ResumedTask(context: storage::Context, task_id: i64) -> MockResult<()>);
    mock_method!(RevertTask(context: storage::Context, task_id: i64, state: proto::TaskState, error: storage::Error) -> MockResult<()>);
    mock_method!(RevertedTask(context: storage::Context, task_id: i64) -> MockResult<()>);
    mock_method!(SucceedTask(context: storage::Context, task_id: i64) -> MockResult<()>);
    mock_method!(SwitchTaskStepAfterPrepare(context: storage::Context, task: Box<proto::Task>) -> MockResult<bool>);
    mock_method!(SwitchTaskStep(context: storage::Context, task: Box<proto::Task>, next_state: proto::TaskState, next_step: proto::Step, subtasks: Vec<Box<proto::Subtask>>) -> MockResult<()>);
    mock_method!(SwitchTaskStepInBatch(context: storage::Context, task: Box<proto::Task>, next_state: proto::TaskState, next_step: proto::Step, subtasks: Vec<Box<proto::Subtask>>) -> MockResult<()>);
    mock_method!(TransferTasks2History(context: storage::Context, tasks: Vec<Box<proto::Task>>) -> MockResult<()>);
    mock_method!(UpdateSubtasksExecIDs(context: storage::Context, subtasks: Vec<Box<proto::SubtaskBase>>) -> MockResult<()>);
    mock_method!(WithNewSession(callback: SessionCallback) -> MockResult<()>);
    mock_method!(WithNewTxn(context: storage::Context, callback: SessionCallback) -> MockResult<()>);
}

/// 构造空期望 MockTaskManager。
pub fn NewMockTaskManager<C: ?Sized>(_controller: &C) -> MockTaskManager {
    MockTaskManager::default()
}
