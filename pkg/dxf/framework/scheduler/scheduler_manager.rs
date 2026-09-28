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

// DXF 调度管理器（Scheduler Manager）。
//
// Owner 节点上的核心组件：负责挑选可调度任务、预留 slot、创建并驱动各任务的
// `Scheduler`、负载均衡（Balancer），以及完成后的清理（清理例程、迁入历史表、
// GC 子任务）。`tick` 对应 Go 调度循环体，不绑定具体异步运行时。
//
// 术语：slot/stripe 见 `slots` 模块；任务排名（rank）为 priority → create_time → id。

use crate::balancer::Balancer;
use crate::interface::*;
use crate::nodes::NodeManager;
use crate::slots::SlotManager;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

/// 默认可并发运行的调度器（任务）数量上限。
pub const DEFAULT_MAX_CONCURRENT_TASKS: usize = 16;
/// 可配置的最大并发任务数上界（防止配置过大耗尽资源）。
pub const MAX_CONCURRENT_TASKS_UPPER_BOUND: usize = 1000;

/// 进程内当前生效的最大并发任务数。
static MAX_CONCURRENT_TASKS: AtomicUsize = AtomicUsize::new(DEFAULT_MAX_CONCURRENT_TASKS);

/// 读取当前最大并发任务数。
pub fn max_concurrent_tasks() -> usize {
    MAX_CONCURRENT_TASKS.load(Ordering::Acquire)
}

/// 设置最大并发任务数；越界时返回错误。
pub fn set_max_concurrent_tasks(value: usize) -> Result<()> {
    if !(DEFAULT_MAX_CONCURRENT_TASKS..=MAX_CONCURRENT_TASKS_UPPER_BOUND).contains(&value) {
        return Err(SchedulerError::new(format!(
            "max_concurrent_task {value} is out of range [{DEFAULT_MAX_CONCURRENT_TASKS}, {MAX_CONCURRENT_TASKS_UPPER_BOUND}]"
        )));
    }
    MAX_CONCURRENT_TASKS.store(value, Ordering::Release);
    Ok(())
}

/// 正在运行的调度器条目：持有调度器实例及其资源预留信息。
struct RunningScheduler {
    /// 具体任务类型的调度器实现。
    scheduler: Arc<dyn Scheduler>,
    /// 启动时用于预留的任务快照。
    reservation: TaskBase,
    /// 是否在启动时占用了 slot。
    allocated_slots: bool,
    /// stripe 回退到单节点预留时的执行节点 ID；stripe 成功时为空。
    reserved_exec_id: String,
}

/// 调度管理器：负责任务选择、资源预留、执行驱动与清理。
///
/// Manager owns scheduler selection, resource reservation, execution and
/// cleanup. `tick` is the body of Go's schedule loop without prescribing a
/// particular async runtime.
pub struct Manager {
    /// 任务元数据读写接口。
    task_manager: Arc<dyn TaskManager>,
    /// 可调度执行节点集合。
    node_manager: Arc<NodeManager>,
    /// slot / stripe 资源管理。
    slot_manager: Arc<SlotManager>,
    /// 子任务负载均衡器。
    balancer: Mutex<Balancer>,
    /// 任务 ID → 正在运行的调度器。
    schedulers: RwLock<HashMap<i64, RunningScheduler>>,
    /// 本节点服务 ID。
    server_id: String,
    /// 本节点资源描述（可选）。
    node_resource: Option<NodeResource>,
    /// 是否已完成 start 初始化。
    initialized: AtomicBool,
}

impl Manager {
    /// 构造管理器并装配 NodeManager / SlotManager / Balancer。
    pub fn new(
        task_manager: Arc<dyn TaskManager>,
        server_id: impl Into<String>,
        node_resource: Option<NodeResource>,
    ) -> Self {
        let node_manager = Arc::new(NodeManager::new());
        let slot_manager = Arc::new(SlotManager::new());
        let param = Param {
            task_manager: Arc::clone(&task_manager),
            node_manager: Arc::clone(&node_manager),
            slot_manager: Arc::clone(&slot_manager),
            server_id: server_id.into(),
            allocated_slots: false,
            node_resource: node_resource.clone(),
        };
        Self {
            task_manager,
            node_manager,
            slot_manager,
            balancer: Mutex::new(Balancer::new(param.clone())),
            schedulers: RwLock::new(HashMap::new()),
            server_id: param.server_id,
            node_resource,
            initialized: AtomicBool::new(false),
        }
    }

    /// 刷新节点视图并标记为已初始化，开始接受 tick。
    pub fn start(&self) -> Result<()> {
        self.node_manager
            .refresh_nodes(self.task_manager.as_ref(), &self.slot_manager)?;
        self.initialized.store(true, Ordering::Release);
        Ok(())
    }

    /// 取消调度循环（仅清除 initialized 标志）。
    pub fn cancel(&self) {
        self.initialized.store(false, Ordering::Release);
    }

    /// 停止管理器：关闭所有调度器并释放已占用的 slot。
    pub fn stop(&self) {
        self.cancel();
        let running = {
            let mut schedulers = self.schedulers.write().expect("scheduler lock poisoned");
            schedulers
                .drain()
                .map(|(_, entry)| entry)
                .collect::<Vec<_>>()
        };
        for entry in running {
            entry.scheduler.close();
            if entry.allocated_slots {
                self.slot_manager
                    .unreserve(&entry.reservation, &entry.reserved_exec_id);
            }
        }
    }

    /// 是否已 start。
    pub fn initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    /// 当前运行中的调度器数量。
    pub fn scheduler_count(&self) -> usize {
        self.schedulers
            .read()
            .expect("scheduler lock poisoned")
            .len()
    }

    /// 返回按任务排名排序的调度器列表。
    pub fn schedulers(&self) -> Vec<Arc<dyn Scheduler>> {
        let mut schedulers = self
            .schedulers
            .read()
            .expect("scheduler lock poisoned")
            .values()
            .map(|entry| Arc::clone(&entry.scheduler))
            .collect::<Vec<_>>();
        schedulers.sort_by(|left, right| left.task().base.compare(&right.task().base));
        schedulers
    }

    /// 一次调度节拍：拉取可调度任务、启动新调度器、推进已有调度器。
    pub fn tick(&self) -> Result<()> {
        if !self.initialized() {
            return Ok(());
        }
        let schedulable = self.get_schedulable_tasks()?;
        self.start_schedulers(schedulable)?;
        self.drive_schedulers();
        Ok(())
    }

    /// 过滤出尚未有调度器、且任务类型已知的候选任务。
    fn get_schedulable_tasks(&self) -> Result<Vec<TaskBase>> {
        // 并发已满时只拉「不需要资源」的任务（取消/回滚等），以便快速响应。
        let tasks = if self.scheduler_count() >= max_concurrent_tasks() {
            self.task_manager.top_no_need_resource_tasks()?
        } else {
            self.task_manager.top_unfinished_tasks()?
        };
        let running = self.schedulers.read().expect("scheduler lock poisoned");
        let mut schedulable = Vec::with_capacity(tasks.len());
        for task in tasks {
            if running.contains_key(&task.id) {
                continue;
            }
            // 未知任务类型：直接标记失败，避免卡在队列中。
            if get_scheduler_factory(&task.task_type).is_none() {
                self.task_manager.fail_task(
                    task.id,
                    task.state,
                    SchedulerError::new("unknown task type"),
                )?;
                continue;
            }
            schedulable.push(task);
        }
        Ok(schedulable)
    }

    /// 为候选任务尝试预留资源并启动调度器。
    fn start_schedulers(&self, tasks: Vec<TaskBase>) -> Result<()> {
        if tasks.is_empty() {
            return Ok(());
        }
        self.slot_manager
            .update(&self.node_manager, self.task_manager.as_ref())?;
        for basic_task in tasks {
            // 仅 Pending/Running/Resuming 需要占用 slot；取消/回滚/暂停等不占资源。
            let allocate_slots = matches!(
                basic_task.state,
                TASK_STATE_PENDING | TASK_STATE_RUNNING | TASK_STATE_RESUMING
            );
            let reserved_exec_id = if allocate_slots {
                // 启动前再次检查并发上限。
                if self.scheduler_count() >= max_concurrent_tasks() {
                    continue;
                }
                let (exec_id, available) = self.slot_manager.can_reserve(&basic_task);
                if !available {
                    continue;
                }
                exec_id
            } else {
                String::new()
            };
            self.start_scheduler(basic_task, allocate_slots, reserved_exec_id)?;
        }
        Ok(())
    }

    /// 创建单个调度器：工厂构造、init、登记预留并加入运行表。
    fn start_scheduler(
        &self,
        basic_task: TaskBase,
        allocated_slots: bool,
        reserved_exec_id: String,
    ) -> Result<()> {
        let task = self.task_manager.task_by_id(basic_task.id)?;
        let Some(factory) = get_scheduler_factory(&task.base.task_type) else {
            return Err(SchedulerError::new("unknown task type"));
        };
        let scheduler = factory(
            task.clone(),
            Param {
                task_manager: Arc::clone(&self.task_manager),
                node_manager: Arc::clone(&self.node_manager),
                slot_manager: Arc::clone(&self.slot_manager),
                server_id: self.server_id.clone(),
                allocated_slots,
                node_resource: self.node_resource.clone(),
            },
        );
        // init 失败视为致命：标记任务失败，不登记到运行表。
        if let Err(error) = scheduler.init() {
            self.task_manager
                .fail_task(task.base.id, task.base.state, error)?;
            return Ok(());
        }
        if allocated_slots {
            self.slot_manager.reserve(&basic_task, &reserved_exec_id);
        }
        self.schedulers
            .write()
            .expect("scheduler lock poisoned")
            .insert(
                basic_task.id,
                RunningScheduler {
                    scheduler,
                    reservation: basic_task,
                    allocated_slots,
                    reserved_exec_id,
                },
            );
        Ok(())
    }

    /// 对每个调度器调用 `schedule_once`；完成后关闭并释放资源。
    fn drive_schedulers(&self) {
        let schedulers = self.schedulers();
        let mut finished = Vec::new();
        for scheduler in schedulers {
            let task_id = scheduler.task().base.id;
            match scheduler.schedule_once() {
                Ok(true) => finished.push(task_id),
                Ok(false) => {}
                // Go retains a scheduler after a retryable scheduling error.
                // 可重试错误时保留调度器，下一轮 tick 再试。
                Err(_) => {}
            }
        }
        for task_id in finished {
            let entry = self
                .schedulers
                .write()
                .expect("scheduler lock poisoned")
                .remove(&task_id);
            if let Some(entry) = entry {
                entry.scheduler.close();
                if entry.allocated_slots {
                    self.slot_manager
                        .unreserve(&entry.reservation, &entry.reserved_exec_id);
                }
            }
        }
    }

    /// 按当前调度器列表执行一轮负载均衡。
    pub fn balance(&self) -> Result<()> {
        self.balancer
            .lock()
            .expect("balancer lock poisoned")
            .balance(&self.schedulers())
    }

    /// 对终态任务执行类型相关清理，再批量迁入历史表。
    pub fn cleanup_finished_tasks(&self) -> Result<usize> {
        let mut tasks = self.task_manager.tasks_in_states(&[
            TASK_STATE_FAILED,
            TASK_STATE_REVERTED,
            TASK_STATE_SUCCEED,
        ])?;
        let mut cleaned = Vec::new();
        for task in &mut tasks {
            if let Some(factory) = get_scheduler_cleanup_factory(&task.base.task_type) {
                // Stop on the first cleanup failure. Already-cleaned tasks are
                // still transferred, which is Go's retry-safe prefix behavior.
                // 遇第一个清理失败即停止；已清理前缀仍迁历史，便于重试。
                if factory().clean_up(task).is_err() {
                    break;
                }
            }
            cleaned.push(task.clone());
        }
        self.task_manager.transfer_tasks_to_history(&cleaned)?;
        Ok(cleaned.len())
    }

    /// 触发子任务垃圾回收。
    pub fn gc_subtasks(&self) -> Result<()> {
        self.task_manager.gc_subtasks()
    }

    /// 采集任务/子任务/正在调度任务数量快照。
    pub fn collect(&self) -> Result<MetricsSnapshot> {
        let tasks = self.task_manager.all_tasks()?;
        let subtasks = self.task_manager.all_subtasks()?;
        let scheduled_tasks = tasks
            .iter()
            .filter(|task| matches!(task.state, TASK_STATE_RUNNING | TASK_STATE_MODIFYING))
            .count();
        Ok(MetricsSnapshot {
            tasks: tasks.len(),
            subtasks: subtasks.len(),
            scheduled_tasks,
        })
    }

    /// 返回节点管理器。
    pub fn node_manager(&self) -> Arc<NodeManager> {
        Arc::clone(&self.node_manager)
    }

    /// 返回槽位管理器。
    pub fn slot_manager(&self) -> Arc<SlotManager> {
        Arc::clone(&self.slot_manager)
    }
}

/// 析构时自动 stop，避免泄漏运行中的调度器。
impl Drop for Manager {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 调度相关指标快照。
pub struct MetricsSnapshot {
    /// 全部任务数。
    pub tasks: usize,
    /// 全部子任务数。
    pub subtasks: usize,
    /// 处于 Running / Modifying 的任务数。
    pub scheduled_tasks: usize,
}

/// Go 风格导出：读取最大并发任务数。
pub fn GetMaxConcurrentTask() -> usize {
    max_concurrent_tasks()
}

/// Go 风格导出：设置最大并发任务数。
pub fn SetMaxConcurrentTask(value: usize) -> Result<()> {
    set_max_concurrent_tasks(value)
}
