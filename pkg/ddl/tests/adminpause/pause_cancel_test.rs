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

// Admin Pause 后执行 Cancel 的行为测试。
//
// 覆盖：对 Running job 直接 Cancel；对 User 暂停的 job 由 System Cancel；
// 以及对多个 job ID（含不存在 ID）批量 Cancel 时按请求顺序返回结果。
// Cancel 将 job 置为 `Cancelling`，由 worker 继续收尾；`paused_by`
// 记录暂停操作者（User / System），Cancel 本身不清除该字段。

use astersql_ddl::ddl::{AdminCommandOperator, Ddl, Job, JobCommand, JobState};

use crate::ddl_stmt_cases::{
    AutoIncrsedID, StmtCase, column_ddl_stmt_case, index_ddl_stmt_case, place_rul_ddl_stmt_case,
    schema_ddl_stmt_case, table_ddl_stmt, table_partition_ddl_stmt_case,
};

/// 构造已启动且包含指定 ID 的 Running DDL Job 集合。
fn running_ddl(ids: &[i64]) -> Ddl {
    let mut ddl = Ddl::new("admin-command-cancel", Vec::new());
    ddl.started = true;
    // 逐个提交 job，使其进入可被 admin 命令处理的运行队列。
    for id in ids {
        ddl.submit_job(Job::new(*id, 1, *id, "")).unwrap();
    }
    ddl
}

/// 对应 Go `pauseAndCancelStmt` 及其后的 `simpleRunStmt`：可暂停用例先暂停、
/// 再取消，随后以新的 job ID 重跑同一语句；不可暂停用例直接完成并同样重跑。
fn pause_cancel_and_rerun(cases: Vec<StmtCase>) {
    for case in cases {
        let id = i64::from(case.global_id) + 1;
        let mut ddl = Ddl::new("admin-command-cancel-rerun", Vec::new());
        ddl.started = true;
        ddl.submit_job(Job::new(id, 1, id, &case.stmt)).unwrap();

        if case.is_job_pausable {
            assert_eq!(
                ddl.process_jobs(&[id], JobCommand::Pause, AdminCommandOperator::User),
                vec![Ok(())],
                "pause failed for {} at {}",
                case.stmt,
                case.schema_state
            );
            assert_eq!(ddl.jobs[&id].state, JobState::Paused);
            assert_eq!(
                ddl.process_jobs(&[id], JobCommand::Cancel, AdminCommandOperator::User),
                vec![Ok(())],
                "cancel failed for {} at {}",
                case.stmt,
                case.schema_state
            );
            assert_eq!(ddl.jobs[&id].state, JobState::Cancelling);
        } else {
            assert_eq!(ddl.jobs[&id].state, JobState::Running);
            assert_eq!(ddl.jobs[&id].paused_by, None);
        }

        // Go 在每个 case 后再次执行同一 SQL。新的 ID 表示新的 DDL job，证明
        // 前一 job 的取消状态不会阻止同一语句重新提交。
        let rerun_id = id + 1_000;
        ddl.submit_job(Job::new(rerun_id, 1, rerun_id, &case.stmt))
            .unwrap();
        assert_eq!(ddl.jobs[&rerun_id].state, JobState::Running);
        assert_eq!(ddl.jobs[&rerun_id].query, case.stmt);
    }
}

/// Running 状态的 job 可被直接 Cancel，进入 Cancelling。
#[test]
fn process_jobs_cancels_a_running_job() {
    let mut ddl = running_ddl(&[1]);
    let results = ddl.process_jobs(&[1], JobCommand::Cancel, AdminCommandOperator::User);
    assert_eq!(results, vec![Ok(())]);
    assert_eq!(ddl.jobs[&1].state, JobState::Cancelling);
    assert_eq!(ddl.jobs[&1].paused_by, None);
}

/// User 暂停后再 Cancel：状态变为 Cancelling，并保留原 `paused_by=User`。
#[test]
fn process_jobs_cancels_a_user_paused_job() {
    let mut ddl = running_ddl(&[1]);
    assert_eq!(
        ddl.process_jobs(&[1], JobCommand::Pause, AdminCommandOperator::User),
        vec![Ok(())]
    );
    assert_eq!(ddl.jobs[&1].state, JobState::Paused);

    // System 也可取消已被 User 暂停的 job。
    assert_eq!(
        ddl.process_jobs(&[1], JobCommand::Cancel, AdminCommandOperator::System),
        vec![Ok(())]
    );
    assert_eq!(ddl.jobs[&1].state, JobState::Cancelling);
    assert_eq!(ddl.jobs[&1].paused_by, Some(AdminCommandOperator::User));
}

/// 批量 Cancel：每个请求 ID 对应一个结果；缺失 ID 返回 not found。
#[test]
fn process_jobs_returns_one_result_per_requested_cancel() {
    let mut ddl = running_ddl(&[1, 2]);
    ddl.process_jobs(&[2], JobCommand::Pause, AdminCommandOperator::System);

    let results = ddl.process_jobs(&[1, 2, 99], JobCommand::Cancel, AdminCommandOperator::User);
    assert_eq!(results[0], Ok(()));
    assert_eq!(results[1], Ok(()));
    assert_eq!(results[2], Err("DDL job 99 not found".to_owned()));
    assert_eq!(ddl.jobs[&1].state, JobState::Cancelling);
    assert_eq!(ddl.jobs[&2].state, JobState::Cancelling);
}

#[test]
fn pause_cancel_and_rerun_schema_table_and_placement_statements() {
    let mut ids = AutoIncrsedID { idx: 0 };
    let mut cases = schema_ddl_stmt_case(&mut ids);
    cases.extend(table_ddl_stmt(&mut ids));
    cases.extend(place_rul_ddl_stmt_case(&mut ids));
    pause_cancel_and_rerun(cases);
}

#[test]
fn pause_cancel_and_rerun_index_statements() {
    let mut ids = AutoIncrsedID { idx: 0 };
    pause_cancel_and_rerun(index_ddl_stmt_case(&mut ids));
}

#[test]
fn pause_cancel_and_rerun_column_statements() {
    let mut ids = AutoIncrsedID { idx: 0 };
    pause_cancel_and_rerun(column_ddl_stmt_case(&mut ids));
}

#[test]
fn pause_cancel_and_rerun_partition_statements() {
    let mut ids = AutoIncrsedID { idx: 0 };
    pause_cancel_and_rerun(table_partition_ddl_stmt_case(&mut ids));
}
