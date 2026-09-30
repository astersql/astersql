// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// `JobManager` 作业生命周期的集成测试。
//
// 对应 Go `job_manager_integration_test.go` 中不依赖 TiDB Domain/SQL 的核心
// 契约：leader/TTL 门禁、锁唯一性、任务收尾、心跳、超时接管与 GC。

use crate::job_manager::{JobManager, TableStatus};
use crate::scan::{ScanResult, TaskTerminateReason, TtlScanTask};
use crate::session::PhysicalTable;
use crate::timer::TtlJobAdapter;

fn ttl_table(table_id: i64, physical_id: i64, enabled: bool) -> PhysicalTable {
    PhysicalTable {
        partition_name: None,
        table_id,
        physical_id,
        schema: "test".to_owned(),
        table: "t".to_owned(),
        key_columns: vec!["id".to_owned()],
        ttl_column: "created_at".to_owned(),
        ttl_enabled: enabled,
        definition_version: 1,
        expire_after_seconds: 60,
    }
}

fn running_status(table_id: i64, job_id: &str, owner: &str, heartbeat: u64) -> TableStatus {
    TableStatus {
        table_id,
        parent_table_id: table_id,
        current_job_id: Some(job_id.to_owned()),
        owner_id: Some(owner.to_owned()),
        owner_heartbeat: heartbeat,
        job_start: heartbeat,
        job_expire: heartbeat.saturating_sub(60),
    }
}

/// 空 `JobStore` 的三个集合均应为空。
#[test]
fn empty_job_store_has_no_active_or_historical_jobs() {
    let store = crate::job::JobStore::default();
    assert!(store.active_jobs.is_empty());
    assert!(store.tasks_by_job.is_empty());
    assert!(store.history.is_empty());
}

/// 对应 Go `TestParallelLockNewJob` 和 `TestSubmitJob`：同一物理表只能有一个活跃 job。
#[test]
fn parallel_lock_contract_allows_exactly_one_active_job() {
    let mut manager = JobManager::new("manager-1", 4);
    manager.is_leader = true;
    manager.refresh_tables([ttl_table(1, 1, true)]);

    assert!(manager.lock_new_job(1, "job-1", 100, false).is_some());
    for contender in 2..=8 {
        assert!(
            manager
                .lock_new_job(1, format!("job-{contender}"), 100, false)
                .is_none()
        );
    }
    assert_eq!(manager.store.active_jobs.len(), 1);
    assert_eq!(manager.store.history.len(), 1);
    assert_eq!(
        manager.statuses[&1].current_job_id.as_deref(),
        Some("job-1")
    );
}

/// 对应 Go `TestFinishJob`：所有任务完成后清理活跃状态并保留历史汇总。
#[test]
fn finished_tasks_close_job_and_preserve_history_summary() {
    let mut manager = JobManager::new("manager-1", 1);
    manager.is_leader = true;
    let table = ttl_table(1, 1, true);
    manager.refresh_tables([table.clone()]);
    manager.lock_new_job(1, "job-1", 100, false).unwrap();
    manager.store.tasks_by_job.insert("job-1".to_owned(), 1);
    manager
        .task_manager
        .push_waiting(crate::job_manager::initial_managed_task(TtlScanTask {
            job_id: "job-1".to_owned(),
            scan_id: 0,
            table,
            expire_time: 40,
            range_start: None,
            range_end: None,
            batch_size: 128,
        }));
    assert_eq!(manager.task_manager.reschedule(101).len(), 1);
    assert!(manager.task_manager.report_finished(ScanResult {
        job_id: "job-1".to_owned(),
        scan_id: 0,
        reason: TaskTerminateReason::Finished,
        error: None,
        scanned_rows: 128,
    }));

    assert_eq!(manager.finish_completed_jobs(200), vec!["job-1"]);
    assert!(manager.store.active_jobs.is_empty());
    assert!(manager.store.tasks_by_job.is_empty());
    assert!(manager.statuses[&1].current_job_id.is_none());
    let history = &manager.store.history["job-1"];
    assert_eq!(history.finish_time, Some(200));
    assert_eq!(history.summary.as_ref().unwrap().total_rows, 128);
}

/// 对应 Go `TestTTLJobDisable`/`TestSubmitJob`：非 leader、TTL 关闭或表 ID 不匹配均拒绝提交。
#[test]
fn submission_enforces_leader_enabled_and_table_identity() {
    let mut manager = JobManager::new("manager-1", 1);
    manager.refresh_tables([ttl_table(10, 11, true)]);
    assert!(!manager.can_submit_job(10, 11));
    manager.is_leader = true;
    assert!(!manager.can_submit_job(99, 11));
    manager.refresh_tables([ttl_table(10, 11, false)]);
    assert!(!manager.can_submit_job(10, 11));
    assert!(manager.submit_job(10, 11, "request-1", 100).is_err());
}

/// 对应 Go 心跳和 `TestRescheduleJobs`：本地 job 续约，失联外部 job 可被接管。
#[test]
fn heartbeat_and_timeout_takeover_do_not_interfere() {
    let mut manager = JobManager::new("manager-2", 1);
    manager.is_leader = true;
    manager.refresh_tables([ttl_table(1, 1, true), ttl_table(2, 2, true)]);
    manager.lock_new_job(1, "local-job", 10, false).unwrap();
    manager
        .statuses
        .insert(2, running_status(2, "remote-job", "manager-1", 10));

    manager.update_heartbeat(20);
    assert_eq!(manager.statuses[&1].owner_heartbeat, 20);
    assert_eq!(manager.statuses[&2].owner_heartbeat, 10);
    assert_eq!(manager.reschedule_timeout_jobs(31, 20), vec![2]);
    assert_eq!(manager.statuses[&2].owner_id.as_deref(), Some("manager-2"));
    assert_eq!(
        manager.statuses[&2].current_job_id.as_deref(),
        Some("remote-job")
    );
    assert_eq!(manager.statuses[&2].owner_heartbeat, 31);
}

/// 对应 Go `TestGCTableStatus`/`TestGCTTLHistory`：仅 leader 清理，且保留期边界与运行状态必须保留。
#[test]
fn leader_gc_honors_retention_boundary_and_running_status() {
    let mut manager = JobManager::new("manager-1", 1);
    manager.refresh_tables([ttl_table(1, 1, true), ttl_table(2, 2, true)]);
    manager.is_leader = true;
    manager.lock_new_job(1, "old", 10, false).unwrap();
    manager.lock_new_job(2, "boundary", 20, false).unwrap();
    manager.store.active_jobs.clear();
    manager.statuses.get_mut(&1).unwrap().current_job_id = None;
    manager.statuses.get_mut(&1).unwrap().owner_id = None;
    manager.refresh_tables([]);
    manager
        .statuses
        .insert(3, running_status(3, "still-running", "other", 1));

    manager.is_leader = false;
    manager.gc(120, 100);
    assert_eq!(manager.store.history.len(), 2);
    manager.is_leader = true;
    manager.gc(120, 100);

    assert!(!manager.store.history.contains_key("old"));
    assert!(manager.store.history.contains_key("boundary"));
    assert!(!manager.statuses.contains_key(&1));
    assert!(manager.statuses.contains_key(&3));
}
