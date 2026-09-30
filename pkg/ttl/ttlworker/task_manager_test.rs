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

// TTL `TaskManager` 调度行为的单元测试。
//
// 覆盖空队列时重新调度的稳定性：无等待任务则不启动扫描。

/// 空管理器执行 `reschedule` 应返回空列表，且运行中/已完成计数均为 0。
#[test]
fn empty_task_manager_reschedule_is_stable() {
    let mut manager = crate::task_manager::TaskManager::new("node-1", 2);
    assert!(manager.reschedule(100).is_empty());
    assert_eq!(manager.running_count(), 0);
    assert!(manager.finished().is_empty());
}

#[test]
fn resigning_a_task_returns_it_to_waiting() {
    let table = crate::session::PhysicalTable {
        partition_name: None,
        table_id: 1,
        physical_id: 1,
        schema: "test".into(),
        table: "t".into(),
        key_columns: vec!["id".into()],
        ttl_column: "created_at".into(),
        ttl_enabled: true,
        definition_version: 1,
        expire_after_seconds: 10,
    };
    let scan = crate::scan::TtlScanTask {
        job_id: "job".into(),
        scan_id: 1,
        table,
        expire_time: 90,
        range_start: None,
        range_end: None,
        batch_size: 10,
    };
    let mut manager = crate::task_manager::TaskManager::new("owner", 1);
    manager.push_waiting(crate::job_manager::initial_managed_task(scan.clone()));
    assert_eq!(manager.reschedule(1), vec![scan.clone()]);
    assert_eq!(
        manager.heartbeat_or_resign(2, |_| false),
        vec![("job".into(), 1)]
    );
    assert_eq!(manager.reschedule(3), vec![scan]);
}

#[test]
fn scan_error_is_reported_as_a_finished_task() {
    let table = crate::session::PhysicalTable {
        partition_name: None,
        table_id: 1,
        physical_id: 1,
        schema: "test".into(),
        table: "t".into(),
        key_columns: vec!["id".into()],
        ttl_column: "created_at".into(),
        ttl_enabled: true,
        definition_version: 1,
        expire_after_seconds: 10,
    };
    let scan = crate::scan::TtlScanTask {
        job_id: "job".into(),
        scan_id: 2,
        table,
        expire_time: 90,
        range_start: None,
        range_end: None,
        batch_size: 10,
    };
    let mut manager = crate::task_manager::TaskManager::new("owner", 1);
    manager.push_waiting(crate::job_manager::initial_managed_task(scan));
    assert_eq!(manager.reschedule(1).len(), 1);

    assert!(manager.report_finished(crate::scan::ScanResult {
        job_id: "job".into(),
        scan_id: 2,
        reason: crate::scan::TaskTerminateReason::Error,
        error: None,
        scanned_rows: 3,
    }));
    assert_eq!(
        manager.finished()[0].status,
        crate::task_manager::TaskStatus::Finished
    );
}
