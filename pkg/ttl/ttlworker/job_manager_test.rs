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

// 作业管理依赖的 `TaskManager` 单元测试。
//
// 覆盖并发槽位下限等与 JobManager 调度相关的约束。

use crate::job_manager::{JobManager, TableStatus};
use crate::session::PhysicalTable;

fn ttl_table() -> PhysicalTable {
    PhysicalTable {
        partition_name: None,
        table_id: 1,
        physical_id: 1,
        schema: "test".to_owned(),
        table: "t1".to_owned(),
        key_columns: vec!["id".to_owned()],
        ttl_column: "created_at".to_owned(),
        ttl_enabled: true,
        definition_version: 1,
        expire_after_seconds: 60,
    }
}

fn status(owner_id: Option<&str>, heartbeat: u64) -> TableStatus {
    TableStatus {
        table_id: 1,
        parent_table_id: 1,
        current_job_id: Some("job-1".to_owned()),
        owner_id: owner_id.map(str::to_owned),
        owner_heartbeat: heartbeat,
        job_start: 1,
        job_expire: 0,
    }
}

/// 构造时传入 0 个 worker 槽位，应被抬升为至少 1。
#[test]
fn task_manager_enforces_at_least_one_worker_slot() {
    let manager = crate::task_manager::TaskManager::new("owner", 0);
    assert_eq!(manager.owner_id, "owner");
    assert_eq!(manager.max_running_tasks, 1);
    assert_eq!(manager.running_count(), 0);
}

#[test]
fn non_leader_does_not_gc_shared_job_state() {
    let mut manager = JobManager::new("owner", 1);
    let mut idle_status = status(None, 0);
    idle_status.current_job_id = None;
    manager.statuses.insert(1, idle_status);
    manager.gc(100, 0);
    assert!(manager.statuses.contains_key(&1));
}

#[test]
fn heartbeat_only_updates_locally_active_jobs() {
    let mut manager = JobManager::new("owner", 1);
    manager.statuses.insert(1, status(Some("owner"), 10));
    manager.update_heartbeat(20);
    assert_eq!(manager.statuses[&1].owner_heartbeat, 10);
}

#[test]
fn reschedule_skips_job_already_active_on_this_manager() {
    let mut manager = JobManager::new("owner", 1);
    manager.is_leader = true;
    manager.refresh_tables([ttl_table()]);
    manager
        .lock_new_job(1, "job-1", 1, false)
        .expect("job should lock");
    manager.statuses.get_mut(&1).unwrap().owner_heartbeat = 1;

    assert!(manager.reschedule_timeout_jobs(100, 10).is_empty());
}
