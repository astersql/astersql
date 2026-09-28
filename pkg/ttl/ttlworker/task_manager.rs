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

// TTL 扫描任务管理器：维护等待/运行/完成队列，并按容量调度扫描任务。
//
// 对应 Go 侧 TTL worker 的 task manager：节点认领任务、心跳续约、回收失效作业，
// 以及根据扫描结果更新任务状态。物理表 TTL 关闭时可通过心跳路径主动卸任。

use std::collections::{BTreeMap, VecDeque};

use crate::scan::{ScanResult, TaskTerminateReason, TtlScanTask};

/// 统计当前处于 `running` 状态的 TTL 任务数量的 SQL（系统表 `mysql.tidb_ttl_task`）。
pub const COUNT_RUNNING_TASKS_SQL: &str =
    "SELECT count(1) FROM mysql.tidb_ttl_task WHERE status = 'running'";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// TTL 扫描任务的生命周期状态。
pub enum TaskStatus {
    /// 已入队，等待本节点调度启动。
    Waiting,
    /// 本节点正在执行扫描/删除。
    Running,
    /// 正常结束。
    Finished,
    /// 兼容旧调用方保留的错误态；Go 的任务表不会将扫描错误写成此状态。
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
/// 任务执行过程中累计的行数统计。
pub struct TaskState {
    /// 已扫描行数。
    pub total_rows: u64,
    /// 成功删除/处理的行数。
    pub success_rows: u64,
    /// 出错行数。
    pub error_rows: u64,
}

#[derive(Clone, Debug)]
/// 被管理器跟踪的单个扫描任务及其归属信息。
pub struct ManagedTask {
    /// 底层扫描任务描述（作业 ID、扫描分片等）。
    pub task: TtlScanTask,
    /// 当前状态。
    pub status: TaskStatus,
    /// 认领该任务的节点 ID；未认领时为 `None`。
    pub owner_id: Option<String>,
    /// 归属心跳时间戳（秒或单调时钟，由调用方约定）。
    pub owner_heartbeat: u64,
    /// 运行期行数统计。
    pub state: TaskState,
}

#[derive(Clone, Debug)]
/// 单节点上的 TTL 任务调度器：限制并发运行数，并从等待队列提升任务。
pub struct TaskManager {
    /// 本节点标识，写入认领信息。
    pub owner_id: String,
    /// 允许同时运行的最大任务数（至少为 1）。
    pub max_running_tasks: usize,
    /// 等待调度的任务队列（FIFO）。
    waiting: VecDeque<ManagedTask>,
    /// 运行中任务，键为 `(job_id, scan_id)`。
    running: BTreeMap<(String, i64), ManagedTask>,
    /// 已结束（成功或错误）的任务列表。
    finished: Vec<ManagedTask>,
}

impl TaskManager {
    /// 创建管理器；`max_running_tasks` 会被钳制到至少 1。
    pub fn new(owner_id: impl Into<String>, max_running_tasks: usize) -> Self {
        Self {
            owner_id: owner_id.into(),
            max_running_tasks: max_running_tasks.max(1),
            waiting: VecDeque::new(),
            running: BTreeMap::new(),
            finished: Vec::new(),
        }
    }
    /// 将任务追加到等待队列末尾。
    pub fn push_waiting(&mut self, task: ManagedTask) {
        if task.status == TaskStatus::Waiting {
            self.waiting.push_back(task);
        }
    }
    /// 在容量允许时从等待队列提升任务到运行中，返回本次新调度的扫描任务副本。
    pub fn reschedule(&mut self, now: u64) -> Vec<TtlScanTask> {
        // 填满运行槽位：跳过已在 running 中的重复 (job_id, scan_id)。
        let mut scheduled = Vec::new();
        while self.running.len() < self.max_running_tasks {
            let Some(mut task) = self.waiting.pop_front() else {
                break;
            };
            if task.status != TaskStatus::Waiting {
                continue;
            }
            let key = (task.task.job_id.clone(), task.task.scan_id);
            if self.running.contains_key(&key) {
                continue;
            }
            task.status = TaskStatus::Running;
            task.owner_id = Some(self.owner_id.clone());
            task.owner_heartbeat = now;
            scheduled.push(task.task.clone());
            self.running.insert(key, task);
        }
        scheduled
    }
    /// 对运行中任务做心跳或卸任：`ttl_enabled` 为假时从 running 移除并返回其键。
    pub fn heartbeat_or_resign(
        &mut self,
        now: u64,
        ttl_enabled: impl Fn(&ManagedTask) -> bool,
    ) -> Vec<(String, i64)> {
        let mut resigned = Vec::new();
        let mut resigned_tasks = Vec::new();
        self.running.retain(|key, task| {
            if ttl_enabled(task) {
                task.owner_heartbeat = now;
                true
            } else {
                resigned.push(key.clone());
                task.status = TaskStatus::Waiting;
                task.owner_id = None;
                task.owner_heartbeat = 0;
                resigned_tasks.push(task.clone());
                false
            }
        });
        // Resignation makes a task available to another manager.  Dropping it
        // would lose the Go task row and prevent a later reschedule.
        for task in resigned_tasks {
            self.waiting.push_back(task);
        }
        resigned
    }
    /// 根据扫描结果将对应运行中任务移入 finished；找不到任务时返回 `false`。
    pub fn report_finished(&mut self, result: ScanResult) -> bool {
        let key = (result.job_id, result.scan_id);
        let Some(mut task) = self.running.remove(&key) else {
            return false;
        };
        // WorkerStop 与 Go 的 ReasonWorkerStop 相同：它不是业务失败，
        // 任务必须回到 waiting，避免缩容时丢失扫描任务。
        task.state.total_rows = result.scanned_rows;
        if result.reason == TaskTerminateReason::WorkerStop {
            task.status = TaskStatus::Waiting;
            task.owner_id = None;
            task.owner_heartbeat = 0;
            self.waiting.push_back(task);
        } else {
            // Go 的 reportTaskFinished 对所有非 WorkerStop 结果都将任务行更新为
            // finished；扫描错误属于完成状态中的诊断信息，不能成为一个额外的
            // 持久状态，否则 job manager 会永远等不到全部任务完成。
            task.status = TaskStatus::Finished;
            self.finished.push(task);
        }
        true
    }
    /// 丢弃作业 ID 不在 `valid_jobs` 中的等待/运行任务（作业已被取消或过期）。
    pub fn remove_invalid_jobs(&mut self, valid_jobs: &std::collections::BTreeSet<String>) {
        self.waiting
            .retain(|task| valid_jobs.contains(&task.task.job_id));
        self.running
            .retain(|(job_id, _), _| valid_jobs.contains(job_id));
    }
    /// 当前运行中任务数量。
    pub fn running_count(&self) -> usize {
        self.running.len()
    }
    /// 已结束任务切片视图。
    pub fn finished(&self) -> &[ManagedTask] {
        &self.finished
    }
}
