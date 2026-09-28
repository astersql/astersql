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

// Copyright 2013 The ql Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSES/QL-LICENSE file.

use crate::ddl::Job;
use crate::ddl_history::HistoryStore;

fn job(id: i64) -> Job {
    Job::new(id, 0, 0, "")
}

#[test]
fn test_ddl_history_basic() {
    let mut store = HistoryStore::default();
    store.add_history_job(job(3), false);
    store.add_history_job(job(4), false);
    store.add_history_job(job(1), false);
    store.add_history_job(job(2), false);

    assert_eq!(store.get_by_id(1).map(|job| job.id), Some(1));
    assert_eq!(
        store.last_n(2).iter().map(|job| job.id).collect::<Vec<_>>(),
        vec![4, 3]
    );

    // Go GetAllHistoryDDLJobs explicitly sorts the complete result by ID ascending.
    assert_eq!(
        store.all().iter().map(|job| job.id).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );

    assert_eq!(
        store
            .scan(2, 2)
            .expect("bounded scan failed")
            .iter()
            .map(|job| job.id)
            .collect::<Vec<_>>(),
        vec![2, 1]
    );

    // Go ScanHistoryDDLJobs applies DefNumGetDDLHistoryJobs when both values are zero.
    assert_eq!(
        store
            .scan(0, 0)
            .expect("default scan failed")
            .iter()
            .map(|job| job.id)
            .collect::<Vec<_>>(),
        vec![4, 3, 2, 1]
    );

    let mut large_store = HistoryStore::default();
    for id in 1..=2050 {
        large_store.add_history_job(job(id), false);
    }
    let jobs = large_store.scan(0, 0).expect("default scan failed");
    assert_eq!(jobs.len(), 2048);
    assert_eq!(
        (jobs.first().unwrap().id, jobs.last().unwrap().id),
        (2050, 3)
    );
}

#[test]
fn test_scan_history_ddl_jobs_with_error_limit() {
    let store = HistoryStore::default();
    for start_job_id in [10, -1] {
        assert_eq!(
            store.scan(start_job_id, 0),
            Err("when 'start_job_id' is specified, it must work with a 'limit'".to_owned())
        );
    }
}

#[test]
fn duplicate_job_id_is_replaced_and_batches_can_stop_early() {
    let mut store = HistoryStore::default();
    store.add_history_job(Job::new(1, 1, 1, "old"), false);
    store.add_history_job(Job::new(1, 2, 2, "replacement"), true);
    store.add_history_job(job(2), false);

    assert_eq!(store.all().len(), 2);
    assert_eq!(store.get_by_id(1).unwrap().query, "replacement");

    let mut batches = Vec::new();
    store.iter_batches(1, |batch| {
        batches.push(batch[0].id);
        true
    });
    assert_eq!(batches, vec![2]);
}
