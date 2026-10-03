// Copyright 2026 AsterSQL.

// 调度器测试共用的内存桩与装配辅助函数。
//
// `TestTaskManager` 用可直接预置、检查的容器模拟持久化层，`TestExtension` 则通过
// 字段脚本化扩展回调的返回值和错误。两者共同让测试聚焦调度状态迁移，而不依赖真实存储或节点。

use crate::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
/// `TaskManager` 的线程安全内存实现，同时保留副作用记录供测试断言。
pub(super) struct TestTaskManager {
    /// 测试可预置的任务、候选任务、节点与子任务视图。
    pub tasks: Mutex<HashMap<i64, Task>>,
    pub top_unfinished: Mutex<Vec<TaskBase>>,
    pub top_no_resource: Mutex<Vec<TaskBase>>,
    pub nodes: Mutex<Vec<ManagedNode>>,
    pub subtasks: Mutex<Vec<SubtaskBase>>,
    pub active_subtasks: Mutex<HashMap<i64, Vec<SubtaskBase>>>,
    /// 按任务和步骤预置的子任务统计及错误。
    pub state_counts: Mutex<HashMap<(i64, Step), HashMap<SubtaskState, i64>>>,
    pub task_errors: Mutex<HashMap<i64, Vec<SchedulerError>>>,
    pub used_slots: Mutex<HashMap<String, i32>>,
    /// 记录调度器请求的写操作，便于测试同时核对调用参数和最终内存状态。
    pub updated_subtasks: Mutex<Vec<SubtaskBase>>,
    pub persisted_subtasks: Mutex<Vec<Subtask>>,
    pub deleted_nodes: Mutex<Vec<String>>,
    pub transferred_tasks: Mutex<Vec<Task>>,
    pub transfer_error: Mutex<Option<SchedulerError>>,
    pub cleanup_error: Mutex<Option<SchedulerError>>,
    pub cleanup_reads: AtomicUsize,
    pub failed_tasks: Mutex<Vec<(i64, TaskState, SchedulerError)>>,
    /// 规划下一批子任务时返回的上一阶段结果。
    pub previous_metas: Mutex<Vec<Vec<u8>>>,
    pub previous_summaries: Mutex<Vec<SubtaskSummary>>,
    /// 控制 prepare 后是否立即切换步骤，并统计垃圾回收调用次数。
    pub switch_after_prepare: AtomicBool,
    pub gc_calls: AtomicUsize,
}

impl TestTaskManager {
    pub fn insert_task(&self, task: Task) {
        self.tasks
            .lock()
            .expect("tasks lock poisoned")
            .insert(task.base.id, task);
    }

    pub fn task(&self, task_id: i64) -> Task {
        self.tasks
            .lock()
            .expect("tasks lock poisoned")
            .get(&task_id)
            .cloned()
            .expect("test task must exist")
    }

    fn change_state(&self, task_id: i64, state: TaskState) -> Result<()> {
        let mut tasks = self.tasks.lock().expect("tasks lock poisoned");
        let task = tasks
            .get_mut(&task_id)
            .ok_or_else(|| SchedulerError::new(format!("task {task_id} not found")))?;
        task.base.state = state;
        Ok(())
    }
}

impl TaskManager for TestTaskManager {
    fn cleanup_tasks(&self) -> Result<Vec<Task>> {
        self.cleanup_reads.fetch_add(1, Ordering::AcqRel);
        if let Some(error) = self.cleanup_error.lock().unwrap().clone() {
            return Err(error);
        }
        let mut tasks =
            self.tasks_in_states(&[TASK_STATE_FAILED, TASK_STATE_REVERTED, TASK_STATE_SUCCEED])?;
        tasks.truncate(crate::proto::GetTaskCleanupBatchSize() as usize);
        Ok(tasks)
    }

    fn top_unfinished_tasks(&self) -> Result<Vec<TaskBase>> {
        Ok(self
            .top_unfinished
            .lock()
            .expect("top-unfinished lock poisoned")
            .clone())
    }

    fn top_no_need_resource_tasks(&self) -> Result<Vec<TaskBase>> {
        Ok(self
            .top_no_resource
            .lock()
            .expect("top-no-resource lock poisoned")
            .clone())
    }

    fn all_tasks(&self) -> Result<Vec<TaskBase>> {
        Ok(self
            .tasks
            .lock()
            .expect("tasks lock poisoned")
            .values()
            .map(|task| task.base.clone())
            .collect())
    }

    fn all_subtasks(&self) -> Result<Vec<SubtaskBase>> {
        Ok(self
            .subtasks
            .lock()
            .expect("subtasks lock poisoned")
            .clone())
    }

    fn tasks_in_states(&self, states: &[TaskState]) -> Result<Vec<Task>> {
        let mut tasks: Vec<_> = self
            .tasks
            .lock()
            .expect("tasks lock poisoned")
            .values()
            .filter(|task| states.contains(&task.base.state))
            .cloned()
            .collect();
        tasks.sort_by_key(|task| task.base.id);
        Ok(tasks)
    }

    fn task_by_id(&self, task_id: i64) -> Result<Task> {
        self.tasks
            .lock()
            .expect("tasks lock poisoned")
            .get(&task_id)
            .cloned()
            .ok_or_else(|| SchedulerError::new(format!("task {task_id} not found")))
    }

    fn task_base_by_id(&self, task_id: i64) -> Result<TaskBase> {
        self.task_by_id(task_id).map(|task| task.base)
    }

    fn all_nodes(&self) -> Result<Vec<ManagedNode>> {
        Ok(self.nodes.lock().expect("nodes lock poisoned").clone())
    }

    fn delete_dead_nodes(&self, nodes: &[String]) -> Result<()> {
        self.deleted_nodes
            .lock()
            .expect("deleted-nodes lock poisoned")
            .extend_from_slice(nodes);
        self.nodes
            .lock()
            .expect("nodes lock poisoned")
            .retain(|node| !nodes.contains(&node.id));
        Ok(())
    }

    fn transfer_tasks_to_history(&self, tasks: &[Task]) -> Result<()> {
        if let Some(error) = self.transfer_error.lock().unwrap().clone() {
            return Err(error);
        }
        self.transferred_tasks
            .lock()
            .expect("transferred-tasks lock poisoned")
            .extend_from_slice(tasks);
        self.tasks
            .lock()
            .unwrap()
            .retain(|id, _| !tasks.iter().any(|task| task.base.id == *id));
        Ok(())
    }

    fn gc_subtasks(&self) -> Result<()> {
        self.gc_calls.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    fn fail_task(&self, task_id: i64, current: TaskState, error: SchedulerError) -> Result<()> {
        self.failed_tasks
            .lock()
            .expect("failed-tasks lock poisoned")
            .push((task_id, current, error.clone()));
        self.change_state(task_id, TASK_STATE_FAILED)?;
        self.tasks
            .lock()
            .expect("tasks lock poisoned")
            .get_mut(&task_id)
            .expect("task must exist")
            .error = Some(error);
        Ok(())
    }

    fn revert_task(&self, task_id: i64, _current: TaskState, error: SchedulerError) -> Result<()> {
        self.change_state(task_id, TASK_STATE_REVERTING)?;
        self.tasks
            .lock()
            .expect("tasks lock poisoned")
            .get_mut(&task_id)
            .expect("task must exist")
            .error = Some(error);
        Ok(())
    }

    fn awaiting_resolve_task(
        &self,
        task_id: i64,
        _current: TaskState,
        error: SchedulerError,
    ) -> Result<()> {
        self.change_state(task_id, TASK_STATE_AWAITING_RESOLUTION)?;
        self.tasks
            .lock()
            .expect("tasks lock poisoned")
            .get_mut(&task_id)
            .expect("task must exist")
            .error = Some(error);
        Ok(())
    }

    fn reverted_task(&self, task_id: i64) -> Result<()> {
        self.change_state(task_id, TASK_STATE_REVERTED)
    }

    fn paused_task(&self, task_id: i64) -> Result<()> {
        self.change_state(task_id, TASK_STATE_PAUSED)
    }

    fn pause_task_on_error(
        &self,
        task_id: i64,
        current: TaskState,
        step: Step,
        error: SchedulerError,
    ) -> Result<()> {
        {
            let mut tasks = self.tasks.lock().expect("tasks lock poisoned");
            let task = tasks
                .get_mut(&task_id)
                .ok_or_else(|| SchedulerError::new(format!("task {task_id} not found")))?;
            if task.base.state != current {
                return Err(SchedulerError::new("task changed"));
            }
            task.base.state = TASK_STATE_PAUSING;
            task.error = Some(error);
        }
        if let Some(subtasks) = self
            .active_subtasks
            .lock()
            .expect("active-subtasks lock poisoned")
            .get_mut(&task_id)
        {
            for subtask in subtasks {
                if subtask.step == step && subtask.state == SUBTASK_STATE_FAILED {
                    subtask.state = SUBTASK_STATE_PAUSED;
                }
            }
        }
        Ok(())
    }

    fn resumed_task(&self, task_id: i64) -> Result<()> {
        self.change_state(task_id, TASK_STATE_RUNNING)
    }

    fn modified_task(&self, task: &Task) -> Result<()> {
        self.insert_task(task.clone());
        Ok(())
    }

    fn succeed_task(&self, task_id: i64) -> Result<()> {
        self.change_state(task_id, TASK_STATE_SUCCEED)
    }

    fn switch_task_step(
        &self,
        task: &Task,
        next_state: TaskState,
        next_step: Step,
        subtasks: &[Subtask],
    ) -> Result<()> {
        // 同时保留新子任务和更新后的任务快照，模拟一次步骤切换写入的两个可观察结果。
        self.persisted_subtasks
            .lock()
            .expect("persisted-subtasks lock poisoned")
            .extend_from_slice(subtasks);
        let mut stored = task.clone();
        stored.base.state = next_state;
        stored.base.step = next_step;
        self.insert_task(stored);
        Ok(())
    }

    fn switch_task_step_in_batch(
        &self,
        task: &Task,
        next_state: TaskState,
        next_step: Step,
        subtasks: &[Subtask],
    ) -> Result<()> {
        self.switch_task_step(task, next_state, next_step, subtasks)
    }

    fn switch_task_step_after_prepare(&self, task: &Task) -> Result<bool> {
        // 该开关专门覆盖“准备完成后由存储层直接推进”的分支。
        if self.switch_after_prepare.load(Ordering::Acquire) {
            let mut stored = task.clone();
            stored.base.step = STEP_PREPARED;
            self.insert_task(stored);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn used_slots_on_nodes(&self) -> Result<HashMap<String, i32>> {
        Ok(self
            .used_slots
            .lock()
            .expect("used-slots lock poisoned")
            .clone())
    }

    fn active_subtasks(&self, task_id: i64) -> Result<Vec<SubtaskBase>> {
        Ok(self
            .active_subtasks
            .lock()
            .expect("active-subtasks lock poisoned")
            .get(&task_id)
            .cloned()
            .unwrap_or_default())
    }

    fn subtask_count_by_states(
        &self,
        task_id: i64,
        step: Step,
    ) -> Result<HashMap<SubtaskState, i64>> {
        Ok(self
            .state_counts
            .lock()
            .expect("state-counts lock poisoned")
            .get(&(task_id, step))
            .cloned()
            .unwrap_or_default())
    }

    fn subtask_errors(&self, task_id: i64) -> Result<Vec<SchedulerError>> {
        Ok(self
            .task_errors
            .lock()
            .expect("task-errors lock poisoned")
            .get(&task_id)
            .cloned()
            .unwrap_or_default())
    }

    fn resume_subtasks(&self, task_id: i64) -> Result<()> {
        // 恢复只重置暂停项，避免误改同一任务下已经运行或结束的子任务。
        if let Some(subtasks) = self
            .active_subtasks
            .lock()
            .expect("active-subtasks lock poisoned")
            .get_mut(&task_id)
        {
            for subtask in subtasks {
                if subtask.state == SUBTASK_STATE_PAUSED {
                    subtask.state = SUBTASK_STATE_PENDING;
                }
            }
        }
        Ok(())
    }

    fn update_subtask_exec_ids(&self, subtasks: &[SubtaskBase]) -> Result<()> {
        // 先记录原始更新请求，再同步活动视图，便于分别断言调用与读取结果。
        self.updated_subtasks
            .lock()
            .expect("updated-subtasks lock poisoned")
            .extend_from_slice(subtasks);
        let updates = subtasks
            .iter()
            .map(|subtask| (subtask.id, subtask.exec_id.clone()))
            .collect::<HashMap<_, _>>();
        for active in self
            .active_subtasks
            .lock()
            .expect("active-subtasks lock poisoned")
            .values_mut()
        {
            for subtask in active {
                if let Some(exec_id) = updates.get(&subtask.id) {
                    subtask.exec_id.clone_from(exec_id);
                }
            }
        }
        Ok(())
    }

    fn previous_subtask_metas(&self, _task_id: i64, _step: Step) -> Result<Vec<Vec<u8>>> {
        Ok(self
            .previous_metas
            .lock()
            .expect("previous-metas lock poisoned")
            .clone())
    }

    fn previous_subtask_summaries(
        &self,
        _task_id: i64,
        _step: Step,
    ) -> Result<Vec<SubtaskSummary>> {
        Ok(self
            .previous_summaries
            .lock()
            .expect("previous-summaries lock poisoned")
            .clone())
    }
}

#[derive(Default)]
/// 可脚本化的调度扩展：预置规划结果、一次性错误以及生命周期回调计数。
pub(super) struct TestExtension {
    /// 正常路径下的步骤、候选执行节点和子任务元数据。
    pub next_step: AtomicI64,
    pub eligible: Mutex<Vec<String>>,
    pub metas: Mutex<Vec<Vec<u8>>>,
    /// 对应回调的单次错误注入；回调读取后即清空。
    pub prepare_error: Mutex<Option<SchedulerError>>,
    pub plan_error: Mutex<Option<SchedulerError>>,
    pub done_error: Mutex<Option<SchedulerError>>,
    /// 控制错误分类，并记录各生命周期回调的调用次数。
    pub retryable: AtomicBool,
    pub tick_calls: AtomicUsize,
    pub prepare_calls: AtomicUsize,
    pub done_calls: AtomicUsize,
}

impl Extension for TestExtension {
    fn on_tick(&self, _task: &Task) {
        self.tick_calls.fetch_add(1, Ordering::AcqRel);
    }

    fn on_next_subtasks_batch(
        &self,
        _handle: &dyn TaskHandle,
        _task: &mut Task,
        _exec_ids: &[String],
        _next_step: Step,
    ) -> Result<Vec<Vec<u8>>> {
        // `take` 让错误只影响下一次规划，后续调用自动回到正常脚本结果。
        if let Some(error) = self
            .plan_error
            .lock()
            .expect("plan-error lock poisoned")
            .take()
        {
            return Err(error);
        }
        Ok(self.metas.lock().expect("metas lock poisoned").clone())
    }

    fn on_done(&self, _handle: &dyn TaskHandle, _task: &mut Task) -> Result<()> {
        self.done_calls.fetch_add(1, Ordering::AcqRel);
        if let Some(error) = self
            .done_error
            .lock()
            .expect("done-error lock poisoned")
            .take()
        {
            Err(error)
        } else {
            Ok(())
        }
    }

    fn eligible_instances(&self, _task: &Task) -> Result<Vec<String>> {
        Ok(self
            .eligible
            .lock()
            .expect("eligible lock poisoned")
            .clone())
    }

    fn is_retryable_error(&self, _error: &SchedulerError) -> bool {
        self.retryable.load(Ordering::Acquire)
    }

    fn next_step(&self, _task: &TaskBase) -> Step {
        self.next_step.load(Ordering::Acquire)
    }

    fn on_prepare(&self, _handle: &dyn TaskHandle, task: &mut Task) -> Result<()> {
        self.prepare_calls.fetch_add(1, Ordering::AcqRel);
        if let Some(error) = self
            .prepare_error
            .lock()
            .expect("prepare-error lock poisoned")
            .take()
        {
            Err(error)
        } else {
            // 成功准备时留下稳定标记，供测试确认修改后的任务元数据被继续传递。
            task.meta.extend_from_slice(b"-prepared");
            Ok(())
        }
    }

    fn modify_meta(&self, old_meta: &[u8], modifications: &[Modification]) -> Result<Vec<u8>> {
        // 使用可读且确定的追加编码，使测试能精确核对修改顺序和内容。
        let mut meta = old_meta.to_vec();
        for modification in modifications {
            meta.extend_from_slice(
                format!(":{}={}", modification.kind, modification.to).as_bytes(),
            );
        }
        Ok(meta)
    }
}

/// 构造仅带指定编号和状态、默认需要一个槽位的测试任务。
pub(super) fn task(id: i64, state: TaskState) -> Task {
    Task {
        base: TaskBase {
            id,
            state,
            required_slots: 1,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// 用固定的单节点八槽位环境组装基础调度器。
///
/// `allocated_slots` 原样传入，以便同一套夹具覆盖已分配和待分配资源两条路径。
pub(super) fn scheduler(
    task: Task,
    manager: Arc<TestTaskManager>,
    extension: Arc<TestExtension>,
    allocated_slots: bool,
) -> BaseScheduler {
    manager.insert_task(task.clone());
    let node_manager = Arc::new(NodeManager::new());
    node_manager.set_nodes(vec![ManagedNode {
        id: "n1".to_owned(),
        role: String::new(),
        cpu_count: 8,
    }]);
    let slot_manager = Arc::new(SlotManager::new());
    slot_manager.update_capacity(8);
    slot_manager.set_used_slots(HashMap::from([("n1".to_owned(), 0)]));
    BaseScheduler::new(
        task,
        Param {
            task_manager: manager,
            node_manager,
            slot_manager,
            server_id: "test".to_owned(),
            allocated_slots,
            node_resource: None,
        },
        extension,
    )
}
