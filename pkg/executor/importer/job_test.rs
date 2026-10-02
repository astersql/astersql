// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

use super::*;
use astersql_types::time::Time;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::Ordering;

#[derive(Clone)]
enum Cell {
    Null,
    Int(i64),
    Text(String),
    Time,
}
struct Row(Vec<Cell>);

impl ImportJobRow for Row {
    fn IsNull(&self, index: usize) -> bool {
        matches!(self.0[index], Cell::Null)
    }
    fn Int64(&self, index: usize) -> Result<i64, String> {
        match &self.0[index] {
            Cell::Int(v) => Ok(*v),
            _ => Err(format!("column {index} is not an integer")),
        }
    }
    fn String(&self, index: usize) -> Result<String, String> {
        match &self.0[index] {
            Cell::Text(v) => Ok(v.clone()),
            _ => Err(format!("column {index} is not text")),
        }
    }
    fn Time(&self, index: usize) -> Result<Time, String> {
        match self.0[index] {
            Cell::Time => Ok(Time::default()),
            _ => Err(format!("column {index} is not time")),
        }
    }
}

type QueryResult = Result<Vec<Box<dyn ImportJobRow>>, String>;
#[derive(Default)]
struct Executor {
    executions: Vec<(String, Vec<JobValue>)>,
    queries: Vec<(String, Vec<JobValue>, usize)>,
    query_results: VecDeque<QueryResult>,
    execute_error: Option<String>,
}
impl Executor {
    fn reply(&mut self, rows: Vec<Row>) {
        self.query_results.push_back(Ok(rows
            .into_iter()
            .map(|r| Box::new(r) as Box<dyn ImportJobRow>)
            .collect()));
    }
}
impl ImportJobExecutor for Executor {
    fn ExecuteInternal(&mut self, sql: &str, args: Vec<JobValue>) -> Result<(), String> {
        self.executions.push((sql.into(), args));
        self.execute_error.clone().map_or(Ok(()), Err)
    }
    fn QueryInternal(&mut self, sql: &str, args: Vec<JobValue>, columns: usize) -> QueryResult {
        self.queries.push((sql.into(), args, columns));
        self.query_results
            .pop_front()
            .unwrap_or_else(|| Err("unexpected query".into()))
    }
}

struct Codec;
impl ImportJobCodec for Codec {
    fn EncodeParameters(&self, p: &ImportParameters) -> Result<Vec<u8>, String> {
        Ok(format!("{}|{}", p.Format, p.FileLocation).into_bytes())
    }
    fn DecodeParameters(&self, bytes: &[u8]) -> Result<ImportParameters, String> {
        let value = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
        let (format, file) = value
            .split_once('|')
            .ok_or_else(|| "invalid parameters".to_owned())?;
        Ok(ImportParameters {
            Format: format.into(),
            FileLocation: file.into(),
            ..Default::default()
        })
    }
    fn EncodeSummary(&self, s: &Summary) -> Result<Vec<u8>, String> {
        Ok(s.ImportedRows.to_string().into_bytes())
    }
    fn DecodeSummary(&self, bytes: &[u8]) -> Result<Summary, String> {
        let rows = std::str::from_utf8(bytes)
            .map_err(|e| e.to_string())?
            .parse::<i64>()
            .map_err(|e| e.to_string())?;
        Ok(Summary {
            ImportedRows: rows,
            ..Default::default()
        })
    }
}

fn job_row(id: i64, owner: &str, status: &str, step: &str) -> Row {
    Row(vec![
        Cell::Int(id),
        Cell::Time,
        Cell::Null,
        Cell::Null,
        Cell::Null,
        Cell::Text("test".into()),
        Cell::Text("t".into()),
        Cell::Int(7),
        Cell::Text(owner.into()),
        Cell::Text("csv|s3://bucket/file.csv".into()),
        Cell::Int(123),
        Cell::Text(status.into()),
        Cell::Text(step.into()),
        Cell::Null,
        Cell::Null,
        Cell::Text("group".into()),
    ])
}

#[test]
fn job_state_predicates_match_go_boundaries() {
    let mut job = JobInfo::default();
    for (status, expected) in [
        ("pending", true),
        ("running", true),
        ("finished", false),
        ("failed", false),
        ("cancelled", false),
        ("canceled", false),
    ] {
        job.Status = status.into();
        assert_eq!(expected, job.CanCancel(), "{status}");
        assert_eq!(status == "cancelled", job.IsCancelled(), "{status}");
        assert_eq!(status == "finished", job.IsSuccess(), "{status}");
    }
    job.Status = JobStatusFinished.into();
    assert!(job.IsSuccess());
    job.SourceFileSize = 0;
    job.Status = jobStatusPending.into();
    assert!(job.IsSourceFileSizeUnknown());
    job.Status = JobStatusRunning.into();
    job.Step = JobStepPreparing.into();
    assert!(job.IsSourceFileSizeUnknown());
    job.Step = JobStepImporting.into();
    assert!(!job.IsSourceFileSizeUnknown());
    job.SourceFileSize = 1;
    job.Step = JobStepPreparing.into();
    assert!(!job.IsSourceFileSizeUnknown());
}

#[test]
fn create_job_preserves_go_sql_argument_order_and_last_id() {
    let mut ex = Executor::default();
    ex.reply(vec![Row(vec![Cell::Int(42)])]);
    let p = ImportParameters {
        Format: "csv".into(),
        FileLocation: "s3://bucket/file.csv".into(),
        ..Default::default()
    };
    let id = CreateJob(&mut ex, &Codec, "test", "t", 7, "root@%", "group", &p, 123).unwrap();
    assert_eq!(42, id);
    assert_eq!(42, TestLastImportJobID.load(Ordering::SeqCst));
    assert_eq!(
        vec![
            "test".into(),
            "t".into(),
            7_i64.into(),
            "group".into(),
            "root@%".into(),
            JobValue::Bytes(b"csv|s3://bucket/file.csv".to_vec()),
            123_i64.into(),
            "pending".into(),
            "".into()
        ],
        ex.executions[0].1
    );
    assert_eq!("SELECT LAST_INSERT_ID();", ex.queries[0].0);
    assert_eq!(1, ex.queries[0].2);
}

#[test]
fn lifecycle_updates_keep_go_guards_and_payloads() {
    let mut ex = Executor::default();
    let summary = Summary {
        ImportedRows: 111,
        ..Default::default()
    };
    StartJob(&mut ex, 9, JobStepImporting).unwrap();
    Job2Step(&mut ex, 9, JobStepValidating).unwrap();
    FinishJob(&mut ex, &Codec, 9, Some(&summary)).unwrap();
    FailJob(&mut ex, &Codec, 10, "boom", None).unwrap();
    CancelJob(&mut ex, 11).unwrap();
    assert_eq!(
        vec![
            JobValue::from(JobStatusRunning),
            JobValue::from(JobStepImporting),
            JobValue::from(9_i64),
            JobValue::from(jobStatusPending)
        ],
        ex.executions[0].1
    );
    assert_eq!(
        vec![
            JobValue::from(JobStepValidating),
            JobValue::from(9_i64),
            JobValue::from(JobStatusRunning)
        ],
        ex.executions[1].1
    );
    assert_eq!(JobValue::Bytes(b"111".to_vec()), ex.executions[2].1[2]);
    assert!(ex.executions[2].0.contains("AND status = %?"));
    assert_eq!(JobValue::Bytes(b"{}".to_vec()), ex.executions[3].1[2]);
    assert!(ex.executions[3].0.contains("status IN (%?, %?)"));
    assert_eq!(
        vec![
            JobValue::from(jogStatusCancelled),
            JobValue::from(11_i64),
            JobValue::from(jobStatusPending),
            JobValue::from(JobStatusRunning)
        ],
        ex.executions[4].1
    );
}

#[test]
fn get_job_decodes_nullable_fields_and_enforces_owner() {
    let mut ex = Executor::default();
    ex.reply(vec![job_row(3, "alice", "pending", "")]);
    let job = GetJob(&mut ex, &Codec, 3, "alice", false).unwrap();
    assert_eq!(3, job.ID);
    assert_eq!("csv", job.Parameters.Format);
    assert_eq!("s3://bucket/file.csv", job.Parameters.FileLocation);
    assert_eq!(Time::default(), job.StartTime);
    assert_eq!(Time::default(), job.UpdateTime);
    assert_eq!(Time::default(), job.EndTime);
    assert!(job.Summary.is_none());
    assert!(job.ErrorMessage.is_empty());
    ex.reply(vec![job_row(3, "alice", "pending", "")]);
    assert_eq!(
        "SUPER privilege is required to view another user's import job",
        GetJob(&mut ex, &Codec, 3, "bob", false).unwrap_err()
    );
    ex.reply(vec![]);
    assert_eq!(
        "import job 404 not found",
        GetJob(&mut ex, &Codec, 404, "alice", true).unwrap_err()
    );
}

#[test]
fn prepared_info_is_noop_without_running_row_and_preserves_file() {
    let mut ex = Executor::default();
    ex.reply(vec![]);
    UpdateJobPreparedInfo(&mut ex, &Codec, 8, 456, "csv").unwrap();
    assert!(ex.executions.is_empty());
    ex.reply(vec![Row(vec![Cell::Text(
        "auto|s3://bucket/file.csv".into(),
    )])]);
    UpdateJobPreparedInfo(&mut ex, &Codec, 8, 456, "csv").unwrap();
    assert_eq!(
        vec![
            456_i64.into(),
            JobValue::Bytes(b"csv|s3://bucket/file.csv".to_vec()),
            8_i64.into(),
            JobStatusRunning.into()
        ],
        ex.executions[0].1
    );
}

#[test]
fn list_queries_match_go_visibility_and_group_filters() {
    let mut ex = Executor::default();
    ex.reply(vec![]);
    GetJobsByGroupKey(&mut ex, &Codec, "alice", "batch", false).unwrap();
    assert!(
        ex.queries[0]
            .0
            .contains("created_by = %? AND group_key = %?")
    );
    assert_eq!(
        vec![JobValue::from("alice"), JobValue::from("batch")],
        ex.queries[0].1
    );
    ex.reply(vec![]);
    GetJobsByGroupKey(&mut ex, &Codec, "ignored", "", true).unwrap();
    assert!(ex.queries[1].0.ends_with("WHERE group_key != ''"));
    ex.reply(vec![]);
    GetAllViewableJobs(&mut ex, &Codec, "alice", false).unwrap();
    assert!(ex.queries[2].0.ends_with("WHERE created_by = %?"));
    ex.reply(vec![]);
    GetAllViewableJobs(&mut ex, &Codec, "ignored", true).unwrap();
    assert_eq!(baseQuerySQL, ex.queries[3].0);
}

#[test]
fn active_count_and_executor_errors_are_propagated() {
    let mut ex = Executor::default();
    ex.reply(vec![Row(vec![Cell::Int(2)])]);
    assert_eq!(2, GetActiveJobCnt(&mut ex, "test", "t").unwrap());
    assert_eq!(
        vec![
            JobValue::from(jobStatusPending),
            JobValue::from(JobStatusRunning),
            JobValue::from("test"),
            JobValue::from("t")
        ],
        ex.queries[0].1
    );
    ex.execute_error = Some("write failed".into());
    assert_eq!(
        "write failed",
        StartJob(&mut ex, 1, JobStepImporting).unwrap_err()
    );
}

#[test]
fn import_parameters_display_matches_go_json() {
    let mut p = ImportParameters {
        ColumnsAndVars: "(a,b)".into(),
        SetClause: "c=1".into(),
        FileLocation: "s3://bucket/file.csv".into(),
        Format: "csv".into(),
        Options: HashMap::new(),
    };
    assert_eq!(
        r#"{"columns-and-vars":"(a,b)","set-clause":"c=1","file-location":"s3://bucket/file.csv","format":"csv"}"#,
        p.to_string()
    );
    p.ColumnsAndVars.clear();
    p.SetClause.clear();
    p.Options = HashMap::from([
        ("z".into(), "line\n\"quoted\"".into()),
        ("a".into(), "1".into()),
    ]);
    assert_eq!(
        r#"{"file-location":"s3://bucket/file.csv","format":"csv","options":{"a":"1","z":"line\n\"quoted\""}}"#,
        p.to_string()
    );
}

#[test]
fn pending_cancellation_uses_only_pending_predicate_and_propagates_sql_errors() {
    let mut executor = Executor::default();
    CancelPendingJob(&mut executor, 41).unwrap();
    assert!(executor.executions[0].0.contains("status IN (%?)"));
    assert_eq!(
        executor.executions[0].1,
        vec![
            jogStatusCancelled.into(),
            41_i64.into(),
            jobStatusPending.into()
        ]
    );
    assert!(!executor.executions[0].0.contains("end_time"));
    assert!(!executor.executions[0].0.contains("start_time"));
    executor.execute_error = Some("SQL unavailable".into());
    assert_eq!(
        CancelPendingJob(&mut executor, 42).unwrap_err(),
        "SQL unavailable"
    );
}
