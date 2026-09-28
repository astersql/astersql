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

// 说明：Go 用 sqlmock；Rust 用 JobDatabase trait + CanonicalDatabase 等价覆盖。
// NOTE: the Go tests drive `*sql.DB` through `github.com/DATA-DOG/go-sqlmock`.
// The Rust production code instead abstracts the database behind the
// `JobDatabase`/`JobRows` traits (see job_manager.rs), so these tests provide
// an equivalent in-process mock (`CanonicalDatabase`) that programs query
// results and errors exactly like the Go sqlmock expectations, preserving the
// same case-by-case coverage as the Go table.

// `JobManagerImpl` 的单元测试。
//
// 用进程内 `CanonicalDatabase` 模拟 Go sqlmock：预设查询/执行响应，
// 覆盖 SubmitJob、GetJobStatus、CancelJob、GetGroupSummary、GetJobsByGroup
// 的成功、空结果与错误分支，并校验生成的 SQL 文本。

use crate::{
    ErrInvalidOptions, ErrJobNotFound, JobDatabase, JobManager, JobRows, NewJobManager, SQLValue,
};
use astersql_errors as errors;
use std::any::Any;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// 预设的查询响应：结果行或错误。
enum QueryResponse {
    Rows(Vec<Vec<SQLValue>>),
    Error(errors::SharedError),
}

/// 预设的执行响应：成功或错误。
enum ExecResponse {
    Ok,
    Error(errors::SharedError),
}

/// 可弹出的内存结果行集合，实现 `JobRows`。
struct CanonicalRows {
    rows: VecDeque<Vec<SQLValue>>,
}

impl JobRows for CanonicalRows {
    fn Next(&mut self) -> Result<Option<Vec<SQLValue>>, errors::SharedError> {
        Ok(self.rows.pop_front())
    }

    fn Close(&mut self) -> Result<(), errors::SharedError> {
        Ok(())
    }
}

#[derive(Default)]
/// 记录实际 SQL，并按队列吐出预设的 Query/Exec 响应。
struct CanonicalDatabase {
    queries: Mutex<Vec<String>>,
    executions: Mutex<Vec<String>>,
    query_responses: Mutex<VecDeque<QueryResponse>>,
    exec_responses: Mutex<VecDeque<ExecResponse>>,
}

impl CanonicalDatabase {
    /// 排队一组查询结果行。
    fn push_rows(&self, rows: Vec<Vec<SQLValue>>) {
        self.query_responses
            .lock()
            .unwrap()
            .push_back(QueryResponse::Rows(rows));
    }

    /// 排队一次查询错误。
    fn push_query_error(&self, error: errors::SharedError) {
        self.query_responses
            .lock()
            .unwrap()
            .push_back(QueryResponse::Error(error));
    }

    /// 排队一次执行成功。
    fn push_exec_ok(&self) {
        self.exec_responses
            .lock()
            .unwrap()
            .push_back(ExecResponse::Ok);
    }

    /// 排队一次执行错误。
    fn push_exec_error(&self, error: errors::SharedError) {
        self.exec_responses
            .lock()
            .unwrap()
            .push_back(ExecResponse::Error(error));
    }
}

impl JobDatabase for CanonicalDatabase {
    fn QueryContext(
        &self,
        _ctx: &(dyn Any + Send + Sync),
        query: &str,
    ) -> Result<Box<dyn JobRows>, errors::SharedError> {
        // 记录 SQL，再弹出队首响应；无预设时默认空结果集。
        self.queries.lock().unwrap().push(query.to_owned());
        match self
            .query_responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(QueryResponse::Rows(Vec::new()))
        {
            QueryResponse::Rows(rows) => Ok(Box::new(CanonicalRows { rows: rows.into() })),
            QueryResponse::Error(error) => Err(error),
        }
    }

    fn ExecContext(
        &self,
        _ctx: &(dyn Any + Send + Sync),
        query: &str,
    ) -> Result<(), errors::SharedError> {
        // 记录 SQL，再弹出队首响应；无预设时默认 Ok。
        self.executions.lock().unwrap().push(query.to_owned());
        match self
            .exec_responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(ExecResponse::Ok)
        {
            ExecResponse::Ok => Ok(()),
            ExecResponse::Error(error) => Err(error),
        }
    }
}

/// 构造与 `scanJobStatus` 一致的 21 列测试行。
/// job_status_row mirrors the 21-column `SHOW IMPORT JOB` row shape shared by
/// every Go sqlmock fixture in this file (`scanJobStatus`'s expected layout).
fn job_status_row(id: i64, group: &str, table: &str, status: &str) -> Vec<SQLValue> {
    use SQLValue::{Int64, Null, String as Text};
    vec![
        Int64(id),
        Text(group.to_owned()),
        Text("s3://bucket/file.csv".to_owned()),
        Text(table.to_owned()),
        Int64(1),
        Text("import".to_owned()),
        Text(status.to_owned()),
        Text("100MB".to_owned()),
        Int64(1000),
        Text("success".to_owned()),
        Text("2023-01-01 10:00:00".to_owned()),
        Text("2023-01-01 10:00:01".to_owned()),
        Text("2023-01-01 10:00:02".to_owned()),
        Text("user".to_owned()),
        Text("2023-01-01 10:00:02".to_owned()),
        Null,
        Text("100MB".to_owned()),
        Text("100MB".to_owned()),
        Text("100%".to_owned()),
        Text("10MB/s".to_owned()),
        Text("0s".to_owned()),
    ]
}

/// 模拟底层连接已关闭的错误。
fn conn_done() -> errors::SharedError {
    errors::New("sql: connection is already closed")
}

/// 对照 Go TestSubmitJob：成功取 id、空结果、查询错误。
/// Mirrors Go's `TestSubmitJob`: success returns the scanned job id, an empty
/// result set maps to `ErrNoJobIDReturned`, and a query error propagates.
#[test]
fn submit_job_matches_go_cases() {
    let db = Arc::new(CanonicalDatabase::default());
    let manager = NewJobManager(db.clone());
    let sql_query = "IMPORT INTO ...";

    db.push_rows(vec![job_status_row(123, "", "db.table", "finished")]);
    assert_eq!(123, manager.SubmitJob(&(), sql_query).unwrap());

    db.push_rows(vec![]);
    let error = manager.SubmitJob(&(), sql_query).unwrap_err();
    assert!(error.to_string().contains("no job id returned"));

    db.push_query_error(conn_done());
    assert!(manager.SubmitJob(&(), sql_query).is_err());
}

/// 对照 Go TestGetJobStatus：成功、未找到、错误，并校验 SHOW SQL。
/// Mirrors Go's `TestGetJobStatus`: success, not-found and error branches for
/// `SHOW IMPORT JOB <id>`.
#[test]
fn get_job_status_matches_go_cases() {
    let db = Arc::new(CanonicalDatabase::default());
    let manager = NewJobManager(db.clone());
    let job_id = 123_i64;

    db.push_rows(vec![job_status_row(job_id, "", "db.table", "finished")]);
    let status = manager.GetJobStatus(&(), job_id).unwrap();
    assert_eq!(job_id, status.JobID);
    assert_eq!("finished", status.Status);

    db.push_rows(vec![]);
    let error = manager.GetJobStatus(&(), job_id).unwrap_err();
    assert!(errors::ErrorEqual(Some(&error), Some(&ErrJobNotFound)));

    db.push_query_error(conn_done());
    assert!(manager.GetJobStatus(&(), job_id).is_err());

    assert_eq!(
        vec![
            format!("SHOW IMPORT JOB {job_id}"),
            format!("SHOW IMPORT JOB {job_id}"),
            format!("SHOW IMPORT JOB {job_id}"),
        ],
        *db.queries.lock().unwrap()
    );
}

/// Go's invalid/empty time parse returns `time.Time{}` (year 1), not chrono's
/// global minimum date.
#[test]
fn invalid_job_times_use_go_zero_time() {
    let db = Arc::new(CanonicalDatabase::default());
    let manager = NewJobManager(db.clone());
    let mut row = job_status_row(123, "", "db.table", "finished");
    row[10] = SQLValue::String(String::new());
    row[11] = SQLValue::Null;
    db.push_rows(vec![row]);
    let status = manager.GetJobStatus(&(), 123).unwrap();
    assert_eq!(
        "0001-01-01 00:00:00",
        status.CreateTime.format("%Y-%m-%d %H:%M:%S").to_string()
    );
    assert_eq!(
        "0001-01-01 00:00:00",
        status.StartTime.format("%Y-%m-%d %H:%M:%S").to_string()
    );
}

/// 对照 Go TestCancelJob：成功与执行错误。
/// Mirrors Go's `TestCancelJob`: success and underlying exec error.
#[test]
fn cancel_job_matches_go_cases() {
    let db = Arc::new(CanonicalDatabase::default());
    let manager = NewJobManager(db.clone());
    let job_id = 123_i64;

    db.push_exec_ok();
    manager.CancelJob(&(), job_id).unwrap();

    db.push_exec_error(conn_done());
    assert!(manager.CancelJob(&(), job_id).is_err());

    assert_eq!(
        vec![
            format!("CANCEL IMPORT JOB {job_id}"),
            format!("CANCEL IMPORT JOB {job_id}"),
        ],
        *db.executions.lock().unwrap()
    );
}

/// 对照 Go TestGetGroupSummary：成功字段、空键拒绝、未找到、查询错误。
/// Mirrors Go's `TestGetGroupSummary`: success with full field verification,
/// empty group key rejected before any query, not-found and query error.
#[test]
fn get_group_summary_matches_go_cases() {
    let db = Arc::new(CanonicalDatabase::default());
    let manager = NewJobManager(db.clone());
    let group_key = "test_group";

    db.push_rows(vec![vec![
        SQLValue::String(group_key.to_owned()),
        SQLValue::Int64(10),
        SQLValue::Int64(1),
        SQLValue::Int64(2),
        SQLValue::Int64(3),
        SQLValue::Int64(2),
        SQLValue::Int64(2),
        SQLValue::String("2023-01-01 10:00:00".to_owned()),
        SQLValue::String("2023-01-01 12:00:00".to_owned()),
    ]]);
    let summary = manager.GetGroupSummary(&(), group_key).unwrap();
    assert_eq!(group_key, summary.GroupKey);
    assert_eq!(10, summary.TotalJobs);
    assert_eq!(1, summary.Pending);
    assert_eq!(2, summary.Running);
    assert_eq!(3, summary.Completed);
    assert_eq!(2, summary.Failed);
    assert_eq!(2, summary.Cancelled);

    let error = manager.GetGroupSummary(&(), "").unwrap_err();
    assert!(errors::ErrorEqual(Some(&error), Some(&ErrInvalidOptions)));

    db.push_rows(vec![]);
    let error = manager.GetGroupSummary(&(), group_key).unwrap_err();
    assert!(errors::ErrorEqual(Some(&error), Some(&ErrJobNotFound)));

    db.push_query_error(conn_done());
    assert!(manager.GetGroupSummary(&(), group_key).is_err());

    assert_eq!(
        vec![
            "SHOW IMPORT GROUP 'test_group'".to_owned(),
            "SHOW IMPORT GROUP 'test_group'".to_owned(),
            "SHOW IMPORT GROUP 'test_group'".to_owned(),
        ],
        *db.queries.lock().unwrap()
    );
}

/// 对照 Go TestGetJobsByGroup：多行顺序、空键、空列表合法、引号转义。
/// Mirrors Go's `TestGetJobsByGroup`: two rows preserve order, empty group key
/// is rejected up-front, an empty result set is a valid empty list (not
/// `ErrJobNotFound`), and query errors propagate. Also exercises the group
/// key quote-escaping used when building the `SHOW IMPORT JOBS` statement.
#[test]
fn get_jobs_by_group_matches_go_cases() {
    let db = Arc::new(CanonicalDatabase::default());
    let manager = NewJobManager(db.clone());
    let group_key = "test_group";

    db.push_rows(vec![
        job_status_row(1, group_key, "db.t1", "finished"),
        job_status_row(2, group_key, "db.t2", "running"),
    ]);
    let jobs = manager.GetJobsByGroup(&(), group_key).unwrap();
    assert_eq!(2, jobs.len());
    assert_eq!(1, jobs[0].JobID);
    assert_eq!("finished", jobs[0].Status);
    assert_eq!(2, jobs[1].JobID);
    assert_eq!("running", jobs[1].Status);

    let error = manager.GetJobsByGroup(&(), "").unwrap_err();
    assert!(errors::ErrorEqual(Some(&error), Some(&ErrInvalidOptions)));

    db.push_rows(vec![]);
    let jobs = manager.GetJobsByGroup(&(), group_key).unwrap();
    assert!(jobs.is_empty());

    db.push_query_error(conn_done());
    assert!(manager.GetJobsByGroup(&(), group_key).is_err());

    // Quoting: a group key containing a single quote must be escaped by
    // doubling it, matching Go's `strings.ReplaceAll(groupKey, "'", "''")`.
    db.push_rows(vec![job_status_row(1, "group'1", "db.t1", "finished")]);
    manager.GetJobsByGroup(&(), "group'1").unwrap();

    assert_eq!(
        vec![
            "SHOW IMPORT JOBS WHERE GROUP_KEY = 'test_group'".to_owned(),
            "SHOW IMPORT JOBS WHERE GROUP_KEY = 'test_group'".to_owned(),
            "SHOW IMPORT JOBS WHERE GROUP_KEY = 'test_group'".to_owned(),
            "SHOW IMPORT JOBS WHERE GROUP_KEY = 'group''1'".to_owned(),
        ],
        *db.queries.lock().unwrap()
    );
}
