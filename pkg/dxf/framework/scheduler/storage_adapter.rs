// Copyright 2026 AsterSQL.

//! Connect the durable DXF storage manager to the scheduler state machine.

use crate::interface::*;
use astersql_dxf_framework_storage as storage;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;
use storage::proto;

static TYPES: LazyLock<Mutex<HashMap<String, &'static str>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
fn task_type(value: &str) -> &'static str {
    let mut types = TYPES.lock().expect("task type interner poisoned");
    *types
        .entry(value.to_owned())
        .or_insert_with(|| Box::leak(value.to_owned().into_boxed_str()))
}
fn error(error: storage::Error) -> SchedulerError {
    SchedulerError::new(error.to_string())
}
fn context() -> storage::Context {
    storage::Context::default()
}

fn from_base(value: proto::TaskBase) -> TaskBase {
    TaskBase {
        id: value.ID,
        key: value.Key,
        task_type: value.Type.to_owned(),
        state: value.State,
        step: value.Step,
        priority: value.Priority,
        required_slots: value.RequiredSlots,
        target_scope: value.TargetScope,
        create_time: value.CreateTime,
        max_node_count: value.MaxNodeCount,
        extra_params: ExtraParams {
            manual_recovery: value.ExtraParams.ManualRecovery,
            pause_on_kv_disk_full: value.ExtraParams.PauseOnKVDiskFull,
            max_runtime_slots: value.ExtraParams.MaxRuntimeSlots,
            target_steps: value.ExtraParams.TargetSteps,
            prepare_mode: value.ExtraParams.PrepareMode,
        },
        keyspace: value.Keyspace,
    }
}
fn to_base(value: &TaskBase) -> proto::TaskBase {
    proto::TaskBase {
        ID: value.id,
        Key: value.key.clone(),
        Type: task_type(&value.task_type),
        State: value.state,
        Step: value.step,
        Priority: value.priority,
        RequiredSlots: value.required_slots,
        TargetScope: value.target_scope.clone(),
        CreateTime: value.create_time,
        MaxNodeCount: value.max_node_count,
        ExtraParams: proto::ExtraParams {
            ManualRecovery: value.extra_params.manual_recovery,
            PauseOnKVDiskFull: value.extra_params.pause_on_kv_disk_full,
            MaxRuntimeSlots: value.extra_params.max_runtime_slots,
            TargetSteps: value.extra_params.target_steps.clone(),
            PrepareMode: value.extra_params.prepare_mode,
        },
        Keyspace: value.keyspace.clone(),
    }
}
fn from_task(value: proto::Task) -> Task {
    Task {
        base: from_base(value.TaskBase),
        meta: value.Meta,
        error: value.Error.map(SchedulerError::new),
        previous_state: value.ModifyParam.PrevState,
        modifications: value
            .ModifyParam
            .Modifications
            .into_iter()
            .map(|change| Modification {
                kind: change.Type.to_owned(),
                to: change.To,
            })
            .collect(),
    }
}
fn to_task(value: &Task) -> proto::Task {
    proto::Task {
        TaskBase: to_base(&value.base),
        SchedulerID: String::new(),
        StartTime: SystemTime::UNIX_EPOCH,
        StateUpdateTime: SystemTime::UNIX_EPOCH,
        Meta: value.meta.clone(),
        Error: value.error.as_ref().map(|error| error.0.clone()),
        ModifyParam: proto::ModifyParam {
            PrevState: value.previous_state,
            Modifications: value
                .modifications
                .iter()
                .map(|change| proto::Modification {
                    Type: task_type(&change.kind),
                    To: change.to,
                })
                .collect(),
        },
    }
}
fn from_subtask_base(value: proto::SubtaskBase) -> SubtaskBase {
    SubtaskBase {
        id: value.ID,
        task_id: value.TaskID,
        step: value.Step,
        state: value.State,
        exec_id: value.ExecID,
        concurrency: value.Concurrency,
        ordinal: value.Ordinal,
    }
}
fn to_subtask_base(value: &SubtaskBase, parent_type: &str) -> proto::SubtaskBase {
    proto::SubtaskBase {
        ID: value.id,
        Step: value.step,
        Type: task_type(parent_type),
        TaskID: value.task_id,
        State: value.state,
        ExecID: value.exec_id.clone(),
        CreateTime: SystemTime::UNIX_EPOCH,
        StartTime: SystemTime::UNIX_EPOCH,
        Concurrency: value.concurrency,
        Ordinal: value.ordinal,
    }
}
fn to_subtask(value: &Subtask, parent_type: &str) -> proto::Subtask {
    proto::Subtask {
        SubtaskBase: to_subtask_base(&value.base, parent_type),
        UpdateTime: SystemTime::UNIX_EPOCH,
        Meta: value.meta.clone(),
        Summary: String::new(),
    }
}

#[derive(Clone)]
pub struct StorageTaskManagerAdapter {
    pub manager: storage::TaskManager,
}
impl StorageTaskManagerAdapter {
    pub fn new(manager: storage::TaskManager) -> Self {
        Self { manager }
    }
}

impl TaskManager for StorageTaskManagerAdapter {
    fn top_unfinished_tasks(&self) -> Result<Vec<TaskBase>> {
        self.manager
            .GetTopUnfinishedTasks(context())
            .map(|items| items.into_iter().map(from_base).collect())
            .map_err(error)
    }
    fn top_no_need_resource_tasks(&self) -> Result<Vec<TaskBase>> {
        self.manager
            .GetTopNoNeedResourceTasks(context())
            .map(|items| items.into_iter().map(from_base).collect())
            .map_err(error)
    }
    fn all_tasks(&self) -> Result<Vec<TaskBase>> {
        self.manager
            .GetAllTasks(context())
            .map(|items| {
                items
                    .unwrap_or_default()
                    .into_iter()
                    .map(from_base)
                    .collect()
            })
            .map_err(error)
    }
    fn all_subtasks(&self) -> Result<Vec<SubtaskBase>> {
        self.manager
            .GetAllSubtasks(context())
            .map(|items| {
                items
                    .unwrap_or_default()
                    .into_iter()
                    .map(from_subtask_base)
                    .collect()
            })
            .map_err(error)
    }
    fn tasks_in_states(&self, states: &[TaskState]) -> Result<Vec<Task>> {
        self.manager
            .GetTasksInStates(
                context(),
                states
                    .iter()
                    .map(|state| storage::Value::String((*state).into()))
                    .collect(),
            )
            .map(|items| items.into_iter().map(from_task).collect())
            .map_err(error)
    }
    fn cleanup_tasks(&self) -> Result<Vec<Task>> {
        self.manager
            .GetCleanupTasks(context())
            .map(|items| items.into_iter().map(from_task).collect())
            .map_err(error)
    }
    fn task_by_id(&self, task_id: i64) -> Result<Task> {
        self.manager
            .GetTaskByID(context(), task_id)
            .map(from_task)
            .map_err(error)
    }
    fn task_base_by_id(&self, task_id: i64) -> Result<TaskBase> {
        self.manager
            .GetTaskBaseByID(context(), task_id)
            .map(from_base)
            .map_err(error)
    }
    fn all_nodes(&self) -> Result<Vec<ManagedNode>> {
        self.manager
            .GetAllNodes(context())
            .map(|items| {
                items
                    .into_iter()
                    .map(|node| ManagedNode {
                        id: node.ID,
                        role: node.Role,
                        cpu_count: node.CPUCount,
                    })
                    .collect()
            })
            .map_err(error)
    }
    fn delete_dead_nodes(&self, nodes: &[String]) -> Result<()> {
        self.manager
            .DeleteDeadNodes(context(), nodes.to_vec())
            .map_err(error)
    }
    fn transfer_tasks_to_history(&self, tasks: &[Task]) -> Result<()> {
        self.manager
            .TransferTasks2History(context(), tasks.iter().map(to_task).collect())
            .map_err(error)
    }
    fn gc_subtasks(&self) -> Result<()> {
        self.manager.GCSubtasks(context()).map_err(error)
    }
    fn fail_task(&self, task_id: i64, current: TaskState, cause: SchedulerError) -> Result<()> {
        self.manager
            .FailTask(
                context(),
                task_id,
                current,
                Some(storage::Error::new(cause.0)),
            )
            .map_err(error)
    }
    fn revert_task(&self, task_id: i64, current: TaskState, cause: SchedulerError) -> Result<()> {
        self.manager
            .RevertTask(
                context(),
                task_id,
                current,
                Some(storage::Error::new(cause.0)),
            )
            .map_err(error)
    }
    fn awaiting_resolve_task(
        &self,
        task_id: i64,
        current: TaskState,
        cause: SchedulerError,
    ) -> Result<()> {
        self.manager
            .AwaitingResolveTask(
                context(),
                task_id,
                current,
                Some(storage::Error::new(cause.0)),
            )
            .map_err(error)
    }
    fn reverted_task(&self, task_id: i64) -> Result<()> {
        self.manager.RevertedTask(context(), task_id).map_err(error)
    }
    fn paused_task(&self, task_id: i64) -> Result<()> {
        self.manager.PausedTask(context(), task_id).map_err(error)
    }
    fn pause_task_on_error(
        &self,
        task_id: i64,
        current: TaskState,
        step: Step,
        cause: SchedulerError,
    ) -> Result<()> {
        self.manager
            .PauseTaskOnError(
                context(),
                task_id,
                current,
                step,
                Some(storage::Error::new(cause.0)),
            )
            .map_err(error)
    }
    fn resumed_task(&self, task_id: i64) -> Result<()> {
        self.manager.ResumedTask(context(), task_id).map_err(error)
    }
    fn modified_task(&self, task: &Task) -> Result<()> {
        self.manager
            .ModifiedTask(context(), to_task(task))
            .map_err(error)
    }
    fn succeed_task(&self, task_id: i64) -> Result<()> {
        self.manager.SucceedTask(context(), task_id).map_err(error)
    }
    fn switch_task_step(
        &self,
        task: &Task,
        next_state: TaskState,
        next_step: Step,
        subtasks: &[Subtask],
    ) -> Result<()> {
        self.manager
            .SwitchTaskStep(
                context(),
                to_task(task),
                next_state,
                next_step,
                subtasks
                    .iter()
                    .map(|subtask| to_subtask(subtask, &task.base.task_type))
                    .collect(),
            )
            .map_err(error)
    }
    fn switch_task_step_in_batch(
        &self,
        task: &Task,
        next_state: TaskState,
        next_step: Step,
        subtasks: &[Subtask],
    ) -> Result<()> {
        self.manager
            .SwitchTaskStepInBatch(
                context(),
                to_task(task),
                next_state,
                next_step,
                subtasks
                    .iter()
                    .map(|subtask| to_subtask(subtask, &task.base.task_type))
                    .collect(),
            )
            .map_err(error)
    }
    fn switch_task_step_after_prepare(&self, task: &Task) -> Result<bool> {
        self.manager
            .SwitchTaskStepAfterPrepare(context(), to_task(task))
            .map_err(error)
    }
    fn used_slots_on_nodes(&self) -> Result<HashMap<String, i32>> {
        self.manager.GetUsedSlotsOnNodes(context()).map_err(error)
    }
    fn active_subtasks(&self, task_id: i64) -> Result<Vec<SubtaskBase>> {
        self.manager
            .GetActiveSubtasks(context(), task_id)
            .map(|items| items.into_iter().map(from_subtask_base).collect())
            .map_err(error)
    }
    fn subtask_count_by_states(
        &self,
        task_id: i64,
        step: Step,
    ) -> Result<HashMap<SubtaskState, i64>> {
        self.manager
            .GetSubtaskCntGroupByStates(context(), task_id, step)
            .map_err(error)
    }
    fn subtask_errors(&self, task_id: i64) -> Result<Vec<SchedulerError>> {
        self.manager
            .GetSubtaskErrors(context(), task_id)
            .map(|items| {
                items
                    .into_iter()
                    .map(|item| {
                        SchedulerError::new(item.map_or(String::new(), |error| error.to_string()))
                    })
                    .collect()
            })
            .map_err(error)
    }
    fn resume_subtasks(&self, task_id: i64) -> Result<()> {
        self.manager
            .ResumeSubtasks(context(), task_id)
            .map_err(error)
    }
    fn update_subtask_exec_ids(&self, subtasks: &[SubtaskBase]) -> Result<()> {
        self.manager
            .UpdateSubtasksExecIDs(
                context(),
                subtasks
                    .iter()
                    .map(|item| to_subtask_base(item, ""))
                    .collect(),
            )
            .map_err(error)
    }
    fn previous_subtask_metas(&self, task_id: i64, step: Step) -> Result<Vec<Vec<u8>>> {
        self.manager
            .GetAllSubtasksByStepAndState(context(), task_id, step, proto::SubtaskStateSucceed)
            .map(|items| {
                items
                    .unwrap_or_default()
                    .into_iter()
                    .map(|item| item.Meta)
                    .collect()
            })
            .map_err(error)
    }
    fn previous_subtask_summaries(&self, task_id: i64, step: Step) -> Result<Vec<SubtaskSummary>> {
        self.manager
            .GetAllSubtaskSummaryByStep(context(), task_id, step)
            .map(|items| {
                items
                    .unwrap_or_default()
                    .into_iter()
                    .map(|item| SubtaskSummary {
                        row_count: item.RowCount,
                    })
                    .collect()
            })
            .map_err(error)
    }
}
