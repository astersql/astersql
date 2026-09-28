// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// DDL Job 调度器：在 owner（DDL Owner，集群中唯一负责推进 DDL 的节点）上
// 按 schema 依赖串行/并行投递 General 与 Reorg（重组）类任务到对应 worker。

use crate::SchemaLoader;
use crate::ddl::{Job, JobState};
use crate::ddl_running_jobs::{InvolvingSchemaInfo, RunningJobs};
use crate::job_worker::{JobContext, JobWorker};
use crate::schema_version::SchemaAction;
use std::collections::{HashSet, VecDeque};
use std::sync::RwLock;
use std::thread;
use std::time::Duration;

const JOB_RECORD_CAPACITY: usize = 16;
const JOB_ONCE_CAPACITY: usize = 1000;

/// Tracks jobs whose last schema version has not yet been synchronized.
///
/// Like Go's `unSyncedJobTracker`, duplicate additions are idempotent and
/// removing an absent ID is harmless.
#[derive(Debug)]
pub struct UnSyncedJobTracker {
    job_ids: RwLock<HashSet<i64>>,
    once_ids: RwLock<HashSet<i64>>,
}

impl Default for UnSyncedJobTracker {
    fn default() -> Self {
        Self {
            job_ids: RwLock::new(HashSet::with_capacity(JOB_RECORD_CAPACITY)),
            once_ids: RwLock::new(HashSet::with_capacity(JOB_ONCE_CAPACITY)),
        }
    }
}

impl UnSyncedJobTracker {
    /// Mark a job as waiting for schema synchronization.
    pub fn add_un_synced(&self, job_id: i64) {
        self.job_ids
            .write()
            .expect("un-synced job tracker poisoned")
            .insert(job_id);
    }

    /// Return whether a job is waiting for schema synchronization.
    pub fn is_un_synced(&self, job_id: i64) -> bool {
        self.job_ids
            .read()
            .expect("un-synced job tracker poisoned")
            .contains(&job_id)
    }

    /// Remove a job after its schema version has synchronized.
    pub fn remove_un_synced(&self, job_id: i64) {
        self.job_ids
            .write()
            .expect("un-synced job tracker poisoned")
            .remove(&job_id);
    }

    /// Return whether this owner may already have run the job once.
    pub fn maybe_already_run_once(&self, job_id: i64) -> bool {
        self.once_ids
            .read()
            .expect("job once tracker poisoned")
            .contains(&job_id)
    }

    /// Remember that this owner has run the job, bounding the hint map like Go.
    pub fn set_already_run_once(&self, job_id: i64) {
        let mut once_ids = self.once_ids.write().expect("job once tracker poisoned");
        if once_ids.len() > JOB_ONCE_CAPACITY {
            *once_ids = HashSet::with_capacity(JOB_RECORD_CAPACITY);
        }
        once_ids.insert(job_id);
    }
}

/// DDL 任务类型：普通 schema 变更或需要 reorg（数据重组）的任务。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobType {
    /// 普通 DDL（如改元数据、轻量变更）。
    General,
    /// 需要重组数据的 DDL（如加索引回填）。
    Reorg,
}
/// 已入队、待调度的任务及其涉及的 schema 对象与动作。
pub struct ScheduledJob {
    /// 底层 DDL Job。
    pub job: Job,
    /// 本任务涉及的 schema/表信息，用于冲突检测。
    pub involves: Vec<InvolvingSchemaInfo>,
    /// 任务类型，决定投递给哪个 worker。
    pub job_type: JobType,
    /// Schema 版本动作（SchemaAction）。
    pub action: SchemaAction,
}
/// Job 调度器：维护队列、运行中集合，以及 General/Reorg 两组 worker。
pub struct JobScheduler {
    /// 当前节点是否为 DDL Owner。
    pub owner: bool,
    /// 是否已关闭（关闭后不再调度）。
    pub closed: bool,
    queue: VecDeque<ScheduledJob>,
    running: RunningJobs,
    general_worker: JobWorker,
    reorg_worker: JobWorker,
    /// 强制重载 schema 的累计次数（成为 owner 等场景递增）。
    pub reload_schema_count: usize,
}
impl JobScheduler {
    /// 使用给定的 General/Reorg worker 构造调度器。
    pub fn new(general_worker: JobWorker, reorg_worker: JobWorker) -> Self {
        Self {
            owner: false,
            closed: false,
            queue: VecDeque::new(),
            running: RunningJobs::default(),
            general_worker,
            reorg_worker,
            reload_schema_count: 0,
        }
    }
    /// 成为 owner 时：标记所有权并必须重载 schema。
    pub fn on_become_owner(&mut self) {
        self.owner = true;
        self.must_reload_schemas();
    }
    /// 卸任 owner 时：清除所有权，并将运行中任务重置为 pending。
    pub fn on_retire_owner(&mut self) {
        self.owner = false;
        self.running.reset_all_pending();
    }
    /// 将任务追加到调度队列尾部。
    pub fn enqueue(&mut self, job: ScheduledJob) {
        self.queue.push_back(job);
    }
    /// 调度一轮：仅 owner 且未关闭时，按冲突规则投递可运行任务一步。
    ///
    /// 返回本轮成功推进的任务数；与运行中任务 schema 冲突的任务会重新入队并记为 pending。
    pub fn schedule(&mut self) -> Result<usize, String> {
        if self.closed || !self.owner {
            return Ok(0);
        }
        let mut delivered = 0;
        let count = self.queue.len();
        // 只遍历本轮开始时队列长度，避免新入队任务在同轮被重复处理。
        for _ in 0..count {
            let Some(mut scheduled) = self.queue.pop_front() else {
                break;
            };
            // schema 冲突：记 pending 并放回队尾，稍后重试。
            if !self
                .running
                .check_runnable(scheduled.job.id, &scheduled.involves)
            {
                self.running.add_pending(scheduled.involves.clone());
                self.queue.push_back(scheduled);
                continue;
            }
            self.running
                .add_running(scheduled.job.id, scheduled.involves.clone());
            let worker = if scheduled.job_type == JobType::Reorg {
                &mut self.reorg_worker
            } else {
                &mut self.general_worker
            };
            let mut context = JobContext::default();
            let result =
                worker.transit_one_job_step(&mut context, &mut scheduled.job, scheduled.action);
            // Synced/Cancelled 视为完成，否则放回队列等待下一步。
            let finished = matches!(scheduled.job.state, JobState::Synced | JobState::Cancelled);
            self.running.finish_or_pend_job(
                scheduled.job.id,
                scheduled.involves.clone(),
                !finished,
            );
            if !finished {
                self.queue.push_back(scheduled);
            }
            result?;
            delivered += 1;
        }
        self.running.reset_all_pending();
        Ok(delivered)
    }
    /// 关闭调度器并关闭两组 worker（幂等）。
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.general_worker.close();
        self.reorg_worker.close();
    }
    /// 记录一次必须重载 schema（测试中用计数器观测）。
    pub fn must_reload_schemas(&mut self) {
        self.reload_schema_count += 1;
    }

    /// Reload schemas until the loader succeeds or scheduler cancellation is observed.
    ///
    /// Cancellation is checked after a failed reload, matching Go's select after
    /// `SchemaLoader.Reload`. `retry_interval` is injectable so tests need not wait.
    pub fn must_reload_schemas_with<L, C>(
        &mut self,
        loader: &L,
        retry_interval: Duration,
        is_cancelled: C,
    ) where
        L: SchemaLoader + ?Sized,
        C: Fn() -> bool,
    {
        self.reload_schema_count += 1;
        loop {
            if loader.reload().is_ok() {
                return;
            }
            if is_cancelled() {
                return;
            }
            thread::sleep(retry_interval);
        }
    }
    /// 队列非空且仍有运行中任务时，视为 worker 池已耗尽（背压信号）。
    pub fn worker_pool_exhausted(&self) -> bool {
        !self.queue.is_empty() && !self.running.running_ids().is_empty()
    }
}
