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

// Admin Pause / Resume 与操作者权限（User vs System）测试。
//
// `AdminCommandOperator::User` 对应人工下发的 admin 命令；
// `System` 对应内部自动暂停（例如升级、资源协调）。System 暂停的 job
// 不允许 User Resume，必须由 System Resume；System 可 Resume User 暂停的 job。
// Resume 成功后清除 `paused_by` 并将状态恢复为 Running。

use astersql_ddl::ddl::{AdminCommandOperator, Ddl, Job, JobCommand, JobState};

use crate::ddl_stmt_cases::{
    AutoIncrsedID, StmtCase, column_ddl_stmt_case, index_ddl_stmt_case, place_rul_ddl_stmt_case,
    schema_ddl_stmt_case, table_ddl_stmt, table_partition_ddl_stmt_case,
};

/// 构造仅含一个 Running job（id=1）的 DDL 实例。
fn running_job() -> Ddl {
    let mut ddl = Ddl::new("admin-command-resume", Vec::new());
    ddl.started = true;
    ddl.submit_job(Job::new(1, 1, 1, "")).unwrap();
    ddl
}

/// 对应 Go `pauseResumeAndCancel(..., false)`：逐条覆盖 DDL case 矩阵。
///
/// 可暂停 case 必须完成 Running -> Paused -> Running，并保留原 SQL 与目标表身份；
/// 不可暂停 case 在 Go 中不会发出 admin 命令，因此保持 Running 且没有暂停操作者。
fn pause_and_resume(cases: Vec<StmtCase>) {
    for case in cases {
        let id = i64::from(case.global_id) + 1;
        let mut ddl = Ddl::new("admin-command-pause-resume", Vec::new());
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
            assert_eq!(ddl.jobs[&id].paused_by, Some(AdminCommandOperator::User));
            assert_eq!(
                ddl.process_jobs(&[id], JobCommand::Resume, AdminCommandOperator::User),
                vec![Ok(())],
                "resume failed for {} at {}",
                case.stmt,
                case.schema_state
            );
            assert_eq!(ddl.jobs[&id].state, JobState::Running);
            assert_eq!(ddl.jobs[&id].paused_by, None);
        } else {
            assert_eq!(ddl.jobs[&id].state, JobState::Running);
            assert_eq!(ddl.jobs[&id].paused_by, None);
        }

        assert_eq!(ddl.jobs[&id].query, case.stmt);
        assert_eq!(ddl.jobs[&id].table_id, id);
    }
}

/// User Pause 后 User Resume：恢复 Running 并清空 `paused_by`。
#[test]
fn process_jobs_user_pause_and_resume_clears_the_owner() {
    let mut ddl = running_job();
    assert_eq!(
        ddl.process_jobs(&[1], JobCommand::Pause, AdminCommandOperator::User),
        vec![Ok(())]
    );
    assert_eq!(ddl.jobs[&1].state, JobState::Paused);
    assert_eq!(ddl.jobs[&1].paused_by, Some(AdminCommandOperator::User));

    assert_eq!(
        ddl.process_jobs(&[1], JobCommand::Resume, AdminCommandOperator::User),
        vec![Ok(())]
    );
    assert_eq!(ddl.jobs[&1].state, JobState::Running);
    assert_eq!(ddl.jobs[&1].paused_by, None);
}

/// System Pause 后 User 不可 Resume；须由 System Resume。
#[test]
fn process_jobs_system_pause_requires_a_system_resume() {
    let mut ddl = running_job();
    assert_eq!(
        ddl.process_jobs(&[1], JobCommand::Pause, AdminCommandOperator::System),
        vec![Ok(())]
    );
    // User 无权恢复系统暂停的 job，状态与 paused_by 保持不变。
    assert_eq!(
        ddl.process_jobs(&[1], JobCommand::Resume, AdminCommandOperator::User),
        vec![Err("user cannot resume a system-paused DDL job".to_owned())]
    );
    assert_eq!(ddl.jobs[&1].state, JobState::Paused);
    assert_eq!(ddl.jobs[&1].paused_by, Some(AdminCommandOperator::System));

    assert_eq!(
        ddl.process_jobs(&[1], JobCommand::Resume, AdminCommandOperator::System),
        vec![Ok(())]
    );
    assert_eq!(ddl.jobs[&1].state, JobState::Running);
    assert_eq!(ddl.jobs[&1].paused_by, None);
}

/// System 操作者可以 Resume 被 User 暂停的 job。
#[test]
fn process_jobs_system_operator_can_resume_a_user_paused_job() {
    let mut ddl = running_job();
    ddl.process_jobs(&[1], JobCommand::Pause, AdminCommandOperator::User);
    assert_eq!(
        ddl.process_jobs(&[1], JobCommand::Resume, AdminCommandOperator::System),
        vec![Ok(())]
    );
    assert_eq!(ddl.jobs[&1].state, JobState::Running);
    assert_eq!(ddl.jobs[&1].paused_by, None);
}

#[test]
fn pause_and_resume_schema_table_and_placement_statements() {
    let mut ids = AutoIncrsedID { idx: 0 };
    let mut cases = schema_ddl_stmt_case(&mut ids);
    cases.extend(table_ddl_stmt(&mut ids));
    cases.extend(place_rul_ddl_stmt_case(&mut ids));
    pause_and_resume(cases);
}

#[test]
fn pause_and_resume_index_statements() {
    let mut ids = AutoIncrsedID { idx: 0 };
    pause_and_resume(index_ddl_stmt_case(&mut ids));
}

#[test]
fn pause_and_resume_column_statements() {
    let mut ids = AutoIncrsedID { idx: 0 };
    pause_and_resume(column_ddl_stmt_case(&mut ids));
}

#[test]
fn pause_and_resume_partition_statements() {
    let mut ids = AutoIncrsedID { idx: 0 };
    pause_and_resume(table_partition_ddl_stmt_case(&mut ids));
}
