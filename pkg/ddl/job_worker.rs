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

pub(crate) fn account_job_ru(
    next_gen: bool,
    transaction_size: usize,
    transaction_kv_byte_weight: f64,
    job: &mut astersql_meta_model::group_3::Job,
) {
    if matches!(
        job.state,
        astersql_meta_model::group_3::JobState::Cancelled
            | astersql_meta_model::group_3::JobState::RollbackDone
    ) {
        job.ru = 0.0;
    } else if next_gen {
        job.ru += transaction_size as f64 * transaction_kv_byte_weight;
    }
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
    /// Bind transient reorg contexts to one leadership tenure.
    fn bind_owner_epoch(&mut self, _: u64) -> Result<(), String> {
        Ok(())
    }
    /// GC registration uses an independent autocommit session, as in Go.
    fn register_delete_ranges(
        &mut self,
        _: &mut astersql_meta_model::group_3::Job,
    ) -> Result<(), String> {
        Err("DDL delete-range session unavailable".into())
    }

    fn backfill_index_batch(
        &mut self,
        _: crate::backfilling::IndexBackfillBatch,
    ) -> Result<crate::backfilling::BackfillTaskContext, String> {
        Err("transactional index backfill adapter unavailable".into())
    }
    fn query(&mut self, sql: &str, label: &str) -> Result<Vec<Vec<String>>, String>;
    fn begin(&mut self) -> Result<(), String>;
    fn commit(&mut self) -> Result<(), String>;
    fn rollback(&mut self);
    /// Return the bytes currently buffered by the active DDL transaction.
    fn transaction_size(&mut self) -> Result<usize, String> {
        Err("active DDL transaction size unavailable".into())
    }
    /// Report completed DDL RU to the job's resource group.
    fn report_ddl_job_ru(&mut self, _: &astersql_meta_model::group_3::Job) {}
    fn with_transaction(&mut self, operation: TransactionOperation) -> Result<Vec<u8>, String>;
    fn with_execution_context(&mut self, _: ExecutionOperation) -> Result<Vec<u8>, String> {
        Err("DDL execution context unavailable".into())
    }
}

pub type TransactionOperation =
    Box<dyn FnOnce(&mut dyn astersql_kv::Transaction) -> Result<Vec<u8>, String> + Send + 'static>;

/// SQL and metadata access to the same worker session. Transaction callbacks
/// release their borrow before SQL execution. Explicit resource callbacks retain
/// Go transaction boundaries for operations requiring an independent session.
pub trait JobExecutionContext {
    fn cached_storage_class_observation(
        &mut self,
        _: &crate::storage_class_transition::StorageClassTransitionOperation,
    ) -> Option<crate::storage_class_transition::StorageClassTransitionStatus> {
        None
    }
    /// Use this owner's real URI cache and initialized ingest disk resource.
    fn reorg_index_environment(
        &mut self,
    ) -> Result<&mut dyn crate::index::ReorgIndexEnvironment, String> {
        Err("DDL index reorg environment unavailable".into())
    }
    /// Real pooled build session: independently committed rows and actual read TSO.
    fn build_create_mview_data(
        &mut self,
        _: &mut astersql_meta_model::group_3::Job,
        _: &astersql_meta_model::TableInfo,
    ) -> Result<(u64, i64), String> {
        Err("materialized view independent build session unavailable".into())
    }
    /// Go prewriteCreateMaterializedViewRefreshInfo commits on a separate pooled
    /// session before the enclosing metadata/job transaction commits.
    fn prewrite_create_mview_refresh(&mut self, _: i64) -> Result<u64, String> {
        Err("materialized view independent refresh prewrite unavailable".into())
    }

    /// Publish the successful initial build into the durable refresh schedule.
    fn finish_create_mview_refresh(
        &mut self,
        _: &str,
        _: &astersql_meta_model::TableInfo,
        _: u64,
    ) -> Result<(), String> {
        Err("materialized view refresh publication unavailable".into())
    }

    /// Remove refresh bookkeeping while rolling CREATE MATERIALIZED VIEW back.
    fn delete_create_mview_refresh(&mut self, _: i64) -> Result<(), String> {
        Err("materialized view refresh cleanup unavailable".into())
    }

    /// Evaluate persisted MLog schedules on an isolated UTC evaluation context.
    fn derive_create_mlog_schedule(
        &mut self,
        _: &str,
        _: &astersql_meta_model::TableInfo,
    ) -> Result<(Option<i64>, bool), String> {
        Err("materialized view log schedule evaluation unavailable".into())
    }

    fn configure_create_table_replica(
        &mut self,
        _: &astersql_meta_model::TableInfo,
    ) -> Result<(), String> {
        Err("create-table PD replica resource unavailable".into())
    }
    fn put_create_table_bundles(
        &mut self,
        _: &[astersql_ddl_placement::Bundle],
    ) -> Result<(), String> {
        Err("create-table PD placement resource unavailable".into())
    }

    fn check_create_table_columnar(
        &mut self,
        _: &astersql_meta_model::TableInfo,
    ) -> Result<(), String> {
        Err("create-table columnar resource unavailable".into())
    }
    fn create_table_affinity(&mut self, _: &astersql_meta_model::TableInfo) -> Result<(), String> {
        Err("create-table affinity resource unavailable".into())
    }
    fn rebase_create_table_ids(
        &mut self,
        _: i64,
        _: &astersql_meta_model::TableInfo,
    ) -> Result<(), String> {
        Err("create-table allocator resource unavailable".into())
    }
    fn register_create_table_ttl(
        &mut self,
        _: &astersql_meta_model::TableInfo,
    ) -> Result<(), String> {
        Err("create-table TTL resource unavailable".into())
    }

    fn masking_policy_timestamp(&mut self) -> Result<String, String> {
        Err("DDL masking policy clock unavailable".into())
    }
    fn update_table_labels(
        &mut self,
        _: &str,
        _: &str,
        _: &str,
        _: &astersql_meta_model::TableInfo,
        _: bool,
    ) -> Result<(), String> {
        Err("DDL table label resources unavailable".into())
    }
    fn delete_drop_table_ttl(&mut self, _: i64) -> Result<(), String> {
        Err("drop-table TTL resource unavailable".into())
    }
    fn cleanup_drop_table_resources(
        &mut self,
        _: &astersql_meta_model::TableInfo,
    ) -> Result<(), String> {
        Err("drop-table replica resource unavailable".into())
    }
    fn delete_drop_table_affinity(
        &mut self,
        _: &astersql_meta_model::TableInfo,
    ) -> Result<(), String> {
        Err("drop-table affinity resource unavailable".into())
    }
    fn drop_table_rule_ids(
        &mut self,
        _: &str,
        _: &astersql_meta_model::TableInfo,
    ) -> Result<Vec<String>, String> {
        Err("drop-table label codec unavailable".into())
    }

    #[allow(clippy::too_many_arguments)]
    fn backfill_modified_column(
        &mut self,
        _: &astersql_meta_model::TableInfo,
        _: &astersql_meta_model::ColumnInfo,
        _: &astersql_meta_model::ColumnInfo,
        _: i64,
        _: &[u8],
        _: &[u8],
        _: usize,
        _: u64,
        _: Option<&astersql_meta_model::TimeZoneLocation>,
    ) -> Result<(Vec<u8>, i64), String> {
        Err("transactional modify-column backfill unavailable".into())
    }
    fn backfill_prepared_indexes(
        &mut self,
        _: crate::backfilling::IndexBackfillBatch,
    ) -> Result<crate::backfilling::BackfillTaskContext, String> {
        Err("transactional modify-column index backfill unavailable".into())
    }

    fn ingest_modified_indexes(
        &mut self,
        _: crate::backfilling::IndexBackfillBatch,
        _: &mut astersql_meta_model::group_3::Job,
    ) -> Result<crate::backfilling::BackfillTaskContext, String> {
        Err("modify-column SST ingest unavailable".into())
    }
    fn analyze_modified_table(
        &mut self,
        _: &mut astersql_meta_model::group_3::Job,
        _: &astersql_meta_model::TableInfo,
    ) -> Result<i8, String> {
        Err("modify-column analyze executor unavailable".into())
    }
    fn merge_modified_indexes(
        &mut self,
        _: crate::backfilling::IndexBackfillBatch,
        _: &mut astersql_meta_model::group_3::Job,
    ) -> Result<crate::backfilling::BackfillTaskContext, String> {
        Err("modify-column temporary index merge unavailable".into())
    }
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
    /// Identifies one leadership tenure, preventing an old task from resuming
    /// after this same process loses and regains ownership.
    fn owner_epoch(&self) -> u64 {
        0
    }
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
        session.bind_owner_epoch(lease.owner_epoch())?;
        executor.recover(job, lease)?;
        check_job_lease(lease)?;
        if let Err(error) = session.begin() {
            session.rollback();
            return Err(error);
        }
        // RU is encoded into the job row in this transaction. Keep the
        // caller-visible job consistent with durable state when any later
        // operation, including commit itself, abandons the transaction.
        let ru_before_transaction = job.ru;
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
            let failed = matches!(
                job.state,
                astersql_meta_model::group_3::JobState::Cancelled
                    | astersql_meta_model::group_3::JobState::RollbackDone
            );
            let transaction_size = if !failed && astersql_config_kerneltype::IsNextGen() {
                session.transaction_size()?
            } else {
                0
            };
            account_job_ru(
                astersql_config_kerneltype::IsNextGen(),
                transaction_size,
                astersql_config::get_global_config()
                    .ruv2
                    .ddl_weights
                    .txn_kv_bytes,
                job,
            );
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
            if job.state == astersql_meta_model::group_3::JobState::Synced && job.ru > 0.0 {
                astersql_metrics::ru_v2::AddDDLJobRU(job.ru);
                session.report_ddl_job_ru(job);
            }
            Ok(result.schema_version)
        })();
        if outcome.is_err() {
            session.rollback();
            job.ru = ru_before_transaction;
        }
        outcome
    }
}

impl JobWorker {
    /// Execute only the transaction backend; mode selection remains with the
    /// action. This is a production stage entrypoint, not a complete ADD INDEX handler.
    pub fn run_transactional_index_backfill(
        &mut self,
        session: &mut dyn DurableJobSession,
        lease: &dyn JobLease,
        job: &astersql_meta_model::group_3::Job,
        reorg: &mut crate::reorg::PersistentReorgContext,
        index_ids: &[i64],
    ) -> Result<crate::backfilling::BackfillResult, String> {
        use crate::backfilling::{
            BackfillResult, IndexBackfillBatch, ReorgBackfillTask, merge_warnings_and_counts,
        };
        use astersql_meta_model::group_3::{JobState, ReorgType};
        if self.closed {
            return Err("DDL worker is closed".into());
        }
        check_job_lease(lease)?;
        let epoch = lease.owner_epoch();
        if epoch == 0 {
            return Err("DDL owner tenure unavailable".into());
        }
        let meta = job
            .reorg_meta
            .as_ref()
            .ok_or("DDL reorg metadata missing")?;
        // Never silently route an ingest/DXF job through ordinary transactions.
        // The action retains responsibility for selecting/starting those backends.
        if meta.IsDistReorg || meta.ReorgTp != ReorgType::ReorgTypeTxn {
            return Err("transaction worker cannot execute selected ingest/DXF backend".into());
        }
        if job.state != JobState::Running
            || job.schema_state != astersql_meta_model::SchemaState::WriteReorganization
            || !matches!(
                job.tp,
                astersql_meta_model::group_3::ACTION_ADD_INDEX
                    | astersql_meta_model::group_3::ACTION_ADD_PRIMARY_KEY
            )
            || reorg.info.job_id != job.id
            || reorg.info.element.element_type != b"_idx_"
            || !index_ids.contains(&reorg.info.element.id)
        {
            return Err("DDL backfill job is not running or has invalid elements".into());
        }
        let rows = session.query(
            &format!(
                "select job_meta from mysql.tidb_ddl_job where job_id = {}",
                job.id
            ),
            "get_job",
        )?;
        let expected = rows
            .first()
            .and_then(|row| row.first())
            .ok_or("DDL job disappeared")?
            .clone();
        let persisted =
            astersql_meta::decode_go_history_job(expected.as_bytes()).map_err(|e| e.to_string())?;
        if persisted.state != JobState::Running
            || persisted.tp != job.tp
            || persisted.raw_args != job.raw_args
            || persisted.schema_id != job.schema_id
            || persisted.table_id != job.table_id
            || persisted.schema_state != job.schema_state
        {
            return Err("job meta changed by others".into());
        }
        let mut result = BackfillResult {
            next_key: reorg.info.start_key.clone(),
            ..Default::default()
        };
        let batch_size =
            usize::try_from(meta.GetBatchSize()).map_err(|_| "invalid backfill batch size")?;
        if batch_size == 0 {
            return Err("invalid backfill batch size".into());
        }
        while reorg.info.start_key < reorg.info.end_key {
            let request = IndexBackfillBatch {
                schema_id: job.schema_id,
                table_id: job.table_id,
                index_ids: index_ids.to_vec(),
                task: ReorgBackfillTask {
                    physical_table_id: reorg.info.physical_table_id,
                    job_id: job.id,
                    start_key: reorg.info.start_key.clone(),
                    end_key: reorg.info.end_key.clone(),
                    priority: job.priority as i32,
                    ..Default::default()
                },
                batch_size,
                resource_group: meta.ResourceGroupName.clone(),
                sql_mode: meta.SQLMode as i64,
            };
            let mut attempts = 0;
            let context = loop {
                check_job_lease_epoch(lease, epoch)?;
                if let Err(error) = session.begin() {
                    session.rollback();
                    return Err(error);
                }
                let batch = (|| {
                    let rows = session.query(
                        &format!(
                            "select job_meta from mysql.tidb_ddl_job where job_id = {}",
                            job.id
                        ),
                        "get_job",
                    )?;
                    if rows.first().and_then(|r| r.first()) != Some(&expected) {
                        return Err("job meta changed by others (paused or cancelled)".into());
                    }
                    let context = session.backfill_index_batch(request.clone())?;
                    if context.next_key <= reorg.info.start_key
                        || context.next_key > reorg.info.end_key
                    {
                        return Err("backfill adapter returned invalid progress".into());
                    }
                    check_job_lease_epoch(lease, epoch)?;
                    // Touch the observed job row in this same transaction. A
                    // concurrent ADMIN cancel/pause commits a conflicting write,
                    // even though this snapshot cannot see its new state yet.
                    let job_id = job.id;
                    session.with_transaction(Box::new(move |txn| {
                        let key = astersql_kv::Key(
                            astersql_tablecodec::EncodeRowKeyWithHandle(
                                astersql_meta_metadef::TiDBDDLJobTableID,
                                Box::new(astersql_tablecodec::kv::IntHandle(job_id)),
                            )
                            .0,
                        );
                        let value = astersql_kv::GetValue(
                            &astersql_kv::Context::default(),
                            txn,
                            key.clone(),
                        )
                        .map_err(|e| e.to_string())?;
                        // SQL UPDATE may elide unchanged values. The KV write
                        // intentionally remains in the transaction write set.
                        txn.Set(key, value).map_err(|e| e.to_string())?;
                        Ok(Vec::new())
                    }))?;
                    check_job_lease_epoch(lease, epoch)?;
                    session.commit()?;
                    Ok::<_, String>(context)
                })();
                match batch {
                    Ok(context) => break context,
                    Err(error) => {
                        session.rollback();
                        // Go RunInNewTxn retries only storage retryable failures;
                        // the whole batch is regenerated in the new snapshot.
                        if !error.contains(astersql_kv::TxnRetryableMark)
                            || attempts
                                >= astersql_kv::MaxRetryCnt
                                    .load(std::sync::atomic::Ordering::Relaxed)
                        {
                            return Err(error);
                        }
                        attempts += 1;
                        astersql_kv::BackOff(attempts);
                    }
                }
            };
            result.total_added_count += context.added_count;
            result.total_scan_count += context.scan_count;
            merge_warnings_and_counts(&mut result.warnings, &mut result.warning_counts, &context);
            reorg.runtime.increase_row_count(context.added_count);
            reorg
                .runtime
                .merge_warnings(&context.warnings, &context.warning_counts);
            // Checkpoint publication is independent, as in Go. If it fails,
            // committed index entries remain idempotently replayable.
            check_job_lease_epoch(lease, epoch)?;
            if let Err(error) = session.begin() {
                session.rollback();
                return Err(error);
            }
            let publish = (|| {
                crate::reorg::PersistentReorgHandler::stage_update(
                    session,
                    &reorg.info,
                    &context.next_key,
                )?;
                check_job_lease_epoch(lease, epoch)?;
                session.commit()
            })();
            if let Err(error) = publish {
                session.rollback();
                return Err(error);
            }
            reorg.info.start_key = context.next_key.clone();
            result.next_key = context.next_key;
            if context.done {
                break;
            }
        }
        Ok(result)
    }
}

fn check_job_lease_epoch(lease: &dyn JobLease, epoch: u64) -> Result<(), String> {
    check_job_lease(lease)?;
    if lease.owner_epoch() != epoch {
        return Err("DDL owner tenure changed".into());
    }
    Ok(())
}

/// The shared initialization stage is a durable worker operation, like the
/// transactional backfill stage above. It does not pretend to run later index
/// build, ingest, DXF, rollback or publication stages.
struct IndexReorgInitialization {
    indexes: Vec<astersql_meta_model::IndexInfo>,
}
impl DurableJobExecutor for IndexReorgInitialization {
    fn runnable(
        &mut self,
        _: &mut dyn DurableJobSession,
        _: &astersql_meta_model::group_3::Job,
    ) -> Result<bool, String> {
        Ok(true)
    }
    fn recover(
        &mut self,
        _: &astersql_meta_model::group_3::Job,
        _: &dyn JobLease,
    ) -> Result<(), String> {
        Ok(())
    }
    fn wait_synced(
        &mut self,
        _: &astersql_meta_model::group_3::Job,
        _: i64,
        _: &dyn JobLease,
    ) -> Result<(), String> {
        Ok(())
    }
    fn step(
        &mut self,
        session: &mut dyn DurableJobSession,
        job: &mut astersql_meta_model::group_3::Job,
    ) -> Result<DurableJobStep, String> {
        let mut current = astersql_meta_model::group_3::Job::decode(
            &job.encode(false).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let mut indexes = self.indexes.clone();
        let output = session.with_execution_context(Box::new(move |context| {
            let outcome = crate::persistent_actions::initialize_reorg_indexes(
                context,
                &mut current,
                &mut indexes,
            );
            serde_json::to_vec(&(
                current.encode(false).map_err(|e| e.to_string())?,
                indexes,
                outcome,
            ))
            .map_err(|e| e.to_string())
        }))?;
        let (encoded, indexes, outcome): (
            Vec<u8>,
            Vec<astersql_meta_model::IndexInfo>,
            Result<(), String>,
        ) = serde_json::from_slice(&output).map_err(|e| e.to_string())?;
        // Initialization only owns ReorgMeta. Preserve the caller's decoded args
        // and multi-schema proxy fields, which are intentionally absent on wire.
        job.reorg_meta = astersql_meta_model::group_3::Job::decode(&encoded)
            .map_err(|e| e.to_string())?
            .reorg_meta;
        self.indexes = indexes;
        outcome?;
        Ok(DurableJobStep {
            schema_version: 0,
            update_raw_args: false,
            removed: false,
        })
    }
}
impl JobWorker {
    /// Initialize the real job through its owner's SQL/KV transaction and URI
    /// cache. Return the changed index metadata for the action's schema step.
    pub fn initialize_persisted_index_reorg(
        &mut self,
        session: &mut dyn DurableJobSession,
        lease: &dyn JobLease,
        job: &mut astersql_meta_model::group_3::Job,
        expected_bytes: &[u8],
        indexes: Vec<astersql_meta_model::IndexInfo>,
    ) -> Result<Vec<astersql_meta_model::IndexInfo>, String> {
        struct InitializationLease<'a> {
            lease: &'a dyn JobLease,
            epoch: u64,
        }
        impl JobLease for InitializationLease<'_> {
            fn owner_epoch(&self) -> u64 {
                self.epoch
            }
            fn is_owner(&self) -> bool {
                self.lease.is_owner() && self.lease.owner_epoch() == self.epoch
            }
            fn is_cancelled(&self) -> bool {
                self.lease.is_cancelled()
            }
        }
        let captured = InitializationLease {
            lease,
            epoch: lease.owner_epoch(),
        };
        let mut executor = IndexReorgInitialization { indexes };
        self.transit_persisted_job_step(session, &captured, &mut executor, job, expected_bytes)?;
        Ok(executor.indexes)
    }
}
