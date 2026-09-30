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

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use crate::del::{DeleteRateLimiter, DeleteRetryBuffer, DeleteTask};
use crate::scan::TtlStatistics;
use crate::session::{Datum, PhysicalTable, SessionError, SessionState, WorkerSession};

fn task(name: &str, count: usize) -> DeleteTask {
    let rows = (0..count)
        .map(|id| vec![Datum::Integer(id as i64)])
        .collect();
    let statistics = Arc::new(TtlStatistics::default());
    statistics.add_total(count);
    DeleteTask {
        job_id: name.into(),
        table: PhysicalTable {
            partition_name: None,
            table_id: 1,
            physical_id: 1,
            schema: "test".into(),
            table: name.into(),
            key_columns: vec!["id".into()],
            ttl_column: "created_at".into(),
            ttl_enabled: true,
            definition_version: 1,
            expire_after_seconds: 10,
        },
        rows,
        expire_time: 90,
        statistics,
    }
}

#[test]
fn delete_sql_targets_the_physical_partition() {
    let mut delete = task("t", 1);
    delete.table.partition_name = Some("p0".into());
    assert!(
        delete
            .delete_sql(1)
            .starts_with("DELETE LOW_PRIORITY FROM `test`.`t` PARTITION (`p0`) WHERE")
    );
}

#[test]
fn delete_task_continues_after_retryable_and_limiter_errors() {
    struct Session(RefCell<Vec<(String, Vec<Datum>)>>);
    impl WorkerSession for Session {
        fn state(&self) -> &SessionState {
            static S: std::sync::OnceLock<SessionState> = std::sync::OnceLock::new();
            S.get_or_init(SessionState::default)
        }
        fn state_mut(&mut self) -> &mut SessionState {
            panic!("unused")
        }
        fn execute(&mut self, sql: &str, args: &[Datum]) -> Result<Vec<Vec<Datum>>, SessionError> {
            let mut calls = self.0.borrow_mut();
            calls.push((sql.into(), args.to_vec()));
            if calls.len() == 1 {
                Err(SessionError::Execute("retry".into()))
            } else {
                Ok(Vec::new())
            }
        }
    }
    struct Limiter(usize);
    impl DeleteRateLimiter for Limiter {
        fn wait_delete_token(&mut self, _: usize) -> Result<(), SessionError> {
            self.0 += 1;
            if self.0 == 2 {
                Err(SessionError::Execute("stop".into()))
            } else {
                Ok(())
            }
        }
    }
    let delete = task("t", 250);
    let mut session = Session(RefCell::new(Vec::new()));
    let retry = delete.do_delete(&mut session, &mut Limiter(0));
    assert_eq!(session.0.borrow().len(), 2);
    assert_eq!(retry.len(), 200);
    assert_eq!(delete.statistics.snapshot(), (250, 50, 0));
    let calls = session.0.borrow();
    assert!(
        calls[0]
            .0
            .starts_with("DELETE LOW_PRIORITY FROM `test`.`t` WHERE `id` IN")
    );
    assert!(
        calls[0]
            .0
            .ends_with("AND `created_at` < FROM_UNIXTIME(%?) LIMIT 100")
    );
    assert_eq!(calls[0].1.first(), Some(&Datum::Integer(0)));
    assert_eq!(calls[0].1.last(), Some(&Datum::Unsigned(90)));
}

#[test]
fn retry_buffer_matches_go_fifo_timing_retry_and_accounting() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let now = Arc::new(AtomicU64::new(0));
    let clock = Arc::clone(&now);
    let mut buffer = DeleteRetryBuffer::with_options(3, 2, Duration::from_secs(10), move || {
        Duration::from_secs(clock.load(Ordering::Relaxed))
    });
    let t1 = task("t1", 10);
    assert!(!buffer.record_task_result(t1.clone(), Vec::new()));
    buffer.record_task_result(t1.clone(), t1.rows[..1].to_vec());
    now.store(1, Ordering::Relaxed);
    let t2 = task("t2", 10);
    buffer.record_task_result(t2.clone(), t2.rows[..2].to_vec());
    now.store(2, Ordering::Relaxed);
    let t3 = task("t3", 10);
    buffer.record_task_result(t3.clone(), t3.rows[..3].to_vec());
    now.store(3, Ordering::Relaxed);
    let t4 = task("t4", 10);
    buffer.record_task_result(t4.clone(), t4.rows[..4].to_vec());
    assert_eq!(t1.statistics.snapshot(), (10, 0, 1));
    now.store(12, Ordering::Relaxed);
    let mut names = Vec::new();
    assert_eq!(
        buffer.retry_all(|t| {
            names.push(t.job_id.clone());
            t.statistics.add_success(t.rows.len());
            Vec::new()
        }),
        Duration::from_secs(1)
    );
    assert_eq!(names, ["t2", "t3"]);
    now.store(13, Ordering::Relaxed);
    assert_eq!(
        buffer.retry_all(|t| {
            names.push(t.job_id.clone());
            t.statistics.add_success(t.rows.len());
            Vec::new()
        }),
        Duration::from_secs(10)
    );
    assert_eq!(names, ["t2", "t3", "t4"]);

    let t5 = task("t5", 10);
    buffer.record_task_result(t5.clone(), t5.rows[..5].to_vec());
    for timestamp in [23, 33] {
        now.store(timestamp, Ordering::Relaxed);
        buffer.retry_all(|t| {
            t.statistics.add_success(1);
            t.rows[1..].to_vec()
        });
    }
    assert_eq!(t5.statistics.snapshot(), (10, 2, 3));
    assert_eq!(t5.rows.len(), 10);
    let t6 = task("t6", 10);
    buffer.record_task_result(t6.clone(), t6.rows[..7].to_vec());
    buffer.drain();
    assert_eq!(t6.statistics.snapshot(), (10, 0, 7));
}

#[test]
fn retry_defaults_and_zero_interval_match_go_contract() {
    assert_eq!(crate::del::DELETE_MAX_RETRY, 3);
    assert_eq!(crate::del::DELETE_RETRY_BUFFER_SIZE, 128);
    assert_eq!(
        DeleteRetryBuffer::default().retry_interval(),
        crate::del::DELETE_RETRY_INTERVAL
    );
    let mut buffer =
        DeleteRetryBuffer::with_options(usize::MAX, usize::MAX, Duration::ZERO, || Duration::ZERO);
    let t = task("t", 8);
    buffer.record_task_result(t.clone(), t.rows.clone());
    let mut calls = 0;
    buffer.retry_all(|t| {
        calls += 1;
        t.rows[1..].to_vec()
    });
    assert_eq!((calls, buffer.len()), (1, 1));
}
