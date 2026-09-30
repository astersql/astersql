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

// Integration-level parity tests for the in-memory Rust task-manager model.
//
// These tests mirror the Go integration scenarios that the current Rust API can
// express: exclusive task identity, bounded/FIFO scheduling, owner resignation,
// completion accounting, and removal of tasks whose jobs are no longer valid.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use crate::job_manager::initial_managed_task;
use crate::scan::{ScanResult, TaskTerminateReason, TtlScanTask};
use crate::session::PhysicalTable;
use crate::task_manager::{TaskManager, TaskStatus};

fn scan_task(job_id: &str, scan_id: i64) -> TtlScanTask {
    TtlScanTask {
        job_id: job_id.into(),
        scan_id,
        table: PhysicalTable {
            partition_name: None,
            table_id: 7,
            physical_id: 7,
            schema: "test".into(),
            table: "t".into(),
            key_columns: vec!["id".into()],
            ttl_column: "created_at".into(),
            ttl_enabled: true,
            definition_version: 1,
            expire_after_seconds: 86_400,
        },
        expire_time: 100,
        range_start: None,
        range_end: None,
        batch_size: 128,
    }
}

#[test]
fn parallel_lock_allows_exactly_one_copy_of_a_task_identity() {
    let manager = Arc::new(Mutex::new(TaskManager::new("task-manager", 8)));
    let mut threads = Vec::new();
    for _ in 0..5 {
        let manager = Arc::clone(&manager);
        threads.push(std::thread::spawn(move || {
            manager
                .lock()
                .unwrap()
                .push_waiting(initial_managed_task(scan_task("test-job", 1)));
        }));
    }
    for thread in threads {
        thread.join().unwrap();
    }

    let mut manager = manager.lock().unwrap();
    assert_eq!(manager.reschedule(200), vec![scan_task("test-job", 1)]);
    assert_eq!(manager.running_count(), 1);
    assert!(manager.reschedule(201).is_empty());
}

#[test]
fn parallel_schedule_fills_capacity_once_and_preserves_fifo_order() {
    let mut manager = TaskManager::new("task-manager-1", 16);
    for scan_id in 0..32 {
        manager.push_waiting(initial_managed_task(scan_task("test-job", scan_id)));
    }

    let scheduled = manager.reschedule(300);
    assert_eq!(scheduled.len(), 16);
    assert_eq!(
        scheduled
            .iter()
            .map(|task| task.scan_id)
            .collect::<Vec<_>>(),
        (0..16).collect::<Vec<_>>()
    );
    assert_eq!(manager.running_count(), 16);
    assert!(manager.reschedule(301).is_empty());
}

#[test]
fn heartbeat_resignation_makes_only_disabled_tasks_schedulable_again() {
    let mut manager = TaskManager::new("task-manager-1", 4);
    for scan_id in 0..4 {
        manager.push_waiting(initial_managed_task(scan_task("test-job", scan_id)));
    }
    assert_eq!(manager.reschedule(100).len(), 4);

    let resigned = manager.heartbeat_or_resign(200, |task| task.task.scan_id != 0);
    assert_eq!(resigned, vec![("test-job".into(), 0)]);
    assert_eq!(manager.running_count(), 3);

    let rescheduled = manager.reschedule(300);
    assert_eq!(rescheduled, vec![scan_task("test-job", 0)]);
    assert_eq!(manager.running_count(), 4);
}

#[test]
fn completion_and_worker_stop_follow_go_persistence_states() {
    let mut manager = TaskManager::new("task-manager-1", 2);
    manager.push_waiting(initial_managed_task(scan_task("test-job", 0)));
    manager.push_waiting(initial_managed_task(scan_task("test-job", 1)));
    assert_eq!(manager.reschedule(100).len(), 2);

    assert!(manager.report_finished(ScanResult {
        job_id: "test-job".into(),
        scan_id: 0,
        reason: TaskTerminateReason::Finished,
        error: None,
        scanned_rows: 128,
    }));
    assert!(manager.report_finished(ScanResult {
        job_id: "test-job".into(),
        scan_id: 1,
        reason: TaskTerminateReason::WorkerStop,
        error: None,
        scanned_rows: 64,
    }));

    assert_eq!(manager.running_count(), 0);
    assert_eq!(manager.finished().len(), 1);
    assert_eq!(manager.finished()[0].status, TaskStatus::Finished);
    assert_eq!(manager.finished()[0].state.total_rows, 128);
    assert_eq!(manager.reschedule(200), vec![scan_task("test-job", 1)]);
}

#[test]
fn invalid_jobs_are_removed_without_affecting_valid_work() {
    let mut manager = TaskManager::new("task-manager-1", 2);
    manager.push_waiting(initial_managed_task(scan_task("valid-job", 0)));
    manager.push_waiting(initial_managed_task(scan_task("expired-job", 1)));
    assert_eq!(manager.reschedule(100).len(), 2);

    manager.remove_invalid_jobs(&BTreeSet::from(["valid-job".to_owned()]));
    assert_eq!(manager.running_count(), 1);
    assert!(manager.report_finished(ScanResult {
        job_id: "valid-job".into(),
        scan_id: 0,
        reason: TaskTerminateReason::Finished,
        error: None,
        scanned_rows: 1,
    }));
    assert!(!manager.report_finished(ScanResult {
        job_id: "expired-job".into(),
        scan_id: 1,
        reason: TaskTerminateReason::Finished,
        error: None,
        scanned_rows: 1,
    }));
}
