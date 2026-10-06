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

// Scanning cancellation integration tests corresponding to Go's
// `TestCancelWhileScan` and `TestCancelWhileScanAtStatementBoundary`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::scan::{TaskTerminateReason, TtlScanTask, TtlStatistics};
use crate::session::{Datum, PhysicalTable, Row, SessionError, SessionState, WorkerSession};

fn scan_task() -> TtlScanTask {
    TtlScanTask {
        job_id: "test".into(),
        scan_id: 1,
        table: PhysicalTable {
            partition_name: None,
            table_id: 1,
            physical_id: 1,
            schema: "test".into(),
            table: "t".into(),
            key_columns: vec!["id".into()],
            ttl_column: "created_at".into(),
            ttl_enabled: true,
            definition_version: 1,
            expire_after_seconds: 60 * 60,
        },
        expire_time: 1,
        range_start: None,
        range_end: None,
        batch_size: 100,
        scan_index: None,
    }
}

struct CancelableSession {
    state: SessionState,
    canceled: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
}

impl WorkerSession for CancelableSession {
    fn state(&self) -> &SessionState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut SessionState {
        &mut self.state
    }

    fn execute(&mut self, _sql: &str, _args: &[Datum]) -> Result<Vec<Row>, SessionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        while !self.canceled.load(Ordering::SeqCst) {
            thread::yield_now();
        }
        Err(SessionError::Execute("query canceled".into()))
    }
}

#[test]
fn cancel_while_scan_stops_without_retrying() {
    let canceled = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let task_canceled = Arc::clone(&canceled);
    let session_canceled = Arc::clone(&canceled);
    let session_calls = Arc::clone(&calls);

    let scan = thread::spawn(move || {
        let mut session = CancelableSession {
            state: SessionState::default(),
            canceled: session_canceled,
            calls: session_calls,
        };
        scan_task().execute(
            &mut session,
            &TtlStatistics::default(),
            |_| Ok(()),
            || task_canceled.load(Ordering::SeqCst),
        )
    });

    while calls.load(Ordering::SeqCst) == 0 {
        thread::yield_now();
    }
    let cancel_started = Instant::now();
    canceled.store(true, Ordering::SeqCst);
    let result = scan.join().expect("scan thread should terminate");

    assert!(cancel_started.elapsed() < Duration::from_secs(1));
    assert_eq!(result.reason, TaskTerminateReason::Canceled);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

struct StatementBoundarySession {
    state: SessionState,
    canceled: Arc<AtomicBool>,
}

impl WorkerSession for StatementBoundarySession {
    fn state(&self) -> &SessionState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut SessionState {
        &mut self.state
    }

    fn execute(&mut self, _sql: &str, _args: &[Datum]) -> Result<Vec<Row>, SessionError> {
        self.canceled.store(true, Ordering::SeqCst);
        Ok(vec![vec![Datum::Integer(1)]])
    }
}

#[test]
fn cancel_at_statement_boundary_does_not_dispatch_rows() {
    let canceled = Arc::new(AtomicBool::new(false));
    let task_canceled = Arc::clone(&canceled);
    let mut session = StatementBoundarySession {
        state: SessionState::default(),
        canceled,
    };
    let dispatches = AtomicUsize::new(0);
    let statistics = TtlStatistics::default();

    let result = scan_task().execute(
        &mut session,
        &statistics,
        |_| {
            dispatches.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
        || task_canceled.load(Ordering::SeqCst),
    );

    assert_eq!(result.reason, TaskTerminateReason::Canceled);
    assert_eq!(dispatches.load(Ordering::SeqCst), 0);
    assert_eq!(statistics.snapshot(), (0, 0, 0));
}
