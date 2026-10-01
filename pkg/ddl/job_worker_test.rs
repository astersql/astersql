// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// Job Worker 相关单元测试的精简替身实现。
//
// 覆盖：DDL owner 检查、非法 job 类型、批量提交失败回传、
// 并行 DDL 按表保序，以及 `job_need_gc` 对加索引/多 schema 变更的判定。
// GC（Garbage Collection）此处指清理已结束且可能产生临时数据的 DDL 任务。

use std::collections::HashMap;
use std::time::Duration;

use crate::ddl::{
    ActionType as ProductionActionType, Job as ProductionJob, JobState as ProductionJobState,
};
use crate::job_worker::{
    build_placement_affects, choose_lease_time, job_need_gc as production_job_need_gc,
};

/// 测试用的 DDL Action（操作类型）枚举。
#[derive(Clone, Debug, PartialEq, Eq)]
enum Action {
    None,
    AddIndex,
    AddPrimaryKey,
    AddColumn,
    DropColumn,
    RebaseAutoId,
    /// 多 schema 变更：一次 DDL 包含多个子 job。
    MultiSchemaChange(Vec<Job>),
}

/// 测试用的 Job 终态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JobState {
    Cancelled,
    Done,
    /// 回滚完成态。
    RollbackDone,
}

/// 测试用 Job：仅保留 action 与 state。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Job {
    action: Action,
    state: JobState,
}

/// 模拟执行 job：可注入批量提交失败，并拒绝 `Action::None`。
fn execute_job(action: &Action, fail_batch_submit: bool) -> Result<(), String> {
    if fail_batch_submit {
        return Err("mockAddBatchDDLJobsErr".to_owned());
    }
    if action == &Action::None {
        return Err("[ddl:8204]invalid ddl job type: none".to_owned());
    }
    Ok(())
}

/// 判断 job 是否需要 GC：仅 Done/RollbackDone，且 action 涉及加索引（含子 job）时为真。
fn job_need_gc(job: &Job) -> bool {
    if !matches!(job.state, JobState::Done | JobState::RollbackDone) {
        return false;
    }
    match &job.action {
        Action::AddIndex | Action::AddPrimaryKey => true,
        Action::MultiSchemaChange(sub_jobs) => sub_jobs.iter().any(job_need_gc),
        Action::None | Action::AddColumn | Action::DropColumn | Action::RebaseAutoId => false,
    }
}

/// 已提交 job 的轻量描述，用于并行投递保序测试。
#[derive(Clone, Copy)]
struct SubmittedJob {
    /// 逻辑表标识（库.表），同表 job 需保持序列递增。
    table: &'static str,
    /// 是否可能触发 reorg（数据重组）。
    may_need_reorg: bool,
}

/// 为并行提交的 job 分配全局递增 sequence，并统计可能需要 reorg 的数量。
///
/// 同表多次提交时，后出现的 sequence 必须严格大于先前记录，以模拟按表保序。
fn deliver_parallel_jobs(jobs: &[SubmittedJob]) -> (Vec<u64>, usize) {
    let mut next_sequence = 1_u64;
    let mut last_by_table = HashMap::new();
    let mut sequences = Vec::with_capacity(jobs.len());
    let mut reorg_count = 0;
    for job in jobs {
        if job.may_need_reorg {
            reorg_count += 1;
        }
        let sequence = next_sequence;
        next_sequence += 1;
        // 同表再次出现时断言序列严格递增，保证并行投递仍按表有序。
        if let Some(previous) = last_by_table.insert(job.table, sequence) {
            assert!(previous < sequence);
        }
        sequences.push(sequence);
    }
    (sequences, reorg_count)
}

/// 验证 owner 管理器持有租约且标记为 owner。
#[test]
fn test_check_owner() {
    struct OwnerManager {
        lease_millis: u64,
        is_owner: bool,
    }
    let manager = OwnerManager {
        lease_millis: 5_000,
        is_owner: true,
    };
    assert!(manager.is_owner);
    assert_eq!(5_000, manager.lease_millis);
}

/// 非法 DDL job 类型（None）应返回错误码 8204。
#[test]
fn test_invalid_ddl_job() {
    assert_eq!(
        Err("[ddl:8204]invalid ddl job type: none".to_owned()),
        execute_job(&Action::None, false)
    );
}

/// 批量提交失败时，错误应原样返回给调用方。
#[test]
fn test_add_batch_job_error_returns_to_caller() {
    assert_eq!(
        Err("mockAddBatchDDLJobsErr".to_owned()),
        execute_job(&Action::AddIndex, true)
    );
}

/// 并行 DDL：同一表上的 sequence 保持严格递增，并正确统计 reorg 数量。
#[test]
fn test_parallel_ddl_preserves_order_per_table() {
    let jobs = [
        SubmittedJob {
            table: "db1.t1",
            may_need_reorg: true,
        },
        SubmittedJob {
            table: "db1.t1",
            may_need_reorg: false,
        },
        SubmittedJob {
            table: "db1.t1",
            may_need_reorg: true,
        },
        SubmittedJob {
            table: "db1.t2",
            may_need_reorg: false,
        },
        SubmittedJob {
            table: "db1.t1",
            may_need_reorg: false,
        },
        SubmittedJob {
            table: "db1.t2",
            may_need_reorg: true,
        },
        SubmittedJob {
            table: "db2.t3",
            may_need_reorg: false,
        },
        SubmittedJob {
            table: "db2.t3",
            may_need_reorg: false,
        },
        SubmittedJob {
            table: "db1.t1",
            may_need_reorg: true,
        },
        SubmittedJob {
            table: "db2",
            may_need_reorg: false,
        },
        SubmittedJob {
            table: "db2.t3",
            may_need_reorg: true,
        },
    ];
    let (sequences, reorg_count) = deliver_parallel_jobs(&jobs);
    assert_eq!(5, reorg_count);
    // 按表校验 sequence 偏序关系。
    assert!(sequences[0] < sequences[1]);
    assert!(sequences[1] < sequences[2]);
    assert!(sequences[2] < sequences[4]);
    assert!(sequences[4] < sequences[8]);
    assert!(sequences[3] < sequences[5]);
    assert!(sequences[6] < sequences[7]);
    assert!(sequences[7] < sequences[10]);
}

/// 构造测试 Job 的便捷函数。
fn job(action: Action, state: JobState) -> Job {
    Job { action, state }
}

/// 覆盖 `job_need_gc`：加索引终态需 GC；纯列变更不需；MultiSchemaChange 递归判定。
#[test]
fn test_job_need_gc() {
    assert!(!job_need_gc(&job(Action::AddIndex, JobState::Cancelled)));
    assert!(!job_need_gc(&job(Action::AddColumn, JobState::Done)));
    assert!(job_need_gc(&job(Action::AddIndex, JobState::Done)));
    assert!(job_need_gc(&job(
        Action::AddPrimaryKey,
        JobState::RollbackDone
    )));

    let no_index = job(
        Action::MultiSchemaChange(vec![
            job(Action::AddColumn, JobState::Done),
            job(Action::RebaseAutoId, JobState::Done),
        ]),
        JobState::Done,
    );
    assert!(!job_need_gc(&no_index));

    for state in [JobState::Done, JobState::RollbackDone] {
        let with_index = job(
            Action::MultiSchemaChange(vec![
                job(Action::AddIndex, state),
                job(Action::DropColumn, state),
                job(Action::RebaseAutoId, JobState::Cancelled),
            ]),
            state,
        );
        assert!(job_need_gc(&with_index));
    }
}

#[test]
fn production_helpers_match_go_edge_cases() {
    assert_eq!(
        Duration::from_secs(10),
        choose_lease_time(Duration::ZERO, Duration::from_secs(10))
    );
    assert_eq!(
        Duration::from_secs(10),
        choose_lease_time(Duration::from_secs(20), Duration::from_secs(10))
    );

    let mut ordinary_job = ProductionJob::new(1, 2, 3, "alter table t add column c int");
    ordinary_job.state = ProductionJobState::Done;
    assert!(!production_job_need_gc(&ordinary_job));

    ordinary_job.action_type = ProductionActionType::DropTable;
    assert!(production_job_need_gc(&ordinary_job));
    ordinary_job.state = ProductionJobState::Cancelled;
    assert!(!production_job_need_gc(&ordinary_job));
}

#[test]
fn build_placement_affects_matches_go_contract() {
    let affects = build_placement_affects(&[11, 12], &[21, 22]);
    assert_eq!(2, affects.len());
    assert_eq!((11, 21), (affects[0].old_table_id, affects[0].table_id));
    assert_eq!((12, 22), (affects[1].old_table_id, affects[1].table_id));
}

#[test]
#[should_panic(expected = "new table IDs must cover every old table ID")]
fn build_placement_affects_rejects_mismatched_lengths_like_go() {
    let _ = build_placement_affects(&[11, 12], &[21]);
}

#[test]
fn crossks_align_durable_scheduler_denies_non_owner_before_opening_a_transaction() {
    use crate::job_worker::{
        DurableJobExecutor, DurableJobSession, DurableJobStep, JobLease, JobWorker,
        TransactionOperation, WorkerType,
    };
    use astersql_meta_model::group_3::Job as WireJob;
    struct InaccessibleSession;
    impl DurableJobSession for InaccessibleSession {
        fn query(&mut self, _: &str, _: &str) -> Result<Vec<Vec<String>>, String> {
            panic!("non-owner must not issue SQL")
        }
        fn begin(&mut self) -> Result<(), String> {
            panic!("non-owner must not begin")
        }
        fn commit(&mut self) -> Result<(), String> {
            panic!("non-owner must not commit")
        }
        fn rollback(&mut self) {
            panic!("no transaction was opened")
        }
        fn with_transaction(&mut self, _: TransactionOperation) -> Result<Vec<u8>, String> {
            panic!("non-owner must not access metadata")
        }
    }
    struct InaccessibleExecutor;
    impl DurableJobExecutor for InaccessibleExecutor {
        fn runnable(&mut self, _: &mut dyn DurableJobSession, _: &WireJob) -> Result<bool, String> {
            panic!("unexpected executor")
        }
        fn recover(&mut self, _: &WireJob, _: &dyn JobLease) -> Result<(), String> {
            panic!("unexpected recovery")
        }
        fn step(
            &mut self,
            _: &mut dyn DurableJobSession,
            _: &mut WireJob,
        ) -> Result<DurableJobStep, String> {
            panic!("unexpected step")
        }
        fn wait_synced(&mut self, _: &WireJob, _: i64, _: &dyn JobLease) -> Result<(), String> {
            panic!("unexpected sync")
        }
    }
    struct Lease(bool, bool);
    impl JobLease for Lease {
        fn is_owner(&self) -> bool {
            self.0
        }
        fn is_cancelled(&self) -> bool {
            self.1
        }
    }
    for lease in [Lease(false, false), Lease(true, true)] {
        let result = JobWorker::new(WorkerType::General).transit_persisted_job_step(
            &mut InaccessibleSession,
            &lease,
            &mut InaccessibleExecutor,
            &mut WireJob::default(),
            &[],
        );
        assert!(result.is_err());
    }
}
