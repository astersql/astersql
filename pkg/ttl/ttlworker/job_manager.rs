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

// TTL 作业管理器：在 leader 上锁定表级清理作业、心跳、超时接管与收尾汇总。
//
// `JobManager` 聚合表元数据、`tidb_ttl_table_status` 对应的内存状态、本地
// `JobStore` 与 `TaskManager`，并实现 `TtlJobAdapter` 供定时器提交/查询作业。
// 过期水位（expire time）表示“早于该时间的 TTL 列值视为过期”。

use std::collections::{BTreeMap, BTreeSet};

use crate::job::{JobStore, TtlJob};
use crate::job_version_checker::{JobVersionCheckResult, JobVersionChecker, ServerInfo};
use crate::scan::ScanIndex;
use crate::session::PhysicalTable;
use crate::task_manager::{ManagedTask, TaskManager, TaskState, TaskStatus};
use crate::timer::{TtlJobAdapter, TtlJobTrace};

/// 向系统表插入新物理表状态行的 SQL 模板。
pub const INSERT_NEW_TABLE_INTO_STATUS_SQL: &str =
    "INSERT INTO mysql.tidb_ttl_table_status (table_id,parent_table_id) VALUES (%?, %?)";

/// 一次 TTL 作业的行级汇总：总行、成功行、错误行及扫描侧错误信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TtlSummary {
    /// 扫描到的总行数。
    pub total_rows: u64,
    /// 成功删除（或处理）的行数。
    pub success_rows: u64,
    /// 处理失败的行数。
    pub error_rows: u64,
    /// 扫描任务级错误描述；无错误时为空串。
    pub scan_task_err: String,
}

/// 将已完成子任务的计数汇总为 `TtlSummary`，可选附带扫描错误。
pub fn summarize_task_results(tasks: &[ManagedTask], scan_error: Option<&str>) -> TtlSummary {
    let mut summary = TtlSummary::default();
    for task in tasks {
        summary.total_rows += task.state.total_rows;
        summary.success_rows += task.state.success_rows;
        summary.error_rows += task.state.error_rows;
    }
    if let Some(error) = scan_error {
        summary.scan_task_err = error.to_owned();
    }
    summary
}

/// 对应 `mysql.tidb_ttl_table_status` 一行的内存视图。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableStatus {
    /// 物理表 ID。
    pub table_id: i64,
    /// 逻辑/父表 ID。
    pub parent_table_id: i64,
    /// 当前进行中的作业 ID。
    pub current_job_id: Option<String>,
    /// 作业所有者节点 ID。
    pub owner_id: Option<String>,
    /// 所有者最近心跳时间（Unix 秒）。
    pub owner_heartbeat: u64,
    /// 当前作业开始时间。
    pub job_start: u64,
    /// 当前作业使用的过期水位。
    pub job_expire: u64,
}

/// TTL 作业协调器：仅 leader 可锁定新作业；管理心跳、超时抢占与完成回收。
pub struct JobManager {
    /// 本管理器（节点）ID。
    pub id: String,
    /// 是否为当前 TTL 调度 leader。
    pub is_leader: bool,
    /// 已知物理表元数据，按 physical_id 索引。
    pub tables: BTreeMap<i64, PhysicalTable>,
    /// 各物理表的作业状态。
    pub statuses: BTreeMap<i64, TableStatus>,
    /// 本地活跃作业与历史仓。
    pub store: JobStore,
    /// 扫描子任务调度器。
    pub task_manager: TaskManager,
    /// 管理器侧“当前时间”注入点（便于测试）。
    pub now: u64,
    /// 外部请求 ID → (物理表 ID, 作业 ID) 映射。
    request_to_job: BTreeMap<String, (i64, String)>,
    /// Eligible TTL index by physical table; absence keeps the legacy PK path.
    ttl_indexes: BTreeMap<i64, ScanIndex>,
    /// Persisted scan choice by job ID, mirroring scan_index_id in task rows.
    pub job_scan_indexes: BTreeMap<String, Option<ScanIndex>>,
    pub enable_index_scan: bool,
    pub local_server: Result<Option<ServerInfo>, String>,
    pub all_servers: Result<Vec<(String, Option<ServerInfo>)>, String>,
    job_version_checker: JobVersionChecker,
}

impl JobManager {
    /// 构造管理器；`max_running_tasks` 交给 `TaskManager` 约束并发。
    pub fn new(id: impl Into<String>, max_running_tasks: usize) -> Self {
        let id = id.into();
        Self {
            task_manager: TaskManager::new(id.clone(), max_running_tasks),
            id,
            is_leader: false,
            tables: BTreeMap::new(),
            statuses: BTreeMap::new(),
            store: JobStore::default(),
            now: 0,
            request_to_job: BTreeMap::new(),
            ttl_indexes: BTreeMap::new(),
            job_scan_indexes: BTreeMap::new(),
            enable_index_scan: true,
            local_server: Err("server version unavailable".into()),
            all_servers: Err("server versions unavailable".into()),
            job_version_checker: JobVersionChecker::default(),
        }
    }
    pub fn set_ttl_index(&mut self, physical_id: i64, index: Option<ScanIndex>) {
        if let Some(index) = index {
            self.ttl_indexes.insert(physical_id, index);
        } else {
            self.ttl_indexes.remove(&physical_id);
        }
    }
    /// 用最新物理表列表整体替换本地表缓存。
    pub fn refresh_tables(&mut self, tables: impl IntoIterator<Item = PhysicalTable>) {
        self.tables = tables
            .into_iter()
            .map(|table| (table.physical_id, table))
            .collect();
    }
    /// 在 leader 上为指定物理表锁定新作业；失败返回 `None`。
    ///
    /// `check_schedule_interval` 为真时，若距上次 `job_start` 不足表的
    /// `expire_after_seconds` 则拒绝再次调度，避免过于频繁清理。
    pub fn lock_new_job(
        &mut self,
        physical_id: i64,
        job_id: impl Into<String>,
        now: u64,
        check_schedule_interval: bool,
    ) -> Option<TtlJob> {
        if !self.is_leader {
            return None;
        }
        let table = self.tables.get(&physical_id)?.clone();
        let status = self.statuses.entry(physical_id).or_insert(TableStatus {
            table_id: physical_id,
            parent_table_id: table.table_id,
            current_job_id: None,
            owner_id: None,
            owner_heartbeat: 0,
            job_start: 0,
            job_expire: 0,
        });
        // 已有进行中作业，或未满足调度间隔，则不锁定。
        if status.current_job_id.is_some()
            || (check_schedule_interval
                && now.saturating_sub(status.job_start) <= table.expire_after_seconds)
        {
            return None;
        }
        let job_id = job_id.into();
        let expire_time = table.expire_time(now);
        status.current_job_id = Some(job_id.clone());
        status.owner_id = Some(self.id.clone());
        status.owner_heartbeat = now;
        status.job_start = now;
        status.job_expire = expire_time;
        let job = TtlJob {
            id: job_id,
            owner_id: self.id.clone(),
            table,
            create_time: now,
            expire_time,
            finished: false,
        };
        self.store.active_jobs.insert(physical_id, job.clone());
        job.create_history(&mut self.store);
        Some(job)
    }
    /// 刷新本节点作为 owner 的本地活跃作业心跳时间。
    pub fn update_heartbeat(&mut self, now: u64) {
        let active_tables: BTreeSet<_> = self.store.active_jobs.keys().copied().collect();
        for status in self.statuses.values_mut().filter(|status| {
            status.owner_id.as_deref() == Some(&self.id) && active_tables.contains(&status.table_id)
        }) {
            status.owner_heartbeat = now;
        }
    }
    /// 接管非本地活跃且心跳超时的作业：改写 owner 并返回物理表 ID。
    pub fn reschedule_timeout_jobs(&mut self, now: u64, timeout_seconds: u64) -> Vec<i64> {
        let mut locked = Vec::new();
        for (&physical_id, status) in &mut self.statuses {
            if !self.store.active_jobs.contains_key(&physical_id)
                && status.current_job_id.is_some()
                && now.saturating_sub(status.owner_heartbeat) > timeout_seconds
            {
                status.owner_id = Some(self.id.clone());
                status.owner_heartbeat = now;
                locked.push(physical_id);
            }
        }
        locked
    }
    /// 当某作业的全部扫描子任务已完成时，汇总结果并调用 `TtlJob::finish`。
    pub fn finish_completed_jobs(&mut self, now: u64) -> Vec<String> {
        let active: Vec<_> = self.store.active_jobs.values().cloned().collect();
        let mut finished = Vec::new();
        for mut job in active {
            let tasks: Vec<_> = self
                .task_manager
                .finished()
                .iter()
                .filter(|task| task.task.job_id == job.id)
                .cloned()
                .collect();
            let task_count = self.store.tasks_by_job.get(&job.id).copied().unwrap_or(0);
            // 仍有未完成的子任务则跳过。
            if task_count != 0 && tasks.len() < task_count {
                continue;
            }
            let summary = summarize_task_results(&tasks, None);
            if job.finish(&mut self.store, now, summary) {
                if let Some(status) = self.statuses.get_mut(&job.table.physical_id) {
                    status.current_job_id = None;
                    status.owner_id = None;
                }
                finished.push(job.id);
            }
        }
        finished
    }
    /// 仅 leader 按保留期清理历史，并剔除已不存在且无进行中作业的表状态。
    pub fn gc(&mut self, now: u64, retention_seconds: u64) {
        if !self.is_leader {
            return;
        }
        self.store
            .history
            .retain(|_, history| now.saturating_sub(history.create_time) <= retention_seconds);
        let live: BTreeSet<_> = self.tables.keys().copied().collect();
        self.statuses.retain(|physical_id, status| {
            live.contains(physical_id) || status.current_job_id.is_some()
        });
    }
}

impl TtlJobAdapter for JobManager {
    /// 仅 leader、表启用 TTL 且当前无作业时可提交。
    fn can_submit_job(&self, table_id: i64, physical_id: i64) -> bool {
        self.is_leader
            && self
                .tables
                .get(&physical_id)
                .is_some_and(|table| table.table_id == table_id && table.ttl_enabled)
            && self
                .statuses
                .get(&physical_id)
                .is_none_or(|status| status.current_job_id.is_none())
    }
    /// 由定时器调用：锁定作业并记录 request_id 映射，返回进行中的跟踪信息。
    fn submit_job(
        &mut self,
        table_id: i64,
        physical_id: i64,
        request_id: &str,
        now: u64,
    ) -> Result<TtlJobTrace, String> {
        if !self.can_submit_job(table_id, physical_id) {
            return Err("TTL job cannot be submitted".to_owned());
        }
        let job_id = format!("{physical_id}-{now}-{request_id}");
        let selected_index =
            if self.enable_index_scan && self.ttl_indexes.contains_key(&physical_id) {
                match self.job_version_checker.check(
                    now,
                    self.local_server.clone(),
                    self.all_servers.clone(),
                ) {
                    JobVersionCheckResult::AllowIndexScan => {
                        self.ttl_indexes.get(&physical_id).cloned()
                    }
                    JobVersionCheckResult::FallbackToPrimaryKey => None,
                    JobVersionCheckResult::BlockJob => {
                        return Err(
                        "cannot create TTL job while TiDB server build versions are inconsistent"
                            .into(),
                    );
                    }
                }
            } else {
                None
            };
        self.lock_new_job(physical_id, job_id.clone(), now, false)
            .ok_or_else(|| "failed to lock TTL job".to_owned())?;
        self.job_scan_indexes.insert(job_id.clone(), selected_index);
        self.request_to_job
            .insert(request_id.to_owned(), (physical_id, job_id));
        Ok(TtlJobTrace {
            request_id: request_id.to_owned(),
            finished: false,
            summary: None,
        })
    }
    /// 按 request_id 查询作业是否仍活跃；已结束后附带历史汇总。
    fn get_job(
        &self,
        table_id: i64,
        physical_id: i64,
        request_id: &str,
    ) -> Result<TtlJobTrace, String> {
        let (recorded_id, job_id) = self
            .request_to_job
            .get(request_id)
            .ok_or_else(|| "TTL request not found".to_owned())?;
        if *recorded_id != physical_id
            || self
                .tables
                .get(&physical_id)
                .is_none_or(|table| table.table_id != table_id)
        {
            return Err("TTL request table mismatch".to_owned());
        }
        if self.store.active_jobs.contains_key(&physical_id) {
            return Ok(TtlJobTrace {
                request_id: request_id.to_owned(),
                finished: false,
                summary: None,
            });
        }
        let summary = self
            .store
            .history
            .get(job_id)
            .and_then(|history| history.summary.clone());
        Ok(TtlJobTrace {
            request_id: request_id.to_owned(),
            finished: true,
            summary,
        })
    }
    fn now(&self) -> u64 {
        self.now
    }
}

/// 将扫描任务包装为等待调度的 `ManagedTask`（初始无 owner、状态 Waiting）。
pub fn initial_managed_task(task: crate::scan::TtlScanTask) -> ManagedTask {
    ManagedTask {
        task,
        status: TaskStatus::Waiting,
        owner_id: None,
        owner_heartbeat: 0,
        state: TaskState::default(),
    }
}
