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

//! Go `pause_negative_test.go` parity tests.
//!
//! Both Go cases inject a failure after the pause state transition has been
//! prepared. The command must report the injected error, must not leak the
//! tentative state, and a later pause/cancel sequence must still succeed.

use astersql_ddl::ddl::{AdminCommandOperator, Ddl, Job, JobCommand, JobState};

fn running_job() -> Ddl {
    let mut ddl = Ddl::new("admin-pause-negative", Vec::new());
    ddl.started = true;
    ddl.submit_job(Job::new(1, 1, 1, "alter table t add index (id)"))
        .unwrap();
    ddl
}

#[test]
fn pause_on_write_conflict_is_not_retried_and_later_pause_cancel_succeeds() {
    let mut ddl = running_job();

    let (job_errors, command_result) = ddl.process_jobs_transactionally(
        &[1],
        JobCommand::Pause,
        AdminCommandOperator::User,
        || Err("mock failed admin command on ddl jobs".to_owned()),
    );
    assert_eq!(job_errors, vec![Ok(())]);
    assert_eq!(
        command_result,
        Err("mock failed admin command on ddl jobs".to_owned())
    );
    assert_eq!(ddl.jobs[&1].state, JobState::Running);

    let (pause_errors, pause_result) = ddl.process_jobs_transactionally(
        &[1],
        JobCommand::Pause,
        AdminCommandOperator::User,
        || Ok(()),
    );
    assert_eq!(pause_errors, vec![Ok(())]);
    assert_eq!(pause_result, Ok(()));
    assert_eq!(ddl.jobs[&1].state, JobState::Paused);

    let (cancel_errors, cancel_result) = ddl.process_jobs_transactionally(
        &[1],
        JobCommand::Cancel,
        AdminCommandOperator::User,
        || Ok(()),
    );
    assert_eq!(cancel_errors, vec![Ok(())]);
    assert_eq!(cancel_result, Ok(()));
    assert_eq!(ddl.jobs[&1].state, JobState::Cancelling);
}

#[test]
fn pause_failed_on_commit_reports_one_job_result_and_rolls_back() {
    let mut ddl = running_job();

    let (job_errors, command_result) = ddl.process_jobs_transactionally(
        &[1],
        JobCommand::Pause,
        AdminCommandOperator::User,
        || Err("mock commit failed on admin command on ddl jobs".to_owned()),
    );

    assert_eq!(job_errors.len(), 1);
    assert_eq!(job_errors, vec![Ok(())]);
    assert_eq!(
        command_result,
        Err("mock commit failed on admin command on ddl jobs".to_owned())
    );
    assert_eq!(ddl.jobs[&1].state, JobState::Running);
    assert_eq!(ddl.jobs[&1].paused_by, None);
}
