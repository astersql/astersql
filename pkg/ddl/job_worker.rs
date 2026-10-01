// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// DDL Job Worker：推进单个 DDL 任务（job）的状态机，并维护 schema 版本差分。
//
// DDL（Data Definition Language）指 CREATE/ALTER/DROP 等修改元数据的语句。
// 在分布式数据库中，DDL 由 owner（DDL 所有者节点）通过 worker 逐步执行；
// 每一步可能产生 SchemaDiff（schema 版本差分），供其他节点同步元数据。
// Reorg（reorganization，数据重组/回填）用于处理加索引等需要扫描存量数据的场景。

use crate::ddl::{ActionType, Job, JobState};
use crate::schema_version::{AffectedOption, SchemaAction, SchemaDiff, SchemaVersionManager};
use std::time::Duration;

/// Worker 类型：通用 DDL 与加索引两类，便于按负载隔离调度。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerType {
    /// 处理普通 DDL 任务。
    General,
    /// 处理加索引类任务（通常伴随 reorg 回填）。
    AddIndex,
}

/// Reorg（数据重组）上下文：记录回填进度与资源组等信息。
#[derive(Clone, Debug, Default)]
pub struct ReorgContext {
    /// 已处理行数。
    pub row_count: i64,
    /// 回填过程中的警告计数。
    pub warning_count: u64,
    /// 回填是否已完成。
    pub done: bool,
    /// 关联的资源组名称（用于限流/配额）。
    pub resource_group: String,
}

/// 单个 job 执行时的会话上下文。
#[derive(Clone, Debug)]
pub struct JobContext {
    /// 当前节点是否为 DDL owner（只有 owner 可推进 job）。
    pub owner: bool,
    /// 是否启用 MDL（Metadata Lock，元数据锁），用于阻塞冲突的 DML/DDL。
    pub metadata_lock_enabled: bool,
    /// 本步执行是否已开始。
    pub step_started: bool,
    /// 连续错误次数，用于错误重试上限。
    pub error_count: usize,
    /// 与 reorg 回填相关的运行时状态。
    pub reorg: ReorgContext,
}

impl Default for JobContext {
    fn default() -> Self {
        Self {
            owner: true,
            metadata_lock_enabled: true,
            step_started: false,
            error_count: 0,
            reorg: ReorgContext::default(),
        }
    }
}

/// DDL 任务执行器：持有 schema 版本管理器与已完成 job 历史。
pub struct JobWorker {
    /// 本 worker 负责的任务类别。
    pub worker_type: WorkerType,
    /// 是否已关闭；关闭后拒绝继续推进 job。
    pub closed: bool,
    /// Schema 版本管理器：负责更新与解锁 schema 版本。
    pub version_manager: SchemaVersionManager,
    /// 已结束（完成或取消）的 job 历史。
    pub history: Vec<Job>,
}

impl JobWorker {
    /// 创建指定类型的 worker，并初始化空的版本管理器与历史。
    pub fn new(worker_type: WorkerType) -> Self {
        Self {
            worker_type,
            closed: false,
            version_manager: SchemaVersionManager::default(),
            history: Vec::new(),
        }
    }

    /// 关闭 worker，后续 `transit_one_job_step` 将返回错误。
    pub fn close(&mut self) {
        self.closed = true;
    }

    /// 推进 job 一个状态步，必要时写出 SchemaDiff。
    ///
    /// 状态机概要：None → Running → Done → Synced；
    /// Cancelling → Cancelled；Paused 保持不变。
    pub fn transit_one_job_step(
        &mut self,
        context: &mut JobContext,
        job: &mut Job,
        action: SchemaAction,
    ) -> Result<Option<SchemaDiff>, String> {
        if self.closed {
            return Err("DDL worker is closed".into());
        }
        if !context.owner {
            return Err("not DDL owner".into());
        }
        context.step_started = true;
        // 按当前 JobState 做一步迁移；仅 Running 步会真正更新 schema 版本。
        match job.state {
            JobState::None => {
                job.state = JobState::Running;
                Ok(None)
            }
            JobState::Paused => Ok(None),
            JobState::Cancelling => {
                job.state = JobState::Cancelled;
                self.finish_job(job.clone());
                Ok(None)
            }
            JobState::Running => {
                // Go releases schemaVersionManager after every committed job step,
                // allowing unrelated DDL jobs to advance concurrently.
                let diff = self.version_manager.update(job, action, Vec::new());
                self.version_manager.unlock(job.id);
                let diff = diff?;
                job.state = JobState::Done;
                Ok(Some(diff))
            }
            JobState::Done => {
                job.state = JobState::Synced;
                self.finish_job(job.clone());
                Ok(None)
            }
            JobState::Cancelled | JobState::Synced => Ok(None),
        }
    }

    /// 结束 job：解锁对应 schema 版本锁，并写入历史。
    fn finish_job(&mut self, job: Job) {
        self.version_manager.unlock(job.id);
        self.history.push(job);
    }

    /// 累计错误次数；达到阈值（3）则把错误上抛给调用方。
    pub fn count_for_error(&self, context: &mut JobContext, error: &str) -> Result<(), String> {
        context.error_count += 1;
        if context.error_count >= 3 {
            Err(error.to_owned())
        } else {
            Ok(())
        }
    }
}

/// 判断 job 是否需要 GC（垃圾回收）。
///
/// 与 Go 的 `JobNeedGC` 一致，取消的任务不产生 delete-range；当前 Rust
/// `Job` 已建模的 DROP/TRUNCATE 动作需要清理，普通动作不需要。
pub fn job_need_gc(job: &Job) -> bool {
    job.state != JobState::Cancelled
        && matches!(
            job.action_type,
            ActionType::DropTable | ActionType::TruncateTable
        )
}

/// 选择实际 lease（租约）时长；零值表示使用上限。
pub fn choose_lease_time(lease: Duration, maximum: Duration) -> Duration {
    if lease.is_zero() || lease > maximum {
        maximum
    } else {
        lease
    }
}

/// 根据新旧表 ID 列表构造 Placement（放置策略）相关的 AffectedOption 列表。
pub fn build_placement_affects(old_ids: &[i64], new_ids: &[i64]) -> Vec<AffectedOption> {
    assert!(
        new_ids.len() >= old_ids.len(),
        "new table IDs must cover every old table ID"
    );
    old_ids
        .iter()
        .enumerate()
        .map(|(index, &old_table_id)| AffectedOption {
            old_table_id,
            table_id: new_ids[index],
            ..AffectedOption::default()
        })
        .collect()
}

/// A pooled SQL session and its actual KV transaction. Operations stay on the
/// session's owning thread; SQL job writes and metadata writes share one commit.
pub trait DurableJobSession {
    /// GC registration uses an independent autocommit session, as in Go.
    fn register_delete_ranges(
        &mut self,
        _: &mut astersql_meta_model::group_3::Job,
    ) -> Result<(), String> {
        Err("DDL delete-range session unavailable".into())
    }

    fn query(&mut self, sql: &str, label: &str) -> Result<Vec<Vec<String>>, String>;
    fn begin(&mut self) -> Result<(), String>;
    fn commit(&mut self) -> Result<(), String>;
    fn rollback(&mut self);
    fn with_transaction(&mut self, operation: TransactionOperation) -> Result<Vec<u8>, String>;
    fn with_execution_context(&mut self, _: ExecutionOperation) -> Result<Vec<u8>, String> {
        Err("DDL execution context unavailable".into())
    }
}

pub type TransactionOperation =
    Box<dyn FnOnce(&mut dyn astersql_kv::Transaction) -> Result<Vec<u8>, String> + Send + 'static>;

/// SQL and metadata access to the same worker session. Transaction callbacks
/// release their borrow before SQL execution; handlers cannot open another session.
pub trait JobExecutionContext {
    /// Recover durable reorg state on this worker's real SQL session.
    fn restore_reorg(
        &mut self,
        job: &mut astersql_meta_model::group_3::Job,
    ) -> Result<crate::reorg::PersistentReorgContext, String> {
        crate::reorg::restore_reorg(job, |sql| self.query(sql, "get_handle"))
    }
    fn query(&mut self, sql: &str, label: &str) -> Result<Vec<Vec<String>>, String>;
    fn with_transaction(
        &mut self,
        operation: &mut dyn FnMut(&mut dyn astersql_kv::Transaction) -> Result<Vec<u8>, String>,
    ) -> Result<Vec<u8>, String>;
}
pub type ExecutionOperation =
    Box<dyn FnOnce(&mut dyn JobExecutionContext) -> Result<Vec<u8>, String> + Send + 'static>;

/// The owner manager and scheduler cancellation must both permit every commit.
pub trait JobLease {
    fn is_owner(&self) -> bool;
    fn is_cancelled(&self) -> bool;
}

/// Action execution, upgrade policy and schema synchronization are supplied by
/// the normal DDL executor. There is deliberately no default successful step.
pub trait DurableJobExecutor {
    fn runnable(
        &mut self,
        session: &mut dyn DurableJobSession,
        job: &astersql_meta_model::group_3::Job,
    ) -> Result<bool, String>;
    /// Recover a previous owner's unsynchronized schema version before executing.
    fn recover(
        &mut self,
        job: &astersql_meta_model::group_3::Job,
        lease: &dyn JobLease,
    ) -> Result<(), String>;
    /// Action errors that Go persists on Job must be handled by the executor
    /// and returned as a successful transaction result with the updated Job.
    /// Err is reserved for an abandoned transaction (storage/lease/staging failure).
    fn step(
        &mut self,
        session: &mut dyn DurableJobSession,
        job: &mut astersql_meta_model::group_3::Job,
    ) -> Result<DurableJobStep, String>;
    fn wait_synced(
        &mut self,
        job: &astersql_meta_model::group_3::Job,
        schema_version: i64,
        lease: &dyn JobLease,
    ) -> Result<(), String>;
}

pub struct DurableJobStep {
    pub schema_version: i64,
    pub update_raw_args: bool,
    /// A terminal executor step has already deleted the queue row and written
    /// history inside this transaction; do not recreate its queue entry.
    pub removed: bool,
}

fn check_job_lease(lease: &dyn JobLease) -> Result<(), String> {
    if !lease.is_owner() {
        return Err("not DDL owner".into());
    }
    if lease.is_cancelled() {
        return Err("DDL scheduler cancelled".into());
    }
    Ok(())
}

impl JobWorker {
    /// Go transitOneJobStep's transaction boundary, using the full wire Job.
    /// The queue bytes are rechecked inside the same transaction as metadata,
    /// so administrative changes and overlapping owners cannot be overwritten.
    pub fn transit_persisted_job_step(
        &mut self,
        session: &mut dyn DurableJobSession,
        lease: &dyn JobLease,
        executor: &mut dyn DurableJobExecutor,
        job: &mut astersql_meta_model::group_3::Job,
        expected_bytes: &[u8],
    ) -> Result<i64, String> {
        if self.closed {
            return Err("DDL worker is closed".into());
        }
        check_job_lease(lease)?;
        executor.recover(job, lease)?;
        check_job_lease(lease)?;
        if let Err(error) = session.begin() {
            session.rollback();
            return Err(error);
        }
        let outcome = (|| {
            let rows = session.query(
                &format!(
                    "select job_meta from mysql.tidb_ddl_job where job_id = {}",
                    job.id
                ),
                "get_job",
            )?;
            let current = rows
                .first()
                .and_then(|row| row.first())
                .ok_or_else(|| "DDL job disappeared".to_owned())?;
            if current.as_bytes() != expected_bytes {
                return Err("job meta changed by others".into());
            }
            let result = executor.step(session, job)?;
            check_job_lease(lease)?;
            if !result.removed {
                let bytes = astersql_meta::encode_go_ddl_job(job, result.update_raw_args)?;
                let hex = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
                session.query(
                    &format!(
                        "update mysql.tidb_ddl_job set job_meta = X'{hex}' where job_id = {}",
                        job.id
                    ),
                    "update_job",
                )?;
            }
            // A lease may be lost during SQL execution as well as during the
            // metadata callback. Neither case is allowed to commit.
            check_job_lease(lease)?;
            session.commit()?;
            Ok(result.schema_version)
        })();
        if outcome.is_err() {
            session.rollback();
        }
        outcome
    }
}
