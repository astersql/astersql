// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 导入任务（import job）持久化与状态管理。
//
// 操作 `mysql.tidb_import_jobs` 系统表，覆盖任务创建、启动、步进、
// 完成、失败与取消；并按用户/SUPER 权限过滤可见任务。

use std::fmt;
use std::sync::atomic::{AtomicI64, Ordering};

use astersql_types::time::Time;

use crate::{ImportParameters, Summary};

/// 测试用：记录最近一次 CreateJob 返回的 job ID。
pub static TestLastImportJobID: AtomicI64 = AtomicI64::new(0);

/// 任务状态：待启动。
pub const jobStatusPending: &str = "pending";
/// 任务状态：运行中。
pub const JobStatusRunning: &str = "running";
/// 任务状态：已取消（历史拼写 jog 与 Go 侧保持一致）。
pub const jogStatusCancelled: &str = "cancelled";
/// 任务状态：失败。
pub const jobStatusFailed: &str = "failed";
/// 任务状态：成功完成。
pub const JobStatusFinished: &str = "finished";
/// 步骤占位：无具体步骤（完成或未开始时）。
pub const jobStepNone: &str = "";
/// 步骤：准备阶段（解析文件、估算大小等）。
pub const JobStepPreparing: &str = "preparing";
/// 步骤：全局排序（cloud storage 路径）。
pub const JobStepGlobalSorting: &str = "global-sorting";
/// 步骤：导入写入。
pub const JobStepImporting: &str = "importing";
/// 步骤：冲突解析。
pub const JobStepResolvingConflicts: &str = "resolving-conflicts";
/// 步骤：校验（如 checksum）。
pub const JobStepValidating: &str = "validating";

/// 查询 import job 全字段的基础 SELECT。
pub const baseQuerySQL: &str = r#"SELECT
                id, create_time, start_time, update_time, end_time,
                table_schema, table_name, table_id, created_by, parameters, source_file_size,
                status, step, summary, error_message, group_key
            FROM mysql.tidb_import_jobs"#;

#[derive(Clone, Debug, PartialEq)]
/// SQL 绑定参数的通用取值包装。
pub enum JobValue {
    /// SQL NULL。
    Null,
    /// 有符号整数。
    Int(i64),
    /// 无符号整数。
    UInt(u64),
    /// 字符串。
    String(String),
    /// 原始字节（JSON 等）。
    Bytes(Vec<u8>),
}

/// 从 i64 构造整型绑定值。
impl From<i64> for JobValue {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

/// 从字符串切片构造绑定值。
impl From<&str> for JobValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

/// 从拥有字符串构造绑定值。
impl From<String> for JobValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

/// 从字节向量构造绑定值（参数/摘要 JSON）。
impl From<Vec<u8>> for JobValue {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(value)
    }
}

/// 单行查询结果的列访问抽象。
pub trait ImportJobRow {
    /// 第 index 列是否为 NULL。
    fn IsNull(&self, index: usize) -> bool;
    /// 读取第 index 列为 i64。
    fn Int64(&self, index: usize) -> Result<i64, String>;
    /// 读取第 index 列为字符串。
    fn String(&self, index: usize) -> Result<String, String>;
    /// 读取第 index 列为时间。
    fn Time(&self, index: usize) -> Result<Time, String>;
}

/// 执行 import job 相关 SQL 的会话抽象。
pub trait ImportJobExecutor {
    /// 执行无结果集 SQL。
    fn ExecuteInternal(&mut self, sql: &str, arguments: Vec<JobValue>) -> Result<(), String>;
    /// 执行查询并返回行集；expected_columns 用于校验列数。
    fn QueryInternal(
        &mut self,
        sql: &str,
        arguments: Vec<JobValue>,
        expected_columns: usize,
    ) -> Result<Vec<Box<dyn ImportJobRow>>, String>;
}

/// ImportParameters / Summary 的编解码抽象。
pub trait ImportJobCodec {
    /// 编码导入参数。
    fn EncodeParameters(&self, parameters: &ImportParameters) -> Result<Vec<u8>, String>;
    /// 解码导入参数。
    fn DecodeParameters(&self, bytes: &[u8]) -> Result<ImportParameters, String>;
    /// 编码执行摘要。
    fn EncodeSummary(&self, summary: &Summary) -> Result<Vec<u8>, String>;
    /// 解码执行摘要。
    fn DecodeSummary(&self, bytes: &[u8]) -> Result<Summary, String>;
}

#[derive(Clone, Debug, Default)]
/// 导入任务的完整元信息，对应 `tidb_import_jobs` 一行。
pub struct JobInfo {
    /// 任务 ID。
    pub ID: i64,
    /// 创建时间。
    pub CreateTime: Time,
    /// 开始运行时间（未启动则为零值）。
    pub StartTime: Time,
    /// 最近更新时间。
    pub UpdateTime: Time,
    /// 结束时间（未结束则为零值）。
    pub EndTime: Time,
    /// 目标库名。
    pub TableSchema: String,
    /// 目标表名。
    pub TableName: String,
    /// 目标表 ID。
    pub TableID: i64,
    /// 创建者用户名。
    pub CreatedBy: String,
    /// 导入参数快照。
    pub Parameters: ImportParameters,
    /// 源文件总大小（字节）；准备前可能未知。
    pub SourceFileSize: i64,
    /// 任务状态字符串。
    pub Status: String,
    /// 当前步骤字符串。
    pub Step: String,
    /// 执行摘要；未完成时可能为空。
    pub Summary: Option<Box<Summary>>,
    /// 失败/取消时的错误信息。
    pub ErrorMessage: String,
    /// 任务分组键，便于批量查询。
    pub GroupKey: String,
}

impl JobInfo {
    /// 仅 pending/running 状态允许取消。
    pub fn CanCancel(&self) -> bool {
        self.Status == jobStatusPending || self.Status == JobStatusRunning
    }

    /// 是否已成功结束。
    pub fn IsSuccess(&self) -> bool {
        self.Status == JobStatusFinished
    }

    /// 源文件大小是否尚未确定（pending 或 preparing 阶段）。
    pub fn IsSourceFileSizeUnknown(&self) -> bool {
        if self.SourceFileSize > 0 {
            return false;
        }
        self.Status == jobStatusPending
            || (self.Status == JobStatusRunning && self.Step == JobStepPreparing)
    }
}

/// 按 ID 查询任务；无 SUPER 时只能查看本人创建的任务。
pub fn GetJob(
    executor: &mut dyn ImportJobExecutor,
    codec: &dyn ImportJobCodec,
    job_id: i64,
    user: &str,
    has_super_privilege: bool,
) -> Result<Box<JobInfo>, String> {
    let rows = executor.QueryInternal(
        &format!("{baseQuerySQL} WHERE id = %?"),
        vec![job_id.into()],
        16,
    )?;
    if rows.len() != 1 {
        return Err(format!("import job {job_id} not found"));
    }
    // 无 SUPER 权限时校验创建者与当前用户一致。
    let information = convert2JobInfo(rows[0].as_ref(), codec)?;
    if !has_super_privilege && information.CreatedBy != user {
        return Err("SUPER privilege is required to view another user's import job".into());
    }
    Ok(information)
}

/// 统计指定表上 pending/running 的活跃任务数。
pub fn GetActiveJobCnt(
    executor: &mut dyn ImportJobExecutor,
    table_schema: &str,
    table_name: &str,
) -> Result<i64, String> {
    let rows = executor.QueryInternal(
        r#"select count(1) from mysql.tidb_import_jobs
        where status in (%?, %?)
            and table_schema = %? and table_name = %?;"#,
        vec![
            jobStatusPending.into(),
            JobStatusRunning.into(),
            table_schema.into(),
            table_name.into(),
        ],
        1,
    )?;
    let row = rows
        .first()
        .ok_or_else(|| "active import job count returned no row".to_owned())?;
    row.Int64(0)
}

/// 插入新任务并返回 LAST_INSERT_ID。
pub fn CreateJob(
    executor: &mut dyn ImportJobExecutor,
    codec: &dyn ImportJobCodec,
    database: &str,
    table: &str,
    table_id: i64,
    user: &str,
    group_key: &str,
    parameters: &ImportParameters,
    source_file_size: i64,
) -> Result<i64, String> {
    // 先序列化参数再插入，最后取自增 ID。
    let parameters = codec.EncodeParameters(parameters)?;
    executor.ExecuteInternal(
        r#"INSERT INTO mysql.tidb_import_jobs
            (table_schema, table_name, table_id, group_key, created_by, parameters, source_file_size, status, step)
            VALUES (%?, %?, %?, %?, %?, %?, %?, %?, %?);"#,
        vec![
            database.into(),
            table.into(),
            table_id.into(),
            group_key.into(),
            user.into(),
            parameters.into(),
            source_file_size.into(),
            jobStatusPending.into(),
            jobStepNone.into(),
        ],
    )?;
    let rows = executor.QueryInternal("SELECT LAST_INSERT_ID();", Vec::new(), 1)?;
    if rows.len() != 1 {
        return Err(format!(
            "unexpected LAST_INSERT_ID result length {}",
            rows.len()
        ));
    }
    let job_id = rows[0].Int64(0)?;
    TestLastImportJobID.store(job_id, Ordering::SeqCst);
    Ok(job_id)
}

/// 将 pending 任务置为 running，并记录 start/update 时间与步骤。
pub fn StartJob(
    executor: &mut dyn ImportJobExecutor,
    job_id: i64,
    step: &str,
) -> Result<(), String> {
    executor.ExecuteInternal(
        r#"UPDATE mysql.tidb_import_jobs
            SET update_time = CURRENT_TIMESTAMP(6), start_time = CURRENT_TIMESTAMP(6), status = %?, step = %?
            WHERE id = %? AND status = %?;"#,
        vec![
            JobStatusRunning.into(),
            step.into(),
            job_id.into(),
            jobStatusPending.into(),
        ],
    )
}

/// 更新运行中任务的当前步骤。
pub fn Job2Step(
    executor: &mut dyn ImportJobExecutor,
    job_id: i64,
    step: &str,
) -> Result<(), String> {
    executor.ExecuteInternal(
        r#"UPDATE mysql.tidb_import_jobs
            SET update_time = CURRENT_TIMESTAMP(6), step = %?
            WHERE id = %? AND status = %?;"#,
        vec![step.into(), job_id.into(), JobStatusRunning.into()],
    )
}

/// 准备阶段结束后回写源文件大小与检测到的格式。
pub fn UpdateJobPreparedInfo(
    executor: &mut dyn ImportJobExecutor,
    codec: &dyn ImportJobCodec,
    job_id: i64,
    source_file_size: i64,
    format: &str,
) -> Result<(), String> {
    let rows = executor.QueryInternal(
        r#"SELECT parameters FROM mysql.tidb_import_jobs
            WHERE id = %? AND status = %?;"#,
        vec![job_id.into(), JobStatusRunning.into()],
        1,
    )?;
    // 任务不存在或状态已变时静默成功（与 Go 一致）。
    let Some(row) = rows.first() else {
        return Ok(());
    };
    let encoded = row.String(0)?;
    let mut parameters = if encoded.is_empty() {
        ImportParameters::default()
    } else {
        codec.DecodeParameters(encoded.as_bytes())?
    };
    if !format.is_empty() {
        parameters.Format = format.to_owned();
    }
    executor.ExecuteInternal(
        r#"UPDATE mysql.tidb_import_jobs
            SET update_time = CURRENT_TIMESTAMP(6), source_file_size = %?, parameters = %?
            WHERE id = %? AND status = %?;"#,
        vec![
            source_file_size.into(),
            codec.EncodeParameters(&parameters)?.into(),
            job_id.into(),
            JobStatusRunning.into(),
        ],
    )
}

/// 将运行中任务标记为 finished，写入摘要与结束时间。
pub fn FinishJob(
    executor: &mut dyn ImportJobExecutor,
    codec: &dyn ImportJobCodec,
    job_id: i64,
    summary: Option<&Summary>,
) -> Result<(), String> {
    let summary = summary.map_or_else(|| Ok(b"{}".to_vec()), |value| codec.EncodeSummary(value))?;
    executor.ExecuteInternal(
        r#"UPDATE mysql.tidb_import_jobs
            SET update_time = CURRENT_TIMESTAMP(6), end_time = CURRENT_TIMESTAMP(6), status = %?, step = %?, summary = %?
            WHERE id = %? AND status = %?;"#,
        vec![
            JobStatusFinished.into(),
            jobStepNone.into(),
            summary.into(),
            job_id.into(),
            JobStatusRunning.into(),
        ],
    )
}

/// 将 pending/running 任务标记为 failed，写入错误信息与摘要。
pub fn FailJob(
    executor: &mut dyn ImportJobExecutor,
    codec: &dyn ImportJobCodec,
    job_id: i64,
    error_message: &str,
    summary: Option<&Summary>,
) -> Result<(), String> {
    let summary = summary.map_or_else(|| Ok(b"{}".to_vec()), |value| codec.EncodeSummary(value))?;
    executor.ExecuteInternal(
        r#"UPDATE mysql.tidb_import_jobs
            SET update_time = CURRENT_TIMESTAMP(6), end_time = CURRENT_TIMESTAMP(6), status = %?, error_message = %?, summary = %?
            WHERE id = %? AND status IN (%?, %?);"#,
        vec![
            jobStatusFailed.into(),
            error_message.into(),
            summary.into(),
            job_id.into(),
            jobStatusPending.into(),
            JobStatusRunning.into(),
        ],
    )
}

/// 将查询行转换为 JobInfo，处理可空时间/摘要字段。
fn convert2JobInfo(
    row: &dyn ImportJobRow,
    codec: &dyn ImportJobCodec,
) -> Result<Box<JobInfo>, String> {
    // 可空时间列：NULL 映射为零值 Time。
    let zero_time = Time::default();
    let start_time = if row.IsNull(2) {
        zero_time
    } else {
        row.Time(2)?
    };
    let update_time = if row.IsNull(3) {
        Time::default()
    } else {
        row.Time(3)?
    };
    let end_time = if row.IsNull(4) {
        Time::default()
    } else {
        row.Time(4)?
    };
    let parameters = codec.DecodeParameters(row.String(9)?.as_bytes())?;
    let summary_text = if row.IsNull(13) {
        String::new()
    } else {
        row.String(13)?
    };
    let summary = if summary_text.is_empty() {
        None
    } else {
        Some(Box::new(codec.DecodeSummary(summary_text.as_bytes())?))
    };
    Ok(Box::new(JobInfo {
        ID: row.Int64(0)?,
        CreateTime: row.Time(1)?,
        StartTime: start_time,
        UpdateTime: update_time,
        EndTime: end_time,
        TableSchema: row.String(5)?,
        TableName: row.String(6)?,
        TableID: row.Int64(7)?,
        CreatedBy: row.String(8)?,
        Parameters: parameters,
        SourceFileSize: row.Int64(10)?,
        Status: row.String(11)?,
        Step: row.String(12)?,
        Summary: summary,
        ErrorMessage: if row.IsNull(14) {
            String::new()
        } else {
            row.String(14)?
        },
        GroupKey: row.String(15)?,
    }))
}

/// 执行查询并将所有行转为 JobInfo 列表。
fn getJobInfoFromSQL(
    executor: &mut dyn ImportJobExecutor,
    codec: &dyn ImportJobCodec,
    sql: &str,
    arguments: Vec<JobValue>,
) -> Result<Vec<Box<JobInfo>>, String> {
    executor
        .QueryInternal(sql, arguments, 16)?
        .iter()
        .map(|row| convert2JobInfo(row.as_ref(), codec))
        .collect()
}

/// 按 group_key（及可选创建者）过滤任务列表。
pub fn GetJobsByGroupKey(
    executor: &mut dyn ImportJobExecutor,
    codec: &dyn ImportJobCodec,
    user: &str,
    group_key: &str,
    has_super_privilege: bool,
) -> Result<Vec<Box<JobInfo>>, String> {
    // 动态拼接 WHERE：非 SUPER 限制创建者；空 group_key 表示非空分组。
    let mut clauses = Vec::new();
    let mut arguments = Vec::new();
    if !has_super_privilege {
        clauses.push("created_by = %?");
        arguments.push(user.into());
    }
    if group_key.is_empty() {
        clauses.push("group_key != ''");
    } else {
        clauses.push("group_key = %?");
        arguments.push(group_key.into());
    }
    getJobInfoFromSQL(
        executor,
        codec,
        &format!("{baseQuerySQL} WHERE {}", clauses.join(" AND ")),
        arguments,
    )
}

/// 返回当前用户可见的全部任务；SUPER 可见所有。
pub fn GetAllViewableJobs(
    executor: &mut dyn ImportJobExecutor,
    codec: &dyn ImportJobCodec,
    user: &str,
    has_super_privilege: bool,
) -> Result<Vec<Box<JobInfo>>, String> {
    if has_super_privilege {
        getJobInfoFromSQL(executor, codec, baseQuerySQL, Vec::new())
    } else {
        getJobInfoFromSQL(
            executor,
            codec,
            &format!("{baseQuerySQL} WHERE created_by = %?"),
            vec![user.into()],
        )
    }
}

/// 取消 pending/running 任务，写入 cancelled 状态与固定错误文案。
pub fn CancelJob(executor: &mut dyn ImportJobExecutor, job_id: i64) -> Result<(), String> {
    executor.ExecuteInternal(
        r#"UPDATE mysql.tidb_import_jobs
        SET update_time = CURRENT_TIMESTAMP(6), status = %?, error_message = 'cancelled by user'
        WHERE id = %? AND status IN (%?, %?);"#,
        vec![
            jogStatusCancelled.into(),
            job_id.into(),
            jobStatusPending.into(),
            JobStatusRunning.into(),
        ],
    )
}

fn write_json_string(formatter: &mut fmt::Formatter<'_>, value: &str) -> fmt::Result {
    formatter.write_str("\"")?;
    for character in value.chars() {
        match character {
            '\"' => formatter.write_str("\\\"")?,
            '\\' => formatter.write_str("\\\\")?,
            '\u{08}' => formatter.write_str("\\b")?,
            '\u{0c}' => formatter.write_str("\\f")?,
            '\n' => formatter.write_str("\\n")?,
            '\r' => formatter.write_str("\\r")?,
            '\t' => formatter.write_str("\\t")?,
            character if character <= '\u{1f}' => write!(formatter, "\\u{:04x}", character as u32)?,
            character => write!(formatter, "{character}")?,
        }
    }
    formatter.write_str("\"")
}

/// 与 Go 的 `json.Marshal` 一致，将导入参数格式化为 JSON。
impl fmt::Display for ImportParameters {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("{")?;
        let mut has_field = false;
        for (name, value, omit_empty) in [
            ("columns-and-vars", self.ColumnsAndVars.as_str(), true),
            ("set-clause", self.SetClause.as_str(), true),
            ("file-location", self.FileLocation.as_str(), false),
            ("format", self.Format.as_str(), false),
        ] {
            if omit_empty && value.is_empty() {
                continue;
            }
            if has_field {
                formatter.write_str(",")?;
            }
            has_field = true;
            write_json_string(formatter, name)?;
            formatter.write_str(":")?;
            write_json_string(formatter, value)?;
        }
        if !self.Options.is_empty() {
            if has_field {
                formatter.write_str(",")?;
            }
            formatter.write_str("\"options\":{")?;
            let mut options: Vec<_> = self.Options.iter().collect();
            options.sort_unstable_by(|left, right| left.0.cmp(right.0));
            for (index, (name, value)) in options.into_iter().enumerate() {
                if index > 0 {
                    formatter.write_str(",")?;
                }
                write_json_string(formatter, name)?;
                formatter.write_str(":")?;
                write_json_string(formatter, value)?;
            }
            formatter.write_str("}")?;
        }
        formatter.write_str("}")
    }
}
