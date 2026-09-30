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

// `JobManager` 作为 `TtlJobAdapter` 的集成测试。
//
// 覆盖 can_submit_job / submit_job / get_job / now：leader 判定、表与分区可见性、
// 并发 job 互斥、request 查找与完成摘要、以及可注入时钟。

use astersql_ttl_ttlworker::job_manager::{JobManager, TtlSummary};
use astersql_ttl_ttlworker::session::PhysicalTable;
use astersql_ttl_ttlworker::timer::TtlJobAdapter;

/// 构造测试用物理表元数据（含是否开启 TTL）。
fn ttl_table(table_id: i64, physical_id: i64, ttl_enabled: bool) -> PhysicalTable {
    PhysicalTable {
        partition_name: None,
        table_id,
        physical_id,
        schema: "test".to_owned(),
        table: "t1".to_owned(),
        key_columns: vec!["t".to_owned()],
        ttl_column: "t".to_owned(),
        ttl_enabled,
        definition_version: 1,
        expire_after_seconds: 86400,
    }
}

/// 返回已标记为 leader 的 `JobManager`，便于直接测提交路径。
fn leader_manager() -> JobManager {
    let mut manager = JobManager::new("manager-1", 4);
    manager.is_leader = true;
    manager
}

// 对应 Go TestManagerJobAdapterCanSubmitJob：非 leader 节点、未知表、未开启 TTL 的表都不能提交 job。
#[test]
fn test_can_submit_job_rejects_non_leader_and_missing_or_disabled_tables() {
    let mut manager = JobManager::new("manager-1", 4);
    manager.refresh_tables([ttl_table(1, 1, true)]);
    // Not the leader yet: even a valid TTL table must be rejected.
    assert!(!manager.can_submit_job(1, 1));

    manager.is_leader = true;
    assert!(
        !manager.can_submit_job(9999, 9999),
        "unknown table must be rejected"
    );

    manager.refresh_tables([ttl_table(2, 2, false)]);
    assert!(
        !manager.can_submit_job(2, 2),
        "table without TTL enabled must be rejected"
    );

    manager.refresh_tables([ttl_table(2, 2, true)]);
    assert!(manager.can_submit_job(2, 2));
}

// 对应 Go 用例中分区表场景：一张逻辑表下的多个分区各自作为独立 physical_id 判断是否可提交。
#[test]
fn test_can_submit_job_treats_each_partition_independently() {
    let mut manager = leader_manager();
    manager.refresh_tables([
        ttl_table(3, 30, true), // partition p0
        ttl_table(3, 31, true), // partition p1
    ]);
    assert!(manager.can_submit_job(3, 30));
    assert!(manager.can_submit_job(3, 31));
    // Wrong logical table id for that physical id must be rejected.
    assert!(!manager.can_submit_job(999, 30));
}

// 对应 Go 用例末尾：已经有一个 active job 占用该 physical table 时，不能再提交第二个。
#[test]
fn test_can_submit_job_rejects_when_job_already_active() {
    let mut manager = leader_manager();
    manager.refresh_tables([ttl_table(1, 1, true)]);
    assert!(manager.can_submit_job(1, 1));

    let trace = manager
        .submit_job(1, 1, "req1", 100)
        .expect("first submission should succeed");
    assert_eq!(trace.request_id, "req1");
    assert!(!trace.finished);
    assert!(trace.summary.is_none());

    assert!(
        !manager.can_submit_job(1, 1),
        "a second concurrent job must be rejected"
    );
    let err = manager
        .submit_job(1, 1, "req2", 101)
        .expect_err("submit_job must fail once a job is already active");
    assert!(err.contains("cannot be submitted"));
}

// 对应 Go TestManagerJobAdapterGetJob 的 "not found" 分支：未知的 request id 必须报错。
#[test]
fn test_get_job_fails_for_unknown_request_id() {
    let manager = leader_manager();
    let err = manager
        .get_job(1, 2, "req1")
        .expect_err("looking up an unsubmitted request must fail");
    assert!(err.contains("not found"));
}

// 对应 Go 用例：table_id / physical_id 与提交时不一致应报错，而不是静默返回错误的 job。
#[test]
fn test_get_job_fails_when_table_or_physical_id_mismatches() {
    let mut manager = leader_manager();
    manager.refresh_tables([ttl_table(1, 2, true)]);
    manager.submit_job(1, 2, "req1", 0).unwrap();

    assert!(
        manager.get_job(999, 2, "req1").is_err(),
        "mismatched table_id must fail"
    );
    assert!(
        manager.get_job(1, 999, "req1").is_err(),
        "mismatched physical_id must fail"
    );
}

// 对应 Go 用例：job 仍在运行时 get_job 返回 finished=false 且没有 summary；
// job 完成后返回 finished=true 并带上 summary。
#[test]
fn test_get_job_reports_running_then_finished_with_summary() {
    let mut manager = leader_manager();
    manager.refresh_tables([ttl_table(1, 2, true)]);
    manager.submit_job(1, 2, "req1", 0).unwrap();

    let running = manager.get_job(1, 2, "req1").unwrap();
    assert_eq!(running.request_id, "req1");
    assert!(!running.finished);
    assert!(running.summary.is_none());

    let expected_summary = TtlSummary {
        total_rows: 1_000,
        success_rows: 998,
        error_rows: 2,
        scan_task_err: "err1".to_owned(),
    };
    let job_id = manager
        .store
        .active_jobs
        .get(&2)
        .expect("submitted job must be active")
        .id
        .clone();
    let finished_ids = manager.finish_completed_jobs(1000);
    assert_eq!(finished_ids.len(), 1);
    manager
        .store
        .history
        .get_mut(&job_id)
        .expect("finished job must remain queryable from history")
        .summary = Some(expected_summary.clone());

    let finished = manager.get_job(1, 2, "req1").unwrap();
    assert!(finished.finished);
    assert_eq!(finished.summary, Some(expected_summary));
}

// 对应 Go TestManagerJobAdapterNow：JobManager 的 "now" 完全由测试可控的字段驱动，
// 而不是读取真实的会话时区，方便测试对时间敏感的调度逻辑。
#[test]
fn test_now_returns_the_managers_configured_clock() {
    let mut manager = JobManager::new("manager-1", 4);
    assert_eq!(TtlJobAdapter::now(&manager), 0);
    manager.now = 123_456;
    assert_eq!(TtlJobAdapter::now(&manager), 123_456);
}
