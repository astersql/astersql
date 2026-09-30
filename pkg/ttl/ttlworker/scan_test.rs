// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// `ScanWorker` 调度约束与扫描任务行为的单元测试。
//
// 空闲 worker 可调度；同一时刻只允许一个进行中的扫描任务。

use std::collections::VecDeque;

use crate::scan::{ScanResult, ScanWorker, TaskTerminateReason, TtlScanTask, TtlStatistics};
use crate::session::{Datum, PhysicalTable, Row, SessionError, SessionState, WorkerSession};

fn table() -> PhysicalTable {
    PhysicalTable {
        partition_name: None,
        table_id: 1,
        physical_id: 1,
        schema: "test".into(),
        table: "t1".into(),
        key_columns: vec!["id".into()],
        ttl_column: "time".into(),
        ttl_enabled: true,
        definition_version: 1,
        expire_after_seconds: 60,
    }
}

fn task(batch_size: usize) -> TtlScanTask {
    TtlScanTask {
        job_id: "job-1".into(),
        scan_id: 7,
        table: table(),
        expire_time: 100,
        range_start: Some(vec![Datum::Integer(0)]),
        range_end: Some(vec![Datum::Integer(100)]),
        batch_size,
    }
}

#[derive(Default)]
struct MockSession {
    state: SessionState,
    replies: VecDeque<Result<Vec<Row>, SessionError>>,
    calls: Vec<(String, Vec<Datum>)>,
}

impl WorkerSession for MockSession {
    fn state(&self) -> &SessionState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut SessionState {
        &mut self.state
    }

    fn execute(&mut self, sql: &str, args: &[Datum]) -> Result<Vec<Row>, SessionError> {
        self.calls.push((sql.into(), args.to_vec()));
        self.replies.pop_front().expect("unexpected SQL execution")
    }
}

/// 默认 worker 可调度、无当前任务，且 SQL 最大重试次数为 5。
#[test]
fn scan_worker_only_accepts_one_inflight_task() {
    let mut worker = ScanWorker::default();
    assert!(worker.could_schedule());
    assert!(worker.current_task().is_none());
    assert_eq!(crate::scan::SCAN_TASK_EXECUTE_SQL_MAX_RETRY, 5);

    let scan_task = task(3);
    assert!(worker.schedule(scan_task.clone()));
    assert!(!worker.schedule(scan_task));
}

#[test]
fn completed_result_must_be_polled_before_rescheduling() {
    let mut worker = ScanWorker::default();
    assert!(worker.schedule(task(3)));
    worker.finish(ScanResult {
        job_id: "job-1".into(),
        scan_id: 7,
        reason: TaskTerminateReason::Finished,
        error: None,
        scanned_rows: 0,
    });

    assert!(!worker.could_schedule());
    assert!(!worker.schedule(task(3)));
    assert!(worker.poll_result().is_some());
    assert!(worker.could_schedule());
}

#[test]
fn scan_sql_preserves_range_and_cursor_order() {
    let (sql, args) = task(0).scan_sql(Some(&[Datum::Integer(3)]));
    assert_eq!(
        sql,
        "SELECT `id` FROM `test`.`t1` WHERE `time` < FROM_UNIXTIME(%?) AND (`id`) >= (%?) AND (`id`) < (%?) AND (`id`) > (%?) ORDER BY `id` LIMIT 1"
    );
    assert_eq!(
        args,
        vec![
            Datum::Unsigned(100),
            Datum::Integer(0),
            Datum::Integer(100),
            Datum::Integer(3),
        ]
    );
}

#[test]
fn scan_sql_targets_the_physical_partition() {
    let mut task = task(0);
    task.table.partition_name = Some("p0".into());
    let (sql, _) = task.scan_sql(None);
    assert!(sql.starts_with("SELECT `id` FROM `test`.`t1` PARTITION (`p0`) WHERE"));
}

#[test]
fn execute_retries_then_advances_cursor_and_counts_dispatched_rows() {
    let mut session = MockSession {
        replies: VecDeque::from([
            Err(SessionError::Execute("temporary".into())),
            Ok(vec![vec![Datum::Integer(1)], vec![Datum::Integer(2)]]),
            Ok(vec![vec![Datum::Integer(3)]]),
        ]),
        ..MockSession::default()
    };
    let statistics = TtlStatistics::default();
    let mut dispatched = Vec::new();

    let result = task(2).execute(
        &mut session,
        &statistics,
        |rows| {
            dispatched.push(rows);
            Ok(())
        },
        || false,
    );

    assert_eq!(result.reason, TaskTerminateReason::Finished);
    assert_eq!(result.scanned_rows, 3);
    assert_eq!(statistics.snapshot(), (3, 0, 0));
    assert_eq!(dispatched.len(), 2);
    assert_eq!(session.calls.len(), 3);
    assert_eq!(session.calls[0], session.calls[1]);
    assert!(session.calls[2].0.contains("AND (`id`) > (%?)"));
    assert_eq!(session.calls[2].1.last(), Some(&Datum::Integer(2)));
}

#[test]
fn go_merge_43_scan_restarts_after_durable_cursor_and_stops_on_checkpoint_error() {
    let mut session = MockSession {
        replies: VecDeque::from([Ok(vec![vec![Datum::Integer(4)]])]),
        ..MockSession::default()
    };
    let statistics = TtlStatistics::default();
    let result = task(2).execute_with_checkpoint(
        &mut session,
        &statistics,
        Some(vec![Datum::Integer(3)]),
        |_| Ok(()),
        |_| Err(SessionError::Execute("durable checkpoint failed".into())),
        || false,
    );
    assert_eq!(session.calls[0].1.last(), Some(&Datum::Integer(3)));
    assert_eq!(result.reason, TaskTerminateReason::Error);
    assert_eq!(result.scanned_rows, 1);
    assert_eq!(statistics.snapshot(), (1, 0, 0));
}

#[test]
fn execute_propagates_non_retryable_and_dispatch_errors_without_counting_rows() {
    let statistics = TtlStatistics::default();
    let mut session = MockSession {
        replies: VecDeque::from([Err(SessionError::TableChanged)]),
        ..MockSession::default()
    };
    let result = task(2).execute(&mut session, &statistics, |_| Ok(()), || false);
    assert_eq!(result.reason, TaskTerminateReason::Error);
    assert_eq!(result.error, Some(SessionError::TableChanged));
    assert_eq!(session.calls.len(), 1);

    let mut session = MockSession {
        replies: VecDeque::from([Ok(vec![vec![Datum::Integer(1)]])]),
        ..MockSession::default()
    };
    let result = task(2).execute(
        &mut session,
        &statistics,
        |_| Err(SessionError::Execute("dispatch closed".into())),
        || false,
    );
    assert_eq!(result.reason, TaskTerminateReason::Error);
    assert_eq!(statistics.snapshot(), (0, 0, 0));
}

#[test]
fn execute_checks_cancellation_and_error_rate_before_sql() {
    let mut session = MockSession::default();
    let statistics = TtlStatistics::default();
    let canceled = task(2).execute(&mut session, &statistics, |_| Ok(()), || true);
    assert_eq!(canceled.reason, TaskTerminateReason::Canceled);

    statistics.add_total(10_001);
    statistics.add_error(4_001);
    let exceeded = task(2).execute(&mut session, &statistics, |_| Ok(()), || false);
    assert_eq!(exceeded.reason, TaskTerminateReason::ErrorRateExceeded);
    assert!(session.calls.is_empty());
}
