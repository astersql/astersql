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

// 任务表（task table）测试夹具与查询封装。
//
// 任务表持久化 DXF 任务与子任务状态；本模块提供 `TaskTable` trait、
// 嵌入式存储初始化守卫，以及 Mock 节点资源（CPU/内存/磁盘）的辅助。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use crate::context::{DxfError, NodeResource, Subtask, Task, TaskState};
use crate::task_util::NewSubtask;

/// 任务/子任务存储抽象（对应 Go 的 storage.TaskManager）。
pub trait TaskTable: Send + Sync {
    /// 插入子任务，返回新 ID。
    fn insert_subtask(&self, subtask: NewSubtask) -> Result<i64, DxfError>;
    /// 取一个待处理任务。
    fn get_one_pending_task(&self) -> Result<Option<Task>, DxfError>;
    /// 历史表中的子任务数量（可按 task_id 过滤）。
    fn subtask_history_count(&self, task_id: Option<i64>) -> Result<usize, DxfError>;
    /// 按任务 ID 列出子任务。
    fn subtasks_by_task_id(&self, task_id: i64) -> Result<Vec<Subtask>, DxfError>;
    /// 历史任务条数。
    fn task_history_count(&self) -> Result<usize, DxfError>;
    /// 任务结束时间。
    fn task_end_time(&self, task_id: i64) -> Result<Option<SystemTime>, DxfError>;
    /// 子任务结束时间。
    fn subtask_end_time(&self, subtask_id: i64) -> Result<Option<SystemTime>, DxfError>;
    /// 执行过该任务子任务的节点 ID 列表。
    fn subtask_nodes(&self, task_id: i64) -> Result<Vec<String>, DxfError>;
    /// 更新子任务绑定的执行节点。
    fn update_subtask_exec_id(&self, node_id: &str, subtask_id: i64) -> Result<(), DxfError>;
    /// 将子任务迁入历史表。
    fn transfer_subtasks_to_history(&self, task_id: i64) -> Result<(), DxfError>;
    /// 按状态过滤历史任务。
    fn history_tasks_in_states(&self, states: &[TaskState]) -> Result<Vec<Task>, DxfError>;
    /// 删除某任务的全部子任务。
    fn delete_subtasks(&self, task_id: i64) -> Result<(), DxfError>;
    /// 任务是否处于取消中。
    fn is_task_cancelling(&self, task_id: i64) -> Result<bool, DxfError>;
    /// 调试打印子任务信息。
    fn print_subtask_info(&self, task_id: i64) -> Result<(), DxfError>;
}

/// 嵌入式存储 + 会话池的测试运行时。
pub trait TableTestRuntime: Send + Sync {
    /// 创建嵌入式存储/会话池，关闭 dist-task 自启动，并安装全局测试任务表。
    /// Creates the embedded store/session pool, disables dist-task startup and
    /// installs the returned task table as the process-wide test manager.
    fn initialize(
        &self,
        mock_cpu_count: Option<usize>,
    ) -> Result<(String, Arc<dyn TaskTable>), DxfError>;
    /// 关闭指定 store。
    fn shutdown(&self, store_id: &str) -> Result<(), DxfError>;
    /// 设置节点资源并返回旧值。
    fn set_node_resource(&self, resource: NodeResource) -> Result<NodeResource, DxfError>;
}

/// 表测试生命周期守卫：Drop 时取消并 shutdown。
pub struct TableTestGuard {
    /// 测试运行时。
    runtime: Arc<dyn TableTestRuntime>,
    /// 嵌入式 store 标识。
    pub store_id: String,
    /// 已安装的任务表。
    pub task_manager: Arc<dyn TaskTable>,
    /// 取消标志。
    cancelled: Arc<AtomicBool>,
}

impl TableTestGuard {
    /// 取得与守卫共享的取消句柄。
    pub fn cancellation(&self) -> Cancellation {
        Cancellation(self.cancelled.clone())
    }
}

impl Drop for TableTestGuard {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        let _ = self.runtime.shutdown(&self.store_id);
    }
}

#[derive(Clone)]
/// 可跨线程共享的取消信号。
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    /// 置取消标志。
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// 查询是否已取消。
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[allow(non_snake_case)]
/// 以默认 8 CPU 初始化表测试环境。
pub fn InitTableTest(runtime: Arc<dyn TableTestRuntime>) -> Result<TableTestGuard, DxfError> {
    init_table_test(runtime, Some(8))
}

#[allow(non_snake_case)]
/// 初始化表测试并额外返回 Cancellation（不强制 mock CPU）。
pub fn InitTableTestWithCancel(
    // 运行时。
    runtime: Arc<dyn TableTestRuntime>,
) -> Result<(TableTestGuard, Cancellation), DxfError> {
    let guard = init_table_test(runtime, None)?;
    let cancellation = guard.cancellation();
    Ok((guard, cancellation))
}

/// 内部：调用 runtime.initialize 并包装为守卫。
fn init_table_test(
    runtime: Arc<dyn TableTestRuntime>,
    cpu_count: Option<usize>,
) -> Result<TableTestGuard, DxfError> {
    let (store_id, task_manager) = runtime.initialize(cpu_count)?;
    Ok(TableTestGuard {
        runtime,
        store_id,
        task_manager,
        cancelled: Arc::new(AtomicBool::new(false)),
    })
}

#[allow(non_snake_case)]
/// 取一个 Pending 任务。
pub fn GetOneTask(manager: &dyn TaskTable) -> Result<Option<Task>, DxfError> {
    manager.get_one_pending_task()
}

#[allow(non_snake_case)]
/// 历史子任务总数。
pub fn GetSubtasksFromHistory(manager: &dyn TaskTable) -> Result<usize, DxfError> {
    manager.subtask_history_count(None)
}

#[allow(non_snake_case)]
/// 指定任务在历史表中的子任务数。
pub fn GetSubtasksFromHistoryByTaskID(
    manager: &dyn TaskTable,
    task_id: i64,
) -> Result<usize, DxfError> {
    manager.subtask_history_count(Some(task_id))
}

#[allow(non_snake_case)]
/// 按任务 ID 取当前子任务列表。
pub fn GetSubtasksByTaskID(
    manager: &dyn TaskTable,
    task_id: i64,
) -> Result<Vec<Subtask>, DxfError> {
    manager.subtasks_by_task_id(task_id)
}

#[allow(non_snake_case)]
/// 历史任务总数。
pub fn GetTasksFromHistory(manager: &dyn TaskTable) -> Result<usize, DxfError> {
    manager.task_history_count()
}

#[allow(non_snake_case)]
/// 查询任务结束时间。
pub fn GetTaskEndTime(
    manager: &dyn TaskTable,
    task_id: i64,
) -> Result<Option<SystemTime>, DxfError> {
    // Go intentionally treats a failed diagnostic query as the zero time.
    Ok(manager.task_end_time(task_id).unwrap_or(None))
}

#[allow(non_snake_case)]
/// 查询子任务结束时间。
pub fn GetSubtaskEndTime(
    manager: &dyn TaskTable,
    subtask_id: i64,
) -> Result<Option<SystemTime>, DxfError> {
    // Keep the helper's best-effort semantics in parity with GetTaskEndTime.
    Ok(manager.subtask_end_time(subtask_id).unwrap_or(None))
}

#[allow(non_snake_case)]
/// 查询执行节点列表。
pub fn GetSubtaskNodes(manager: &dyn TaskTable, task_id: i64) -> Result<Vec<String>, DxfError> {
    manager.subtask_nodes(task_id)
}

#[allow(non_snake_case)]
/// 更新子任务执行节点 ID。
pub fn UpdateSubtaskExecID(
    manager: &dyn TaskTable,
    node_id: &str,
    subtask_id: i64,
) -> Result<(), DxfError> {
    manager.update_subtask_exec_id(node_id, subtask_id)
}

#[allow(non_snake_case)]
/// 将子任务转入历史表。
pub fn TransferSubTasks2History(manager: &dyn TaskTable, task_id: i64) -> Result<(), DxfError> {
    manager.transfer_subtasks_to_history(task_id)
}

#[allow(non_snake_case)]
/// 按状态列表过滤历史任务；空列表返回空 Vec。
pub fn GetTasksFromHistoryInStates(
    manager: &dyn TaskTable,
    states: &[TaskState],
) -> Result<Vec<Task>, DxfError> {
    // 与 Go 行为一致：空状态列表不查库。
    if states.is_empty() {
        return Ok(Vec::new());
    }
    manager.history_tasks_in_states(states)
}

#[allow(non_snake_case)]
/// 删除任务下全部子任务。
pub fn DeleteSubtasksByTaskID(manager: &dyn TaskTable, task_id: i64) -> Result<(), DxfError> {
    manager.delete_subtasks(task_id)
}

#[allow(non_snake_case)]
/// 任务是否正在取消。
pub fn IsTaskCancelling(manager: &dyn TaskTable, task_id: i64) -> Result<bool, DxfError> {
    manager.is_task_cancelling(task_id)
}

#[allow(non_snake_case)]
/// 打印子任务调试信息。
pub fn PrintSubtaskInfo(manager: &dyn TaskTable, task_id: i64) -> Result<(), DxfError> {
    // This is a diagnostic-only helper; Go ignores errors from both queries.
    let _ = manager.print_subtask_info(task_id);
    Ok(())
}

/// Mock 节点资源守卫：Drop 时恢复原资源。
pub struct NodeResourceGuard {
    runtime: Arc<dyn TableTestRuntime>,
    /// 进入守卫前的节点资源，用于恢复。
    previous: Option<NodeResource>,
}

impl Drop for NodeResourceGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            let _ = self.runtime.set_node_resource(previous);
        }
    }
}

#[allow(non_snake_case)]
/// 将节点资源 mock 为给定 CPU 数（内存/磁盘按比例推算）。
pub fn MockNodeResource(
    runtime: Arc<dyn TableTestRuntime>,
    cpu: usize,
) -> Result<NodeResourceGuard, DxfError> {
    // 内存按 CPU×2GiB，磁盘固定 100GiB，贴近测试常用配额。
    let previous = runtime.set_node_resource(NodeResource {
        cpu_count: cpu,
        memory_bytes: cpu as u64 * 2 * 1024 * 1024 * 1024,
        disk_bytes: 100 * 1024 * 1024 * 1024,
    })?;
    Ok(NodeResourceGuard {
        runtime,
        previous: Some(previous),
    })
}
