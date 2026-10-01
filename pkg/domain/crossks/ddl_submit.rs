// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 跨 Keyspace DDL 提交客户端：解析目标、构建 Alter Table Mode Job、提交并轮询历史状态。
// Table Mode 表示表当前处于 Normal/Import/Restore 等模式，用于控制导入/恢复期间的行为约束。
// DDL Owner 是集群中唯一推进 DDL 任务的节点；notify_owner 用于唤醒其调度。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// 轮询 DDL 历史任务完成状态的间隔。
pub const DDL_HISTORY_POLL_INTERVAL: Duration = Duration::from_millis(100);
#[derive(Clone, Debug, Eq, PartialEq)]
/// DDL 提交路径上的错误包装。
pub struct Error(pub String);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 表模式：Normal 常规读写；Import 导入中；Restore 恢复中。
pub enum TableMode {
    /// 常规模式。
    Normal,
    /// 导入模式（可与 Normal 互转）。
    Import,
    /// 恢复模式（仅允许回到 Normal）。
    Restore,
}
impl TableMode {
    fn jobsubmit_mode(self) -> astersql_ddl_jobsubmit::TableMode {
        match self {
            Self::Normal => astersql_ddl_jobsubmit::TableMode::Normal,
            Self::Import => astersql_ddl_jobsubmit::TableMode::Import,
            Self::Restore => astersql_ddl_jobsubmit::TableMode::Restore,
        }
    }
}
#[derive(Clone, Debug)]
/// 变更表模式的目标描述（schema/表标识、当前与目标模式）。
pub struct AlterTableModeTarget {
    pub schema_id: i64,
    pub schema_name: String,
    pub table_id: i64,
    pub table_name: String,
    pub current_mode: TableMode,
    pub target_mode: TableMode,
}
#[derive(Clone, Copy, Debug, Default)]
/// 提交 Job 时需要携带的会话变量快照。
pub struct SessionVariables {
    /// CDC（变更数据捕获）写入来源标识。
    pub cdc_write_source: u64,
    /// SQL Mode 位图。
    pub sql_mode: u64,
}
#[derive(Clone, Debug)]
/// 提交给 DDL 子系统的 Alter Table Mode 任务。
pub struct AlterTableModeJob {
    pub id: i64,
    pub schema_id: i64,
    pub table_id: i64,
    pub schema_name: String,
    pub table_name: String,
    pub target_mode: TableMode,
    pub query: String,
    pub cdc_write_source: u64,
    pub sql_mode: u64,
}
#[derive(Clone, Debug)]
/// DDL 历史任务终态：已同步、失败或非预期状态。
pub enum HistoryJobState {
    /// 任务已成功同步完成。
    Synced,
    /// 任务失败，携带错误信息。
    Failed(String),
    /// 任务以非预期状态结束。
    Unexpected(String),
}
#[derive(Default)]
/// 可取消令牌：用于中断等待 DDL 完成的轮询循环。
pub struct Cancellation {
    cancelled: AtomicBool,
}
impl Cancellation {
    /// 标记已取消。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    /// 查询是否已取消。
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// DDL 后端抽象：解析元数据、提交 Job、通知 Owner、查询历史。
pub trait DdlBackend: Send + Sync {
    /// 按 schema_id 解析数据库名。
    fn resolve_database(&self, schema_id: i64) -> Result<Option<String>, Error>;
    /// 按 schema_id/table_id 解析表名与当前 TableMode。
    fn resolve_table(
        &self,
        schema_id: i64,
        table_id: i64,
    ) -> Result<Option<(String, TableMode)>, Error>;
    /// Resolve both objects in one snapshot when the backend supports MVCC.
    fn resolve_metadata(
        &self,
        schema_id: i64,
        table_id: i64,
    ) -> Result<(Option<String>, Option<(String, TableMode)>), Error> {
        let database = self.resolve_database(schema_id)?;
        let table = if database.is_some() {
            self.resolve_table(schema_id, table_id)?
        } else {
            None
        };
        Ok((database, table))
    }
    /// 读取提交 Job 所需的会话变量。
    fn session_variables(&self) -> Result<SessionVariables, Error>;
    /// 刷新本节点 server state（与集群状态对齐）。
    fn refresh_server_state(&self) -> Result<(), Error>;
    /// 提交 AlterTableModeJob（可能回填 job.id）。
    fn submit(&self, job: &mut AlterTableModeJob) -> Result<(), Error>;
    /// 通知 DDL Owner 有新任务。
    fn notify_owner(&self) -> Result<(), Error>;
    /// 查询历史 Job 状态；None 表示尚未入库/未完成。
    fn history_job(&self, job_id: i64) -> Result<Option<HistoryJobState>, Error>;
}
/// 面向跨 KS 调用方的 DDL 客户端封装。
pub struct DdlClient {
    backend: Arc<dyn DdlBackend>,
}
impl DdlClient {
    /// 使用给定后端构造客户端。
    pub fn new(backend: Arc<dyn DdlBackend>) -> Self {
        Self { backend }
    }
    /// 解析目标 → 构建 Job → 刷新状态 → 提交 → 通知 Owner → 等待完成。
    pub fn alter_table_mode(
        &self,
        cancellation: &Cancellation,
        request: AlterTableModeTarget,
    ) -> Result<(), Error> {
        let target = self.resolve_alter_table_mode_target(request)?;
        let Some(mut job) = self.build_alter_table_mode_job(&target)? else {
            return Ok(());
        };
        self.backend.refresh_server_state()?;
        self.backend.submit(&mut job)?;
        let _ = self.backend.notify_owner();
        self.wait_ddl_finished(cancellation, job.id)
    }
    /// 校验模式迁移合法性；同模式则返回 None（无需提交）。
    pub fn build_alter_table_mode_job(
        &self,
        target: &AlterTableModeTarget,
    ) -> Result<Option<AlterTableModeJob>, Error> {
        let (job, _, _) = astersql_ddl_jobsubmit::build_alter_table_mode_job(
            astersql_ddl_jobsubmit::SessionVariables::default(),
            astersql_ddl_jobsubmit::AlterTableModeTarget {
                schema_id: target.schema_id,
                table_id: target.table_id,
                schema_name: target.schema_name.clone(),
                table_name: target.table_name.clone(),
                current_mode: target.current_mode.jobsubmit_mode(),
                target_mode: target.target_mode.jobsubmit_mode(),
            },
        )
        .map_err(|error| Error(error.to_string()))?;
        let Some(mut job) = job else {
            return Ok(None);
        };
        let vars = self.backend.session_variables()?;
        job.cdc_write_source = vars.cdc_write_source;
        job.sql_mode = vars.sql_mode;
        Ok(Some(AlterTableModeJob {
            id: job.id,
            schema_id: job.schema_id,
            table_id: job.table_id,
            schema_name: job.schema_name,
            table_name: job.table_name,
            target_mode: target.target_mode,
            query: job.query,
            cdc_write_source: job.cdc_write_source,
            sql_mode: job.sql_mode,
        }))
    }

    /// 用后端元数据校验请求中的 schema/表名，并补齐 current_mode。
    pub fn resolve_alter_table_mode_target(
        &self,
        request: AlterTableModeTarget,
    ) -> Result<AlterTableModeTarget, Error> {
        let (database, table) = self
            .backend
            .resolve_metadata(request.schema_id, request.table_id)?;
        let database = database.ok_or_else(|| {
            Error(format!(
                "database does not exist (Schema ID {})",
                request.schema_id
            ))
        })?;
        let (table, mode) = table.ok_or_else(|| {
            Error(format!(
                "table does not exist (Schema ID {}, Table ID {})",
                request.schema_id, request.table_id
            ))
        })?;
        if database.to_lowercase() != request.schema_name.to_lowercase() {
            return Err(Error(format!(
                "expected schema name {} does not match target schema name {database}",
                request.schema_name
            )));
        }
        if table.to_lowercase() != request.table_name.to_lowercase() {
            return Err(Error(format!(
                "expected table name {} does not match target table name {table}",
                request.table_name
            )));
        }
        Ok(AlterTableModeTarget {
            current_mode: mode,
            ..request
        })
    }
    /// 轮询历史 Job 直至 Synced/Failed/Unexpected，或上下文取消。
    pub fn wait_ddl_finished(&self, cancellation: &Cancellation, job_id: i64) -> Result<(), Error> {
        // 直到取消或读到终态为止，按固定间隔拉取 history_job。
        loop {
            if cancellation.is_cancelled() {
                return Err(Error("context cancelled".into()));
            }
            std::thread::sleep(DDL_HISTORY_POLL_INTERVAL);
            match self.backend.history_job(job_id) {
                Err(_) => continue,
                Ok(None) => continue,
                Ok(Some(HistoryJobState::Synced)) => return Ok(()),
                Ok(Some(HistoryJobState::Failed(error))) => return Err(Error(error)),
                Ok(Some(HistoryJobState::Unexpected(state))) => {
                    return Err(Error(format!(
                        "target DDL job {job_id} finished in unexpected state {state}"
                    )));
                }
            }
        }
    }
}

/// Open a fresh target snapshot while retaining the shared target store.
pub type SnapshotProvider =
    Arc<dyn Fn() -> Result<Box<dyn astersql_kv::Snapshot>, Error> + Send + Sync>;
/// Read variables from a real borrowed target system session.
pub type SessionVariablesProvider = Arc<dyn Fn() -> Result<SessionVariables, Error> + Send + Sync>;
/// Refresh the public serverstate cache before enqueueing.
pub type ServerStateRefresh = Arc<dyn Fn() -> Result<(), Error> + Send + Sync>;

/// Go crossks submission adapter. It owns no election, scheduler or worker.
/// The caller supplies existing jobsubmit/session/systable components; target
/// owner startup remains a responsibility of the normal target service.
pub struct SubmitOnlyBackend {
    options: astersql_ddl_jobsubmit::SubmitOptions,
    snapshot: SnapshotProvider,
    variables: SessionVariablesProvider,
    refresh: ServerStateRefresh,
    notifier: Option<Arc<dyn astersql_ddl_jobsubmit::OwnerNotifier>>,
}
impl SubmitOnlyBackend {
    /// Assemble existing submission components; no background work is started.
    pub fn new(
        options: astersql_ddl_jobsubmit::SubmitOptions,
        snapshot: SnapshotProvider,
        variables: SessionVariablesProvider,
        refresh: ServerStateRefresh,
        notifier: Option<Arc<dyn astersql_ddl_jobsubmit::OwnerNotifier>>,
    ) -> Self {
        Self {
            options,
            snapshot,
            variables,
            refresh,
            notifier,
        }
    }
    fn reader(&self) -> Result<astersql_meta::SnapshotReader, Error> {
        (self.snapshot)().map(astersql_meta::SnapshotReader::new)
    }
    fn mode(mode: astersql_meta_model::TableMode) -> TableMode {
        match mode {
            astersql_meta_model::TableMode::TableModeNormal => TableMode::Normal,
            astersql_meta_model::TableMode::TableModeImport => TableMode::Import,
            astersql_meta_model::TableMode::TableModeRestore => TableMode::Restore,
        }
    }
}
impl DdlBackend for SubmitOnlyBackend {
    fn resolve_database(&self, id: i64) -> Result<Option<String>, Error> {
        self.reader()?
            .get_database(id)
            .map(|v| v.map(|db| db.Name.L))
            .map_err(|e| Error(e.to_string()))
    }
    fn resolve_table(&self, db: i64, id: i64) -> Result<Option<(String, TableMode)>, Error> {
        self.reader()?
            .get_table(db, id)
            .map(|v| v.map(|t| (t.Name.L, Self::mode(t.Mode))))
            .map_err(|e| Error(e.to_string()))
    }
    fn resolve_metadata(
        &self,
        db: i64,
        id: i64,
    ) -> Result<(Option<String>, Option<(String, TableMode)>), Error> {
        let reader = self.reader()?;
        let database = reader.get_database(db).map_err(|e| Error(e.to_string()))?;
        let table = if database.is_some() {
            reader.get_table(db, id).map_err(|e| Error(e.to_string()))?
        } else {
            None
        };
        Ok((
            database.map(|db| db.Name.L),
            table.map(|t| (t.Name.L, Self::mode(t.Mode))),
        ))
    }
    fn session_variables(&self) -> Result<SessionVariables, Error> {
        (self.variables)()
    }
    fn refresh_server_state(&self) -> Result<(), Error> {
        (self.refresh)()
    }
    fn submit(&self, job: &mut AlterTableModeJob) -> Result<(), Error> {
        use astersql_ddl_jobsubmit as submit;
        let mut spec = submit::JobSpec {
            job: submit::Job {
                version: 2,
                schema_id: job.schema_id,
                table_id: job.table_id,
                schema_name: job.schema_name.clone(),
                table_name: job.table_name.clone(),
                job_type: submit::JobType::AlterTableMode,
                query: job.query.clone(),
                binlog_info_present: true,
                cdc_write_source: job.cdc_write_source,
                sql_mode: job.sql_mode,
                involving_schemas: vec![(job.schema_name.clone(), job.table_name.clone())],
                ..Default::default()
            },
            args: submit::table_mode_args(submit::AlterTableModeArgs {
                table_mode: job.target_mode.jobsubmit_mode(),
                schema_id: job.schema_id,
                table_id: job.table_id,
            }),
            id_allocated: true,
        };
        submit::submit_batch(&self.options, std::slice::from_mut(&mut spec))
            .map_err(|e| Error(e.to_string()))?;
        job.id = spec.job.id;
        Ok(())
    }
    fn notify_owner(&self) -> Result<(), Error> {
        self.notifier
            .as_ref()
            .map_or(Ok(()), |n| n.notify().map_err(|e| Error(e.to_string())))
    }
    fn history_job(&self, id: i64) -> Result<Option<HistoryJobState>, Error> {
        let job = self
            .reader()?
            .get_history_ddl_job(id)
            .map_err(|e| Error(e.to_string()))?;
        Ok(job.map(|job| {
            if job.state == astersql_meta_model::group_3::JobState::Synced {
                HistoryJobState::Synced
            } else if let Some(error) = job.error {
                HistoryJobState::Failed(error)
            } else {
                HistoryJobState::Unexpected(job.state.to_string())
            }
        }))
    }
}

/// Go NotifyDDLOwnerByEtcd sends an advisory general-job notification.
/// A failed notification never rolls back an already committed job.
pub struct EtcdOwnerNotifier(pub Arc<dyn astersql_domain_serverinfo::EtcdClient>);
impl astersql_ddl_jobsubmit::OwnerNotifier for EtcdOwnerNotifier {
    fn notify(&self) -> Result<(), astersql_ddl_jobsubmit::Error> {
        self.0
            .Put(
                &astersql_domain_serverinfo::Context::Background(),
                "/tidb/ddl/add_ddl_job_general",
                b"0".to_vec(),
                None,
            )
            .map_err(|e| astersql_ddl_jobsubmit::Error {
                kind: astersql_ddl_jobsubmit::ErrorKind::Storage,
                message: e.to_string(),
            })
    }
}
