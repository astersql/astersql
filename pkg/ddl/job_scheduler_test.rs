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

use crate::job_scheduler::{JobScheduler, UnSyncedJobTracker};
use crate::job_worker::{JobWorker, WorkerType};
use crate::{SchemaLoader, SchemaLoaderError};
use std::sync::Mutex;
use std::time::Duration;

struct SequenceLoader {
    results: Mutex<Vec<Result<(), SchemaLoaderError>>>,
}

impl SequenceLoader {
    fn new(results: Vec<Result<(), SchemaLoaderError>>) -> Self {
        Self {
            results: Mutex::new(results.into_iter().rev().collect()),
        }
    }

    fn remaining(&self) -> usize {
        self.results.lock().expect("loader mutex poisoned").len()
    }
}

impl SchemaLoader for SequenceLoader {
    fn reload(&self) -> Result<(), SchemaLoaderError> {
        self.results
            .lock()
            .expect("loader mutex poisoned")
            .pop()
            .expect("unexpected reload call")
    }
}

fn scheduler() -> JobScheduler {
    JobScheduler::new(
        JobWorker::new(WorkerType::General),
        JobWorker::new(WorkerType::AddIndex),
    )
}

#[test]
fn must_reload_schemas_succeeds_retries_and_stops_when_cancelled() {
    let direct = SequenceLoader::new(vec![Ok(())]);
    let mut scheduler = scheduler();
    scheduler.must_reload_schemas_with(&direct, Duration::ZERO, || false);
    assert_eq!(direct.remaining(), 0);

    let retry = SequenceLoader::new(vec![Err(SchemaLoaderError::new("mock err")), Ok(())]);
    scheduler.must_reload_schemas_with(&retry, Duration::ZERO, || false);
    assert_eq!(retry.remaining(), 0);

    let cancelled = SequenceLoader::new(vec![Err(SchemaLoaderError::new("mock err"))]);
    scheduler.must_reload_schemas_with(&cancelled, Duration::ZERO, || true);
    assert_eq!(cancelled.remaining(), 0);
}

#[test]
fn un_synced_job_tracker_adds_queries_and_removes_ids() {
    let tracker = UnSyncedJobTracker::default();
    tracker.add_un_synced(1);
    assert!(tracker.is_un_synced(1));
    tracker.remove_un_synced(1);
    assert!(!tracker.is_un_synced(1));

    assert!(!tracker.maybe_already_run_once(1));
    tracker.set_already_run_once(1);
    assert!(tracker.maybe_already_run_once(1));
}
