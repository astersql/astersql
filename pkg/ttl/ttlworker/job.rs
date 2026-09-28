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

// TTL（Time To Live，按过期时间自动清理行）作业的内存模型与收尾逻辑。
//
// 本模块对应 Go 版 ttlworker 中的 job 相关结构：描述一次表级 TTL 清理作业、
// 作业历史记录，以及在本地 `JobStore` 中登记/完成作业的辅助方法。
// 持久化侧通过 `FINISH_JOB_SQL` / `REMOVE_TASK_FOR_JOB_SQL` 更新系统表
// `mysql.tidb_ttl_table_status` 与 `mysql.tidb_ttl_task`。

use crate::job_manager::TtlSummary;
use crate::session::PhysicalTable;

/// 将当前作业写回系统表并清空 current_job_* 字段的 UPDATE SQL 模板。
/// `%?` 为占位符：完成时间、汇总 JSON、物理表 ID、作业 ID。
pub const FINISH_JOB_SQL: &str = "UPDATE mysql.tidb_ttl_table_status SET last_job_id=current_job_id,last_job_start_time=current_job_start_time,last_job_finish_time=%?,last_job_ttl_expire=current_job_ttl_expire,last_job_summary=%?,current_job_id=NULL,current_job_owner_id=NULL,current_job_owner_hb_time=NULL,current_job_start_time=NULL,current_job_ttl_expire=NULL,current_job_state=NULL,current_job_status=NULL,current_job_status_update_time=NULL WHERE table_id=%? AND current_job_id=%?";
/// 删除某作业下全部扫描子任务记录的 DELETE SQL 模板。
pub const REMOVE_TASK_FOR_JOB_SQL: &str = "DELETE FROM mysql.tidb_ttl_task WHERE job_id = %?";
/// 创建 TTL job history 行；`finish_time` 按 Go 实现固定为 Unix epoch 后一秒。
pub const CREATE_JOB_HISTORY_SQL: &str = "INSERT INTO mysql.tidb_ttl_job_history (job_id,table_id,parent_table_id,table_schema,table_name,partition_name,create_time,finish_time,ttl_expire,status) VALUES (%?,%?,%?,%?,%?,%?,%?,FROM_UNIXTIME(1),%?,%?)";
/// 完成 TTL job history 行并写入逐类行数与状态。
pub const FINISH_JOB_HISTORY_SQL: &str = "UPDATE mysql.tidb_ttl_job_history SET finish_time=%?,summary_text=%?,expired_rows=%?,deleted_rows=%?,error_delete_rows=%?,status=%? WHERE job_id=%?";

/// TTL job SQL 构造器使用的参数类型，保留 Go `[]any` 中的 NULL/整数/字符串语义。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobSqlValue {
    Null,
    Integer(i64),
    Unsigned(u64),
    String(String),
}

pub fn finish_job_sql(
    table_id: i64,
    finish_time: impl Into<String>,
    summary: impl Into<String>,
    job_id: impl Into<String>,
) -> (&'static str, Vec<JobSqlValue>) {
    (
        FINISH_JOB_SQL,
        vec![
            JobSqlValue::String(finish_time.into()),
            JobSqlValue::String(summary.into()),
            JobSqlValue::Integer(table_id),
            JobSqlValue::String(job_id.into()),
        ],
    )
}

pub fn remove_task_for_job(job_id: impl Into<String>) -> (&'static str, Vec<JobSqlValue>) {
    (
        REMOVE_TASK_FOR_JOB_SQL,
        vec![JobSqlValue::String(job_id.into())],
    )
}

pub fn create_job_history_sql(
    job_id: impl Into<String>,
    table: &PhysicalTable,
    partition_name: Option<&str>,
    expire_time: impl Into<String>,
    create_time: impl Into<String>,
) -> (&'static str, Vec<JobSqlValue>) {
    (
        CREATE_JOB_HISTORY_SQL,
        vec![
            JobSqlValue::String(job_id.into()),
            JobSqlValue::Integer(table.physical_id),
            JobSqlValue::Integer(table.table_id),
            JobSqlValue::String(table.schema.clone()),
            JobSqlValue::String(table.table.clone()),
            partition_name.map_or(JobSqlValue::Null, |name| JobSqlValue::String(name.into())),
            JobSqlValue::String(create_time.into()),
            JobSqlValue::String(expire_time.into()),
            JobSqlValue::String("running".into()),
        ],
    )
}

pub fn finish_job_history_sql(
    job_id: impl Into<String>,
    finish_time: impl Into<String>,
    summary_text: impl Into<String>,
    summary: &TtlSummary,
) -> (&'static str, Vec<JobSqlValue>) {
    (
        FINISH_JOB_HISTORY_SQL,
        vec![
            JobSqlValue::String(finish_time.into()),
            JobSqlValue::String(summary_text.into()),
            JobSqlValue::Unsigned(summary.total_rows),
            JobSqlValue::Unsigned(summary.success_rows),
            JobSqlValue::Unsigned(summary.error_rows),
            JobSqlValue::String("finished".into()),
            JobSqlValue::String(job_id.into()),
        ],
    )
}

/// 一次正在（或刚）运行的 TTL 清理作业。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TtlJob {
    /// 作业唯一 ID。
    pub id: String,
    /// 当前持有该作业的 worker 节点 ID。
    pub owner_id: String,
    /// 目标物理表（分区表时 physical_id 指向分区）。
    pub table: PhysicalTable,
    /// 作业创建时间（Unix 秒）。
    pub create_time: u64,
    /// 本轮清理使用的过期水位：列值早于该时间的行视为过期。
    pub expire_time: u64,
    /// 是否已在本地标记完成。
    pub finished: bool,
}

/// 作业完成后写入历史的快照，供查询与 GC（垃圾回收）保留策略使用。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobHistory {
    /// 作业 ID。
    pub job_id: String,
    /// 物理表 ID（分区 ID 或非分区表 ID）。
    pub table_id: i64,
    /// 逻辑表 ID（分区表的父表 ID）。
    pub parent_table_id: i64,
    /// schema（库）名。
    pub table_schema: String,
    /// 表名。
    pub table_name: String,
    /// 分区名；非分区表为 `None`。
    pub partition_name: Option<String>,
    /// 创建时间。
    pub create_time: u64,
    /// 完成时间；未完成时为 `None`。
    pub finish_time: Option<u64>,
    /// 本轮使用的过期水位。
    pub expire_time: u64,
    /// 扫描/删除汇总统计。
    pub summary: Option<TtlSummary>,
}

/// 本地作业状态仓：活跃作业、每作业任务数、历史记录。
#[derive(Clone, Debug, Default)]
pub struct JobStore {
    /// 按物理表 ID 索引的当前活跃作业。
    pub active_jobs: std::collections::BTreeMap<i64, TtlJob>,
    /// 每个作业 ID 对应的扫描子任务总数。
    pub tasks_by_job: std::collections::BTreeMap<String, usize>,
    /// 按作业 ID 索引的历史记录。
    pub history: std::collections::BTreeMap<String, JobHistory>,
}

impl TtlJob {
    /// 在 `store.history` 中登记一条尚未完成的历史条目。
    pub fn create_history(&self, store: &mut JobStore) {
        store.history.insert(
            self.id.clone(),
            JobHistory {
                job_id: self.id.clone(),
                table_id: self.table.physical_id,
                parent_table_id: self.table.table_id,
                table_schema: self.table.schema.clone(),
                table_name: self.table.table.clone(),
                partition_name: None,
                create_time: self.create_time,
                finish_time: None,
                expire_time: self.expire_time,
                summary: None,
            },
        );
    }
    /// 完成作业：仅当本作业仍是该物理表上的活跃所有者时才生效。
    ///
    /// 成功时从活跃表与任务计数中移除，回填历史的完成时间与汇总，并标记 `finished`。
    pub fn finish(&mut self, store: &mut JobStore, now: u64, summary: TtlSummary) -> bool {
        // 校验：当前活跃作业仍是本 job，避免误完成已被抢占/替换的作业。
        let owned = store
            .active_jobs
            .get(&self.table.physical_id)
            .is_some_and(|job| job.id == self.id);
        if !owned {
            return false;
        }
        store.active_jobs.remove(&self.table.physical_id);
        store.tasks_by_job.remove(&self.id);
        if let Some(history) = store.history.get_mut(&self.id) {
            history.finish_time = Some(now);
            history.summary = Some(summary);
        }
        self.finished = true;
        true
    }
}
