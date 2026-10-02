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

// 任务调度器（BaseScheduler）核心状态机。
//
// 每个 DXF 任务对应一个调度器实例。一次 `schedule_once` 只做一次持久化状态迁移：
// 刷新任务元数据，再按当前状态分发到 cancelling / pausing / resuming / reverting /
// pending / running / modifying 等处理函数。扩展点（Extension）负责类型相关的
// 准备、下一步规划、子任务批次生成与完成回调。
//
// 术语：Subtask 为任务某一步内的并行执行单元；Step 为任务顺序阶段；
// slot 预留由 Manager 注入的 `allocated_slots` 控制。

use crate::interface::*;
use crate::nodes::filter_by_scope;
use astersql_dxf_framework_dxfmetric::InitDistTaskMetrics;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 用户取消任务时写入的错误文案。
pub const TASK_CANCEL_MESSAGE: &str = "cancelled by user";
/// 持久化子任务时的 SQL 重试次数上限。
pub const RETRY_SQL_TIMES: usize = 30;
/// 默认事务总大小上限；子任务元数据总和接近该值的 80% 时改走批量写入。
const DEFAULT_TXN_TOTAL_SIZE_LIMIT: usize = 1024 * 1024 * 1024;

/// 基础调度器：一次 tick 完成一次可持久化的状态机迁移。
///
/// One scheduler tick performs a single durable state-machine transition. A
/// caller may drive it from a thread, timer, or deterministic test harness.
pub struct BaseScheduler {
    context: Context,
    /// 调度依赖：TaskManager、NodeManager、SlotManager 等。
    param: Param,
    /// 当前任务快照（加锁保护）。
    task: Mutex<Task>,
    /// 任务类型扩展点。
    extension: Arc<dyn Extension>,
    /// 关闭标志：为 true 时 `schedule_once` 直接返回完成。
    closed: AtomicBool,
}

impl BaseScheduler {
    /// 用任务、调度参数与扩展点构造基础调度器。
    pub fn new(task: Task, param: Param, extension: Arc<dyn Extension>) -> Self {
        Self {
            context: Context::new(),
            param,
            task: Mutex::new(task),
            extension,
            closed: AtomicBool::new(false),
        }
    }

    /// 用新快照替换本地任务缓存。
    fn replace_task(&self, task: Task) {
        *self.task.lock().expect("scheduler task lock poisoned") = task;
    }

    /// 若系统表中的状态或步骤变化，则重新加载完整任务。
    fn refresh_task_if_needed(&self) -> Result<()> {
        let current = self.task();
        let latest = self.param.task_manager.task_base_by_id(current.base.id)?;
        // 仅当状态或步骤变化时才拉取完整任务，减少系统表读。
        if latest.state != current.base.state || latest.step != current.base.step {
            self.replace_task(self.param.task_manager.task_by_id(current.base.id)?);
        }
        Ok(())
    }

    /// 处理 Cancelling：转入回滚，错误文案为用户取消。
    fn on_cancelling(&self) -> Result<bool> {
        self.revert_task(SchedulerError::new(TASK_CANCEL_MESSAGE))?;
        Ok(false)
    }

    /// 处理 Pausing：等待活跃子任务结束，必要时因 KV 磁盘满保持暂停中。
    fn on_pausing(&self) -> Result<bool> {
        let mut task = self.task();
        let counts = self
            .param
            .task_manager
            .subtask_count_by_states(task.base.id, task.base.step)?;
        let active = state_count(&counts, SUBTASK_STATE_RUNNING)
            + state_count(&counts, SUBTASK_STATE_PENDING);
        // 仍有 pending/running 子任务时继续等待。
        if active > 0 {
            return Ok(false);
        }
        let errors = if state_count(&counts, SUBTASK_STATE_FAILED) > 0
            || state_count(&counts, SUBTASK_STATE_CANCELED) > 0
        {
            self.param.task_manager.subtask_errors(task.base.id)?
        } else {
            Vec::new()
        };
        // KV 磁盘满自动暂停：保持 Pausing，不进入 Paused。
        if should_pause_on_kv_disk_full(&task, &counts, &errors) {
            let error = errors[0].clone();
            self.param.task_manager.pause_task_on_error(
                task.base.id,
                task.base.state,
                task.base.step,
                error.clone(),
            )?;
            task.error = Some(error);
            self.replace_task(task);
            return Ok(false);
        }
        self.param.task_manager.paused_task(task.base.id)?;
        task.base.state = TASK_STATE_PAUSED;
        self.replace_task(task);
        Ok(false)
    }

    /// 处理 Resuming：无预留 slot 时退出；否则恢复子任务或切回 Running。
    fn on_resuming(&self) -> Result<bool> {
        // 未占用 slot 的调度器在 Resuming 时直接退出，由 Manager 重建并预留。
        if !self.param.allocated_slots {
            return Ok(true);
        }
        let mut task = self.task();
        let counts = self
            .param
            .task_manager
            .subtask_count_by_states(task.base.id, task.base.step)?;
        if state_count(&counts, SUBTASK_STATE_PAUSED) == 0 {
            self.param.task_manager.resumed_task(task.base.id)?;
            task.base.state = TASK_STATE_RUNNING;
            self.replace_task(task);
        } else {
            self.param.task_manager.resume_subtasks(task.base.id)?;
        }
        Ok(false)
    }

    /// 处理 Reverting：活跃子任务结束后回调 OnDone 并标记 Reverted。
    fn on_reverting(&self) -> Result<bool> {
        let mut task = self.task();
        let counts = self
            .param
            .task_manager
            .subtask_count_by_states(task.base.id, task.base.step)?;
        let active = state_count(&counts, SUBTASK_STATE_PENDING)
            + state_count(&counts, SUBTASK_STATE_RUNNING);
        if active == 0 {
            self.extension
                .on_done_with_context(&self.context, self, &mut task)?;
            self.param.task_manager.reverted_task(task.base.id)?;
            task.base.state = TASK_STATE_REVERTED;
            on_task_finished(task.base.state, task.error.as_ref());
            self.replace_task(task);
        } else {
            self.extension.on_tick_with_context(&self.context, &task);
        }
        Ok(false)
    }

    /// 处理 Pending：可选 Prepare，再切换到下一 Step。
    fn on_pending(&self) -> Result<bool> {
        if !self.param.allocated_slots {
            return Ok(true);
        }
        let mut task = self.task();
        // PrepareModeRequired：在 INIT 步先执行类型相关准备。
        if task.base.step == STEP_INIT
            && task.base.extra_params.prepare_mode == PREPARE_MODE_REQUIRED
        {
            if let Err(error) =
                self.extension
                    .on_prepare_with_context(&self.context, self, &mut task)
            {
                return self.handle_prepare_or_plan_error(error);
            }
            if !self
                .param
                .task_manager
                .switch_task_step_after_prepare(&task)?
            {
                return Ok(false);
            }
            task.base.step = STEP_PREPARED;
            self.replace_task(task);
        }
        self.switch_to_next_step()?;
        Ok(false)
    }

    /// 处理 Running：失败则回滚/人工恢复，成功则进下一步，否则 OnTick。
    fn on_running(&self) -> Result<bool> {
        if !self.param.allocated_slots {
            return Ok(true);
        }
        let task = self.task();
        let counts = self
            .param
            .task_manager
            .subtask_count_by_states(task.base.id, task.base.step)?;
        if state_count(&counts, SUBTASK_STATE_FAILED) > 0
            || state_count(&counts, SUBTASK_STATE_CANCELED) > 0
        {
            let errors = self.param.task_manager.subtask_errors(task.base.id)?;
            // 磁盘满：切到 Pausing 而非直接回滚。
            if should_pause_on_kv_disk_full(&task, &counts, &errors) {
                let mut paused = task;
                let error = errors[0].clone();
                self.param.task_manager.pause_task_on_error(
                    paused.base.id,
                    paused.base.state,
                    paused.base.step,
                    error.clone(),
                )?;
                paused.base.state = TASK_STATE_PAUSING;
                paused.error = Some(error);
                self.replace_task(paused);
                return Ok(false);
            }
            let error = errors.first().cloned().unwrap_or_else(|| {
                SchedulerError::new(format!(
                    "subtasks failed or canceled without error, task {}, step {}",
                    task.base.id, task.base.step
                ))
            });
            self.revert_or_manual_recover(error)?;
        } else if is_step_succeed(&counts) {
            self.switch_to_next_step()?;
        } else {
            self.extension.on_tick_with_context(&self.context, &task);
        }
        Ok(false)
    }

    /// 处理 Modifying：应用并发/节点数/元数据修改；返回是否需重建调度器。
    fn on_modifying(&self) -> Result<bool> {
        let mut task = self.task();
        let mut recreate = false;
        let mut metadata_changes = Vec::new();
        // 区分内置修改（并发/最大节点数）与交给 Extension 的元数据修改。
        for modification in task.modifications.clone() {
            match modification.kind.as_str() {
                "modify_concurrency" if task.base.required_slots != modification.to as i32 => {
                    task.base.required_slots = modification.to as i32;
                    recreate = true;
                }
                "modify_max_node_count" if modification.to > 0 => {
                    task.base.max_node_count = modification.to as i32;
                }
                "modify_concurrency" | "modify_max_node_count" => {}
                _ => metadata_changes.push(modification),
            }
        }
        if !metadata_changes.is_empty() {
            task.meta = self.extension.modify_meta(&task.meta, &metadata_changes)?;
        }
        self.param.task_manager.modified_task(&task)?;
        task.base.state = task.previous_state;
        task.modifications.clear();
        self.replace_task(task);
        Ok(recreate)
    }

    /// 规划并派发下一 Step 的子任务；若已 Done 则标记任务成功。
    fn switch_to_next_step(&self) -> Result<()> {
        let mut task = self.task();
        let next_step = self.extension.next_step(&task.base);
        // 无更多步骤：OnDone 后标记成功。
        if next_step == STEP_DONE {
            self.extension
                .on_done_with_context(&self.context, self, &mut task)?;
            self.param.task_manager.succeed_task(task.base.id)?;
            task.base.step = STEP_DONE;
            task.base.state = TASK_STATE_SUCCEED;
            on_task_finished(task.base.state, task.error.as_ref());
            self.replace_task(task);
            return Ok(());
        }

        let managed = self.param.node_manager.get_nodes();
        let mut eligible = self.extension.eligible_instances(&task)?;
        // 扩展未指定实例时，按任务 target_scope 过滤托管节点。
        if eligible.is_empty() {
            eligible = filter_by_scope(&managed, &task.base.target_scope);
        }
        if task.base.max_node_count > 0 && eligible.len() > task.base.max_node_count as usize {
            eligible.truncate(task.base.max_node_count as usize);
        }
        if eligible.is_empty() {
            return Err(SchedulerError::new(
                "no available TiDB node to dispatch subtasks",
            ));
        }
        let metas = match self.extension.on_next_subtasks_batch_with_context(
            &self.context,
            self,
            &mut task,
            &eligible,
            next_step,
        ) {
            Ok(metas) => metas,
            Err(error) => return self.handle_prepare_or_plan_error(error).map(|_| ()),
        };
        self.schedule_subtasks(&task, next_step, metas, eligible)?;
        task.base.step = next_step;
        task.base.state = TASK_STATE_RUNNING;
        self.replace_task(task);
        Ok(())
    }

    /// 按 eligible 节点轮询分配子任务并持久化（必要时批量、带重试）。
    fn schedule_subtasks(
        &self,
        task: &Task,
        step: Step,
        metas: Vec<Vec<u8>>,
        eligible: Vec<String>,
    ) -> Result<()> {
        self.param
            .slot_manager
            .update(&self.param.node_manager, self.param.task_manager.as_ref())?;
        let eligible = self
            .param
            .slot_manager
            .adjust_eligible_nodes(eligible, task.base.required_slots);
        let subtasks = metas
            .into_iter()
            .enumerate()
            .map(|(index, meta)| Subtask {
                base: SubtaskBase {
                    id: 0,
                    task_id: task.base.id,
                    step,
                    state: SUBTASK_STATE_PENDING,
                    exec_id: eligible[index % eligible.len()].clone(),
                    concurrency: task.base.required_slots,
                    ordinal: index as i32 + 1,
                },
                meta,
                error: None,
            })
            .collect::<Vec<_>>();
        let total_size = subtasks
            .iter()
            .map(|subtask| subtask.meta.len())
            .sum::<usize>();
        // 元数据过大时走批量接口，避免单事务超限。
        let use_batch = total_size >= (DEFAULT_TXN_TOTAL_SIZE_LIMIT as f64 * 0.8) as usize;
        let mut last_error = None;
        for _ in 0..RETRY_SQL_TIMES {
            let result = if use_batch {
                self.param.task_manager.switch_task_step_in_batch(
                    task,
                    TASK_STATE_RUNNING,
                    step,
                    &subtasks,
                )
            } else {
                self.param
                    .task_manager
                    .switch_task_step(task, TASK_STATE_RUNNING, step, &subtasks)
            };
            match result {
                Ok(()) => return Ok(()),
                // 不稳定子任务集合错误不可重试，立即返回。
                Err(error) if error.0.contains("unstable subtasks") => return Err(error),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| SchedulerError::new("failed to persist subtasks")))
    }

    /// Prepare/规划错误：可重试则上抛，否则转入回滚。
    fn handle_prepare_or_plan_error(&self, error: SchedulerError) -> Result<bool> {
        if self.extension.is_retryable_error(&error) {
            return Err(error);
        }
        self.revert_task(error)?;
        Ok(false)
    }

    /// 将任务标记为 Reverting 并记录错误。
    fn revert_task(&self, error: SchedulerError) -> Result<()> {
        let mut task = self.task();
        self.param
            .task_manager
            .revert_task(task.base.id, task.base.state, error.clone())?;
        task.base.state = TASK_STATE_REVERTING;
        task.error = Some(error);
        self.replace_task(task);
        Ok(())
    }

    /// 若启用人工恢复则进入 AwaitingResolution，否则直接回滚。
    fn revert_or_manual_recover(&self, error: SchedulerError) -> Result<()> {
        let mut task = self.task();
        // 人工恢复：保留中间状态供排查，而不是自动回滚。
        if task.base.extra_params.manual_recovery {
            self.param.task_manager.awaiting_resolve_task(
                task.base.id,
                task.base.state,
                error.clone(),
            )?;
            task.base.state = TASK_STATE_AWAITING_RESOLUTION;
            task.error = Some(error);
            self.replace_task(task);
            Ok(())
        } else {
            self.revert_task(error)
        }
    }
}

pub(crate) fn on_task_finished(state: TaskState, error: Option<&SchedulerError>) {
    let metric_state = match state {
        TASK_STATE_SUCCEED | TASK_STATE_FAILED => state.to_owned(),
        TASK_STATE_REVERTED if error.is_some_and(|error| error.0.contains(TASK_CANCEL_MESSAGE)) => {
            "cancelled".to_owned()
        }
        TASK_STATE_REVERTED if error.is_some_and(|error| is_data_error_for_metric(&error.0)) => {
            "data-error".to_owned()
        }
        TASK_STATE_REVERTED => TASK_STATE_FAILED.to_owned(),
        _ => String::new(),
    };
    if metric_state.is_empty() {
        return;
    }
    let counter = &InitDistTaskMetrics().FinishedTaskCounter;
    counter.with_label_values(&["all"]).inc();
    counter.with_label_values(&[&metric_state]).inc();
}

// Keep the metric's borrowed-string classification in step with Go's
// storage.ClassifyTaskError, which replaced the original scheduler helper.
fn is_data_error_for_metric(message: &str) -> bool {
    let import_data = message.contains("ErrEncodeKV")
        && (message.contains("Value conversion failed for column")
            || (message.contains("Check constraint '") && message.contains("' is violated"))
            || message.contains("Table has no partition for value"));
    let import_conflict = (message.contains("[executor:8167]")
        && message.contains("Duplicate key conflict found"))
        || (message.contains("ErrFoundDataConflictRecords")
            && message.contains("found data conflict records"))
        || (message.contains("ErrFoundIndexConflictRecords")
            && message.contains("found index conflict records"));
    import_data
        || import_conflict
        || (message.contains("[kv:1062]") && message.contains("Duplicate entry"))
}

/// 向扩展点暴露历史子任务元数据/摘要查询。
impl TaskHandle for BaseScheduler {
    /// 查询指定步骤的历史子任务元数据。
    fn previous_subtask_metas(&self, task_id: i64, step: Step) -> Result<Vec<Vec<u8>>> {
        self.param
            .task_manager
            .previous_subtask_metas(task_id, step)
    }

    /// 查询指定步骤的历史子任务摘要。
    fn previous_subtask_summaries(&self, task_id: i64, step: Step) -> Result<Vec<SubtaskSummary>> {
        self.param
            .task_manager
            .previous_subtask_summaries(task_id, step)
    }
}

/// Scheduler 接口实现：初始化、单步调度、关闭与任务访问。
impl Scheduler for BaseScheduler {
    /// 校验 keyspace 不含非法空字符。
    fn init(&self) -> Result<()> {
        if self.task().base.keyspace.contains('\0') {
            Err(SchedulerError::new("invalid task keyspace"))
        } else {
            Ok(())
        }
    }

    /// 执行一次状态机迁移；返回 `true` 表示调度器可回收。
    fn schedule_once(&self) -> Result<bool> {
        if self.closed.load(Ordering::Acquire) {
            return Ok(true);
        }
        self.refresh_task_if_needed()?;
        match self.task().base.state {
            TASK_STATE_CANCELLING => self.on_cancelling(),
            TASK_STATE_PAUSING => self.on_pausing(),
            TASK_STATE_PAUSED => Ok(true),
            TASK_STATE_RESUMING => self.on_resuming(),
            TASK_STATE_REVERTING => self.on_reverting(),
            TASK_STATE_PENDING => self.on_pending(),
            TASK_STATE_RUNNING => self.on_running(),
            TASK_STATE_MODIFYING => self.on_modifying(),
            TASK_STATE_SUCCEED | TASK_STATE_REVERTED | TASK_STATE_FAILED => Ok(true),
            _ => Ok(false),
        }
    }

    /// 标记关闭，后续 `schedule_once` 立即返回完成。
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.context.cancel();
    }

    /// 返回当前任务快照副本。
    fn task(&self) -> Task {
        self.task
            .lock()
            .expect("scheduler task lock poisoned")
            .clone()
    }

    /// 返回任务类型扩展点。
    fn extension(&self) -> Arc<dyn Extension> {
        Arc::clone(&self.extension)
    }
}

/// 读取某子任务状态的计数，缺失视为 0。
fn state_count(counts: &HashMap<SubtaskState, i64>, state: SubtaskState) -> i64 {
    counts.get(state).copied().unwrap_or_default()
}

/// 判断当前步骤是否已全部成功（空或仅有 Succeed）。
fn is_step_succeed(counts: &HashMap<SubtaskState, i64>) -> bool {
    counts.is_empty() || (counts.len() == 1 && counts.contains_key(SUBTASK_STATE_SUCCEED))
}

/// 是否应因 TiKV 磁盘满自动暂停：需开启开关、无 canceled、全部 failed 且错误均为 disk full。
fn should_pause_on_kv_disk_full(
    task: &Task,
    counts: &HashMap<SubtaskState, i64>,
    errors: &[SchedulerError],
) -> bool {
    task.base.extra_params.pause_on_kv_disk_full
        && state_count(counts, SUBTASK_STATE_CANCELED) == 0
        && !errors.is_empty()
        && state_count(counts, SUBTASK_STATE_FAILED) == errors.len() as i64
        && errors
            .iter()
            .all(|error| error.0.to_ascii_lowercase().contains("disk full"))
}

/// 判断错误是否为用户取消（文案含 [`TASK_CANCEL_MESSAGE`]）。
pub fn IsCancelledErr(error: &SchedulerError) -> bool {
    error.0.contains(TASK_CANCEL_MESSAGE)
}
