// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// `ADMIN SHOW DDL JOBS` 执行器：列出运行中与历史 DDL 作业及运维注释。
//
// DDL Job 描述一次 schema 变更；Reorg（重组）阶段可能回填索引数据。
// Comments 列汇总 analyze 状态、ingest/DXF/cloud 与并发等运维标签。

#![allow(non_snake_case)]

use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// DDL 动作元数据：动作名与若干分类标志（改名、多 schema、加索引等）。
pub struct DDLAction {
    pub name: String,
    pub is_rename_table: bool,
    pub is_multi_schema_change: bool,
    pub is_add_index: bool,
    pub is_add_primary_key: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Reorg 完成后 ANALYZE（统计信息收集）的状态。
pub enum AnalyzeState {
    None,
    Running,
    Failed,
    Timeout,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 重组（Reorg）实现类型：事务内回填、Ingest 摄取、TxnMerge 等。
pub enum ReorgType {
    None,
    Txn(String),
    Ingest(String),
    TxnMerge(String),
    Other(String),
}

impl ReorgType {
    /// 返回展示用标签；None 表示无重组类型。
    fn label(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::Txn(label) | Self::Ingest(label) | Self::TxnMerge(label) | Self::Other(label) => {
                Some(label)
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 作业级重组元数据：analyze 状态、分布式/云存储开关与并发参数。
pub struct ReorgMeta {
    pub analyze_state: AnalyzeState,
    pub reorg_type: ReorgType,
    pub is_dist_reorg: bool,
    pub use_cloud_storage: bool,
    pub concurrency: usize,
    pub batch_size: usize,
    pub max_write_speed: u64,
    pub target_scope: String,
    pub max_node_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 多 schema 变更中的子作业快照。
pub struct SubJob {
    pub action: DDLAction,
    pub schema_state: String,
    pub row_count: i64,
    pub real_start_ts: u64,
    pub state: String,
    pub reorg_type: ReorgType,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 作业 binlog 侧完成信息（完成时间戳与最终库表名）。
pub struct BinlogInfo {
    pub finished_ts: u64,
    pub table_name: Option<String>,
    pub multiple_table_names: Option<Vec<String>>,
    pub database_name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 一条 DDL 作业的完整展示模型。
pub struct DDLJob {
    pub id: i64,
    pub schema_name: String,
    pub table_name: String,
    pub schema_id: i64,
    pub table_id: i64,
    pub row_count: i64,
    pub start_ts: u64,
    pub real_start_ts: u64,
    pub action: DDLAction,
    pub schema_state: String,
    pub state: String,
    pub query: String,
    pub binlog_info: Option<BinlogInfo>,
    pub rename_new_table_name: Option<String>,
    pub reorg_meta: Option<ReorgMeta>,
    pub sub_jobs: Vec<SubJob>,
    pub may_need_reorg: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 从 WHERE 提取的列谓词，用于跳过运行中/历史作业或按库表过滤。
pub struct DDLJobPredicates {
    pub column_predicates: HashMap<String, HashSet<String>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 运行时默认重组参数与是否 next_gen 架构。
pub struct DDLRuntimeConfig {
    pub next_gen: bool,
    pub default_reorg_worker_count: usize,
    pub default_reorg_batch_size: usize,
    pub default_reorg_max_write_speed: u64,
}

/// SHOW DDL JOBS 结果 Chunk 写出接口。
pub trait ShowDDLChunk {
    type Time;

    fn grow_and_reset(&mut self, max_chunk_size: usize);
    fn capacity(&self) -> usize;
    fn append_i64(&mut self, column: usize, value: i64);
    fn append_string(&mut self, column: usize, value: &str);
    fn append_time(&mut self, column: usize, value: &Self::Time);
    fn append_null(&mut self, column: usize);
}

/// 按活跃角色校验是否有权查看指定库表的作业。
pub trait DDLPrivilegeChecker<R> {
    fn request_verification(&self, active_roles: &[R], schema_name: &str, table_name: &str)
    -> bool;
}

/// 历史作业迭代器：按需拉取最近若干条。
pub trait LastJobIterator<J, E> {
    fn get_last_jobs(&mut self, count: usize, cache: Vec<J>) -> Result<Vec<J>, E>;
}

/// Production boundary for system sessions, transactions, DDL/meta, and
/// InfoSchema. No method has a disconnected default implementation.
/// 生产环境边界：系统会话、事务、DDL/元数据与 InfoSchema；方法均需显式实现。
pub trait ShowDDLJobsBackend {
    type Context;
    type Error;
    type Session;
    type Transaction;
    type Time;
    type Role: Clone;
    type HistoryIterator: LastJobIterator<DDLJob, Self::Error>;

    fn open_base(&mut self, context: &Self::Context) -> Result<(), Self::Error>;
    fn close_base(&mut self) -> Result<(), Self::Error>;
    fn max_chunk_size(&self) -> usize;
    fn default_history_job_count(&self) -> usize;
    fn runtime_config(&self) -> DDLRuntimeConfig;

    fn get_system_session(&mut self) -> Result<Self::Session, Self::Error>;
    fn release_system_session(&mut self, session: Self::Session);
    fn new_transaction(&mut self, session: &mut Self::Session) -> Result<(), Self::Error>;
    fn transaction(
        &mut self,
        session: &mut Self::Session,
        active: bool,
    ) -> Result<Self::Transaction, Self::Error>;
    fn set_in_transaction(&mut self, session: &mut Self::Session, value: bool);

    fn running_jobs(&mut self, session: &Self::Session) -> Result<Vec<DDLJob>, Self::Error>;
    fn history_iterator(
        &mut self,
        transaction: Self::Transaction,
        schema_names: HashSet<String>,
        table_names: HashSet<String>,
    ) -> Result<Self::HistoryIterator, Self::Error>;

    fn schema_name_by_id(&self, id: i64) -> Option<String>;
    fn table_name_by_id(&self, id: i64) -> Option<String>;
    fn timestamp_to_time(&self, timestamp: u64) -> Self::Time;
}

/// 作业检索器：持有运行中列表、历史迭代器、游标与权限角色。
pub struct DDLJobRetriever<I, T, R> {
    pub runningJobs: Vec<DDLJob>,
    pub historyJobIter: Option<I>,
    pub cursor: usize,
    pub activeRoles: Vec<R>,
    pub cacheJobs: Vec<DDLJob>,
    pub TZLoc: T,
    pub extractor: Option<DDLJobPredicates>,
}

impl<I, T, R> DDLJobRetriever<I, T, R> {
    /// 按谓词决定是否加载运行中/历史作业，并初始化游标。
    fn initial<B>(
        &mut self,
        backend: &mut B,
        transaction: B::Transaction,
        session: &B::Session,
    ) -> Result<(), B::Error>
    where
        B: ShowDDLJobsBackend<HistoryIterator = I, Time = T, Role = R>,
    {
        let mut skip_running_jobs = false;
        let mut skip_history_jobs = false;
        let mut schema_names = HashSet::new();
        let mut table_names = HashSet::new();

        // 根据 state / db_name / table_name 谓词决定跳过哪些作业源。
        if let Some(extractor) = self.extractor.as_ref() {
            if let Some(states) = extractor.column_predicates.get("state") {
                skip_history_jobs = true;
                skip_running_jobs = true;
                for state in states {
                    match state.to_lowercase().as_str() {
                        "cancelled" | "synced" => skip_history_jobs = false,
                        _ => skip_running_jobs = false,
                    }
                }
            }
            schema_names = extractor
                .column_predicates
                .get("db_name")
                .cloned()
                .unwrap_or_default();
            table_names = extractor
                .column_predicates
                .get("table_name")
                .cloned()
                .unwrap_or_default();
        }

        if !skip_running_jobs {
            self.runningJobs = backend.running_jobs(session)?;
        }
        if !skip_history_jobs {
            self.historyJobIter =
                Some(backend.history_iterator(transaction, schema_names, table_names)?);
        }
        self.cursor = 0;
        Ok(())
    }

    /// 将单条作业（及多 schema 子作业）编码为结果行，含权限过滤。
    fn appendJobToChunk<B, C>(
        &self,
        backend: &B,
        request: &mut C,
        job: &DDLJob,
        checker: Option<&dyn DDLPrivilegeChecker<R>>,
        in_show_statement: bool,
    ) where
        B: ShowDDLJobsBackend<HistoryIterator = I, Time = T, Role = R>,
        C: ShowDDLChunk<Time = T>,
    {
        // 优先从 binlog 信息恢复最终库表名与完成时间戳。
        let mut schema_name = job.schema_name.clone();
        let mut table_name = String::new();
        let mut finish_ts = 0;
        if let Some(binlog) = job.binlog_info.as_ref() {
            finish_ts = binlog.finished_ts;
            if let Some(name) = binlog.table_name.as_ref() {
                table_name.clone_from(name);
            } else if job.action.is_rename_table
                && let Some(name) = job.rename_new_table_name.as_ref()
                && !name.is_empty()
            {
                table_name.clone_from(name);
            }
            if let Some(names) = binlog.multiple_table_names.as_ref() {
                table_name = names.join(",");
            }
            if schema_name.is_empty()
                && let Some(name) = binlog.database_name.as_ref()
            {
                schema_name.clone_from(name);
            }
        }
        if table_name.is_empty() {
            table_name.clone_from(&job.table_name);
        }
        if schema_name.is_empty() {
            schema_name = getSchemaName(backend, job.schema_id);
        }
        if table_name.is_empty() {
            table_name = getTableName(backend, job.table_id);
        }

        if let Some(checker) = checker
            && !checker.request_verification(
                &self.activeRoles,
                &schema_name.to_lowercase(),
                &table_name.to_lowercase(),
            )
        {
            return;
        }

        let create_time = ts2Time(backend, job.start_ts);
        let start_time = ts2Time(backend, job.real_start_ts);
        let finish_time = ts2Time(backend, finish_ts);
        appendCommonJobColumns(
            request,
            job.id,
            &schema_name,
            &table_name,
            &job.action.name,
            &job.schema_state,
            job.schema_id,
            job.table_id,
            job.row_count,
            &create_time,
            (job.real_start_ts > 0).then_some(&start_time),
            (finish_ts > 0).then_some(&finish_time),
            &job.state,
        );

        // 多 schema 变更时先为每个子作业追加一行，再写父作业 Comments。
        if job.action.is_multi_schema_change {
            let use_dxf = job
                .reorg_meta
                .as_ref()
                .is_some_and(|meta| meta.is_dist_reorg);
            let use_cloud = job
                .reorg_meta
                .as_ref()
                .is_some_and(|meta| meta.use_cloud_storage);
            for sub_job in &job.sub_jobs {
                let sub_start_time = ts2Time(backend, sub_job.real_start_ts);
                appendCommonJobColumns(
                    request,
                    job.id,
                    &schema_name,
                    &table_name,
                    &format!("{} /* subjob */", sub_job.action.name),
                    &sub_job.schema_state,
                    job.schema_id,
                    job.table_id,
                    sub_job.row_count,
                    &create_time,
                    (sub_job.real_start_ts > 0).then_some(&sub_start_time),
                    (finish_ts > 0).then_some(&finish_time),
                    &sub_job.state,
                );
                let comments = if in_show_statement {
                    showCommentsFromSubjob(sub_job, use_dxf, use_cloud, &backend.runtime_config())
                } else {
                    job.query.clone()
                };
                request.append_string(12, &comments);
            }
        }
        let comments = if in_show_statement {
            showCommentsFromJob(job, &backend.runtime_config())
        } else {
            job.query.clone()
        };
        request.append_string(12, &comments);
    }
}

/// `ADMIN SHOW DDL JOBS` 执行器：系统会话事务中分页写出作业。
pub struct ShowDDLJobsExec<B: ShowDDLJobsBackend> {
    pub BaseExecutor: B,
    pub DDLJobRetriever: DDLJobRetriever<B::HistoryIterator, B::Time, B::Role>,
    pub jobNumber: usize,
    pub sess: Option<B::Session>,
}

impl<B: ShowDDLJobsBackend> ShowDDLJobsExec<B> {
    /// 打开：获取系统会话、开启事务并初始化检索器。
    pub fn Open(&mut self, context: &B::Context) -> Result<(), B::Error> {
        self.BaseExecutor.open_base(context)?;
        if self.jobNumber == 0 {
            self.jobNumber = self.BaseExecutor.default_history_job_count();
        }
        self.sess = Some(self.BaseExecutor.get_system_session()?);
        let session = self
            .sess
            .as_mut()
            .expect("the acquired system session is retained until Close");
        self.BaseExecutor.new_transaction(session)?;
        let transaction = self.BaseExecutor.transaction(session, true)?;
        self.BaseExecutor.set_in_transaction(session, true);
        let result = self
            .DDLJobRetriever
            .initial(&mut self.BaseExecutor, transaction, session);
        result
    }

    /// 先输出运行中作业，再按 jobNumber 补齐历史作业。
    pub fn Next<C: ShowDDLChunk<Time = B::Time>>(
        &mut self,
        _context: &B::Context,
        request: &mut C,
    ) -> Result<(), B::Error> {
        request.grow_and_reset(self.BaseExecutor.max_chunk_size());
        let running_count = self.DDLJobRetriever.runningJobs.len();
        if self.DDLJobRetriever.cursor.saturating_sub(running_count) >= self.jobNumber {
            return Ok(());
        }
        // 运行中作业写完后再从历史迭代器补齐剩余配额。
        let mut count = 0;
        if self.DDLJobRetriever.cursor < running_count {
            let batch = request
                .capacity()
                .min(running_count - self.DDLJobRetriever.cursor);
            for index in self.DDLJobRetriever.cursor..self.DDLJobRetriever.cursor + batch {
                self.DDLJobRetriever.appendJobToChunk(
                    &self.BaseExecutor,
                    request,
                    &self.DDLJobRetriever.runningJobs[index],
                    None,
                    true,
                );
            }
            self.DDLJobRetriever.cursor += batch;
            count += batch;
        }
        if count < request.capacity()
            && let Some(iterator) = self.DDLJobRetriever.historyJobIter.as_mut()
        {
            let remaining = self
                .jobNumber
                .saturating_sub(self.DDLJobRetriever.cursor.saturating_sub(running_count));
            let number = (request.capacity() - count).min(remaining);
            self.DDLJobRetriever.cacheJobs = iterator
                .get_last_jobs(number, std::mem::take(&mut self.DDLJobRetriever.cacheJobs))?;
            for job in &self.DDLJobRetriever.cacheJobs {
                self.DDLJobRetriever
                    .appendJobToChunk(&self.BaseExecutor, request, job, None, true);
            }
            self.DDLJobRetriever.cursor += self.DDLJobRetriever.cacheJobs.len();
        }
        Ok(())
    }

    /// 释放系统会话并关闭基类执行器。
    pub fn Close(&mut self) -> Result<(), B::Error> {
        if let Some(session) = self.sess.take() {
            self.BaseExecutor.release_system_session(session);
        }
        self.BaseExecutor.close_base()
    }
}

#[allow(clippy::too_many_arguments)]
/// 写入作业公共列（ID、库表、动作、schema 状态、时间与 state）。
fn appendCommonJobColumns<C: ShowDDLChunk>(
    request: &mut C,
    job_id: i64,
    schema_name: &str,
    table_name: &str,
    action: &str,
    schema_state: &str,
    schema_id: i64,
    table_id: i64,
    row_count: i64,
    create_time: &C::Time,
    start_time: Option<&C::Time>,
    finish_time: Option<&C::Time>,
    state: &str,
) {
    request.append_i64(0, job_id);
    request.append_string(1, schema_name);
    request.append_string(2, table_name);
    request.append_string(3, action);
    request.append_string(4, schema_state);
    request.append_i64(5, schema_id);
    request.append_i64(6, table_id);
    request.append_i64(7, row_count);
    request.append_time(8, create_time);
    match start_time {
        Some(time) => request.append_time(9, time),
        None => request.append_null(9),
    }
    match finish_time {
        Some(time) => request.append_time(10, time),
        None => request.append_null(10),
    }
    request.append_string(11, state);
}

/// 由作业 ReorgMeta 生成 Comments 列标签（analyze / ingest / 并发等）。
pub fn showCommentsFromJob(job: &DDLJob, config: &DDLRuntimeConfig) -> String {
    let Some(meta) = job.reorg_meta.as_ref() else {
        return String::new();
    };
    let mut labels = Vec::new();
    match meta.analyze_state {
        AnalyzeState::Running => labels.push("analyzing".to_owned()),
        AnalyzeState::Failed => labels.push("analyze_failed".to_owned()),
        AnalyzeState::Timeout => labels.push("analyze_timeout".to_owned()),
        AnalyzeState::None => {}
    }
    let adding_index = job.action.is_add_index || job.action.is_add_primary_key;
    // next_gen 下加索引只保留 analyze 相关标签，跳过 ingest/DXF 等。
    if adding_index && config.next_gen {
        return labels.join(", ");
    }
    if adding_index {
        match &meta.reorg_type {
            ReorgType::Txn(label) | ReorgType::TxnMerge(label) => labels.push(label.clone()),
            ReorgType::Ingest(label) => {
                labels.push(label.clone());
                if meta.is_dist_reorg {
                    labels.push("DXF".to_owned());
                }
                if meta.use_cloud_storage {
                    labels.push("cloud".to_owned());
                }
            }
            ReorgType::None | ReorgType::Other(_) => {}
        }
    }
    if job.may_need_reorg {
        if meta.concurrency != config.default_reorg_worker_count {
            labels.push(format!("thread={}", meta.concurrency));
        }
        if meta.batch_size != config.default_reorg_batch_size {
            labels.push(format!("batch_size={}", meta.batch_size));
        }
        if meta.max_write_speed != config.default_reorg_max_write_speed {
            labels.push(format!("max_write_speed={}", meta.max_write_speed));
        }
        if !meta.target_scope.is_empty() {
            labels.push(format!("service_scope={}", meta.target_scope));
        }
        if meta.max_node_count != 0 {
            labels.push(format!("max_node_count={}", meta.max_node_count));
        }
    }
    labels.join(", ")
}

/// 由子作业重组类型生成 Comments；next_gen 下返回空串。
pub fn showCommentsFromSubjob(
    sub_job: &SubJob,
    use_dxf: bool,
    use_cloud: bool,
    config: &DDLRuntimeConfig,
) -> String {
    if config.next_gen {
        return String::new();
    }
    let Some(label) = sub_job.reorg_type.label() else {
        return String::new();
    };
    let mut labels = vec![label.to_owned()];
    if use_dxf {
        labels.push("DXF".to_owned());
    }
    if use_dxf && use_cloud {
        labels.push("cloud".to_owned());
    }
    labels.join(", ")
}

/// 将 TSO/时间戳转换为后端时间类型。
pub fn ts2Time<B: ShowDDLJobsBackend>(backend: &B, timestamp: u64) -> B::Time {
    backend.timestamp_to_time(timestamp)
}

/// 按 schema_id 解析库名，缺失时返回空串。
pub fn getSchemaName<B: ShowDDLJobsBackend>(backend: &B, id: i64) -> String {
    backend.schema_name_by_id(id).unwrap_or_default()
}

/// 按 table_id 解析表名，缺失时返回空串。
pub fn getTableName<B: ShowDDLJobsBackend>(backend: &B, id: i64) -> String {
    backend.table_name_by_id(id).unwrap_or_default()
}
