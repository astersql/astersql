// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 导入作业（Import Job）管理：提交、查询状态、取消，以及按组汇总。
//
// 通过 `JobDatabase`/`JobRows` 抽象执行 `IMPORT`/`SHOW IMPORT`/`CANCEL IMPORT`
// 等 SQL，并将结果行解码为 `JobStatus`/`GroupStatus`。对应 Go 侧基于 `*sql.DB`
// 的作业管理逻辑。

use crate::{ErrInvalidOptions, ErrJobNotFound, ErrNoJobIDReturned, GroupStatus, JobStatus};
use astersql_errors as errors;
use chrono::{NaiveDate, NaiveDateTime};
use std::any::Any;
use std::sync::Arc;

/// `SHOW IMPORT` 时间列的解析格式（与 Go 侧 layout 一致）。
const TIME_LAYOUT: &str = "%Y-%m-%d %H:%M:%S";

/// SQL 查询结果单元格的简化取值：NULL、整型或字符串。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SQLValue {
    Null,
    Int64(i64),
    String(String),
}

/// 导入作业相关查询的结果行迭代器（类似 `sql.Rows`）。
pub trait JobRows: Send {
    /// 取下一行；无更多行时返回 `Ok(None)`。
    fn Next(&mut self) -> Result<Option<Vec<SQLValue>>, errors::SharedError>;
    /// 关闭底层游标/连接资源。
    fn Close(&mut self) -> Result<(), errors::SharedError>;
}

/// 执行导入相关 SQL 的数据库抽象（查询与执行）。
pub trait JobDatabase: Send + Sync {
    /// 执行查询并返回可迭代的结果行。
    fn QueryContext(
        &self,
        ctx: &(dyn Any + Send + Sync),
        query: &str,
    ) -> Result<Box<dyn JobRows>, errors::SharedError>;

    /// 执行无结果集语句（如 CANCEL）。
    fn ExecContext(
        &self,
        ctx: &(dyn Any + Send + Sync),
        query: &str,
    ) -> Result<(), errors::SharedError>;
}

/// 导入作业生命周期管理接口：提交、查状态、取消、按组查询。
pub trait JobManager: Send + Sync {
    /// 提交导入 SQL，返回新建作业的 job id。
    fn SubmitJob(
        &self,
        ctx: &(dyn Any + Send + Sync),
        query: &str,
    ) -> Result<i64, errors::SharedError>;
    /// 保留 Go 双返回值形状，供需要同时观察 job id 与错误的调用方使用。
    fn SubmitJobParts(
        &self,
        ctx: &(dyn Any + Send + Sync),
        query: &str,
    ) -> (i64, Option<errors::SharedError>) {
        match self.SubmitJob(ctx, query) {
            Ok(job_id) => (job_id, None),
            Err(error) => (0, Some(error)),
        }
    }
    /// 查询指定作业的当前状态。
    fn GetJobStatus(
        &self,
        ctx: &(dyn Any + Send + Sync),
        job_id: i64,
    ) -> Result<JobStatus, errors::SharedError>;
    fn GetJobStatusParts(
        &self,
        ctx: &(dyn Any + Send + Sync),
        job_id: i64,
    ) -> (Option<JobStatus>, Option<errors::SharedError>) {
        match self.GetJobStatus(ctx, job_id) {
            Ok(value) => (Some(value), None),
            Err(error) => (None, Some(error)),
        }
    }
    /// 取消指定导入作业。
    fn CancelJob(
        &self,
        ctx: &(dyn Any + Send + Sync),
        job_id: i64,
    ) -> Result<(), errors::SharedError>;
    /// 按 GroupKey 查询作业组汇总状态。
    fn GetGroupSummary(
        &self,
        ctx: &(dyn Any + Send + Sync),
        group_key: &str,
    ) -> Result<GroupStatus, errors::SharedError>;
    fn GetGroupSummaryParts(
        &self,
        ctx: &(dyn Any + Send + Sync),
        group_key: &str,
    ) -> (Option<GroupStatus>, Option<errors::SharedError>) {
        match self.GetGroupSummary(ctx, group_key) {
            Ok(value) => (Some(value), None),
            Err(error) => (None, Some(error)),
        }
    }
    /// 列出同一 GroupKey 下的全部作业状态。
    fn GetJobsByGroup(
        &self,
        ctx: &(dyn Any + Send + Sync),
        group_key: &str,
    ) -> Result<Vec<JobStatus>, errors::SharedError>;
    fn GetJobsByGroupParts(
        &self,
        ctx: &(dyn Any + Send + Sync),
        group_key: &str,
    ) -> (Option<Vec<JobStatus>>, Option<errors::SharedError>) {
        match self.GetJobsByGroup(ctx, group_key) {
            Ok(value) => (Some(value), None),
            Err(error) => (None, Some(error)),
        }
    }
}

/// 基于 `JobDatabase` 的默认作业管理实现。
pub struct JobManagerImpl {
    db: Arc<dyn JobDatabase>,
}

/// 用给定数据库抽象构造 `JobManagerImpl`。
pub fn NewJobManager(db: Arc<dyn JobDatabase>) -> JobManagerImpl {
    JobManagerImpl { db }
}

impl JobManagerImpl {
    /// 执行查询并解码恰好一行；无行时返回 `missing`；优先保留业务错误再关闭游标。
    fn queryOne<T>(
        &self,
        ctx: &(dyn Any + Send + Sync),
        query: &str,
        missing: &errors::SharedError,
        decode: impl FnOnce(&[SQLValue]) -> Result<T, errors::SharedError>,
    ) -> Result<T, errors::SharedError> {
        let mut rows = self.db.QueryContext(ctx, query)?;
        let result = match rows.Next() {
            Ok(Some(row)) => decode(&row),
            Ok(None) => Err(missing.clone()),
            Err(error) => Err(error),
        };
        // Go 的 defer rows.Close() 始终执行但忽略 Close 错误；确保 Next
        // 失败时也释放游标，不让关闭错误改变查询语义。
        let _ = rows.Close();
        result
    }
}

impl JobManager for JobManagerImpl {
    fn SubmitJob(
        &self,
        ctx: &(dyn Any + Send + Sync),
        query: &str,
    ) -> Result<i64, errors::SharedError> {
        // Submit 复用 SHOW IMPORT 行布局，只取 JobID；空结果视为未返回 id。
        self.queryOne(ctx, query, &ErrNoJobIDReturned, scanJobStatus)
            .map(|status| status.JobID)
    }

    fn GetJobStatus(
        &self,
        ctx: &(dyn Any + Send + Sync),
        job_id: i64,
    ) -> Result<JobStatus, errors::SharedError> {
        self.queryOne(
            ctx,
            &format!("SHOW IMPORT JOB {job_id}"),
            &ErrJobNotFound,
            scanJobStatus,
        )
    }

    fn CancelJob(
        &self,
        ctx: &(dyn Any + Send + Sync),
        job_id: i64,
    ) -> Result<(), errors::SharedError> {
        self.db
            .ExecContext(ctx, &format!("CANCEL IMPORT JOB {job_id}"))
    }

    fn GetGroupSummary(
        &self,
        ctx: &(dyn Any + Send + Sync),
        group_key: &str,
    ) -> Result<GroupStatus, errors::SharedError> {
        // 空 group_key 非法；单引号按 SQL 字面量规则加倍转义。
        if group_key.is_empty() {
            return Err((*ErrInvalidOptions).clone());
        }
        let escaped = group_key.replace('\'', "''");
        self.queryOne(
            ctx,
            &format!("SHOW IMPORT GROUP '{escaped}'"),
            &ErrJobNotFound,
            scanGroupStatus,
        )
    }

    fn GetJobsByGroup(
        &self,
        ctx: &(dyn Any + Send + Sync),
        group_key: &str,
    ) -> Result<Vec<JobStatus>, errors::SharedError> {
        // 与 GetGroupSummary 相同的空键校验与引号转义。
        if group_key.is_empty() {
            return Err((*ErrInvalidOptions).clone());
        }
        let escaped = group_key.replace('\'', "''");
        let mut rows = self.db.QueryContext(
            ctx,
            &format!("SHOW IMPORT JOBS WHERE GROUP_KEY = '{escaped}'"),
        )?;
        let mut jobs = Vec::new();
        // 逐行解码；空结果集合法（返回空 Vec，不是 ErrJobNotFound）。
        let result = loop {
            match rows.Next() {
                Ok(Some(row)) => match scanJobStatus(&row) {
                    Ok(status) => jobs.push(status),
                    Err(error) => break Err(error),
                },
                Ok(None) => break Ok(jobs),
                Err(error) => break Err(error),
            }
        };
        // 与 Go 的 defer rows.Close() 对齐：始终收尾，但忽略关闭错误。
        let _ = rows.Close();
        result
    }
}

/// 将 `SHOW IMPORT JOB` 的 21 列结果解码为 `JobStatus`。
fn scanJobStatus(row: &[SQLValue]) -> Result<JobStatus, errors::SharedError> {
    requireColumns(row, 21, "SHOW IMPORT JOB")?;
    Ok(JobStatus {
        JobID: requiredInt(row, 0, "job_id")?,
        GroupKey: nullableString(row, 1, "group_key")?,
        DataSource: requiredString(row, 2, "data_source")?,
        TargetTable: requiredString(row, 3, "target_table")?,
        TableID: requiredInt(row, 4, "table_id")?,
        Phase: requiredString(row, 5, "phase")?,
        Status: requiredString(row, 6, "status")?,
        SourceFileSize: requiredString(row, 7, "source_file_size")?,
        ImportedRows: nullableInt(row, 8, "imported_rows")?,
        ResultMessage: nullableString(row, 9, "result_message")?,
        CreateTime: parseTime(&requiredString(row, 10, "create_time")?),
        StartTime: parseTime(&nullableString(row, 11, "start_time")?),
        EndTime: parseTime(&nullableString(row, 12, "end_time")?),
        CreatedBy: requiredString(row, 13, "created_by")?,
        UpdateTime: parseTime(&nullableString(row, 14, "update_time")?),
        Step: nullableString(row, 15, "step")?,
        ProcessedSize: nullableString(row, 16, "processed_size")?,
        TotalSize: nullableString(row, 17, "total_size")?,
        Percent: nullableString(row, 18, "percent")?,
        Speed: nullableString(row, 19, "speed")?,
        ETA: nullableString(row, 20, "eta")?,
    })
}

/// 将 `SHOW IMPORT GROUP` 的 9 列结果解码为 `GroupStatus`。
fn scanGroupStatus(row: &[SQLValue]) -> Result<GroupStatus, errors::SharedError> {
    requireColumns(row, 9, "SHOW IMPORT GROUP")?;
    Ok(GroupStatus {
        GroupKey: requiredString(row, 0, "group_key")?,
        TotalJobs: requiredInt(row, 1, "total_jobs")?,
        Pending: requiredInt(row, 2, "pending")?,
        Running: requiredInt(row, 3, "running")?,
        Completed: requiredInt(row, 4, "completed")?,
        Failed: requiredInt(row, 5, "failed")?,
        Cancelled: requiredInt(row, 6, "cancelled")?,
        FirstJobCreateTime: parseTime(&nullableString(row, 7, "first_job_create_time")?),
        LastJobUpdateTime: parseTime(&nullableString(row, 8, "last_job_update_time")?),
    })
}

/// 断言结果行列数与期望一致，否则带上 SQL 来源名报错。
fn requireColumns(
    row: &[SQLValue],
    expected: usize,
    source: &str,
) -> Result<(), errors::SharedError> {
    if row.len() == expected {
        Ok(())
    } else {
        Err(errors::New(format!(
            "{source} returned {} columns, expected {expected}",
            row.len()
        )))
    }
}

/// 读取必填字符串列；类型不符则报错。
fn requiredString(
    row: &[SQLValue],
    index: usize,
    name: &str,
) -> Result<String, errors::SharedError> {
    match &row[index] {
        SQLValue::String(value) => Ok(value.clone()),
        value => Err(errors::New(format!(
            "column {name} must be string, got {value:?}"
        ))),
    }
}

/// 读取可空字符串列；NULL 映射为空串。
fn nullableString(
    row: &[SQLValue],
    index: usize,
    name: &str,
) -> Result<String, errors::SharedError> {
    match &row[index] {
        SQLValue::Null => Ok(String::new()),
        SQLValue::String(value) => Ok(value.clone()),
        value => Err(errors::New(format!(
            "column {name} must be string or NULL, got {value:?}"
        ))),
    }
}

/// 读取必填 int64 列。
fn requiredInt(row: &[SQLValue], index: usize, name: &str) -> Result<i64, errors::SharedError> {
    match row[index] {
        SQLValue::Int64(value) => Ok(value),
        ref value => Err(errors::New(format!(
            "column {name} must be int64, got {value:?}"
        ))),
    }
}

/// 读取可空 int64 列；NULL 映射为 0。
fn nullableInt(row: &[SQLValue], index: usize, name: &str) -> Result<i64, errors::SharedError> {
    match row[index] {
        SQLValue::Null => Ok(0),
        SQLValue::Int64(value) => Ok(value),
        ref value => Err(errors::New(format!(
            "column {name} must be int64 or NULL, got {value:?}"
        ))),
    }
}

/// 按 `TIME_LAYOUT` 解析时间；失败时回退为 `NaiveDateTime::MIN`。
fn parseTime(value: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(value, TIME_LAYOUT).unwrap_or_else(|_| zeroTime())
}

/// Go's zero `time.Time` is year 1, unlike chrono's representable minimum
/// year (which is far before year 1).
fn zeroTime() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(1, 1, 1)
        .expect("year 1 is a valid Gregorian date")
        .and_hms_opt(0, 0, 0)
        .expect("midnight is a valid time")
}
