// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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


use crate::ddl::{Job, JobState};
use crate::job_submitter::{
    JobSpec, JobSubmitter, build_query_string_from_jobs, merge_create_table_jobs,
};

/// 同 schema 的多条 CREATE TABLE 应合并为一条 pending，且只通知一次。
#[test]
fn submitter_merges_create_table_jobs_and_notifies_once() {
    let mut submitter = JobSubmitter::default();
    let jobs = vec![
        JobSpec::new(Job::new(1, 7, 1, "create table db.t1(id int)"), false),
        JobSpec::new(Job::new(2, 7, 2, "create table db.t2(id int)"), false),
    ];
    assert_eq!(vec![Ok(1)], submitter.submit(jobs));
    let pending = submitter.take_pending();
    assert_eq!(1, pending.len());
    assert_eq!(2, pending[0].merged_jobs.len());
    assert_eq!(1, submitter.notification_count());
}

/// 已持久化的 Job ID 再次提交应返回 already exists 错误。
#[test]
fn submitter_rejects_duplicate_persisted_job_ids() {
    let mut submitter = JobSubmitter::default();
    assert_eq!(
        vec![Ok(1)],
        submitter.submit(vec![JobSpec::new(
            Job::new(1, 1, 1, "alter table t add c int"),
            false
        )])
    );
    let result = submitter.submit(vec![JobSpec::new(Job::new(1, 1, 1, "drop table t"), false)]);
    assert!(result[0].as_ref().unwrap_err().contains("already exists"));
    assert_eq!(1, submitter.notification_count());
}

/// Go 只在 query 尚无尾分号时补一个分号，已有的多个尾分号必须原样保留。
#[test]
fn query_builder_preserves_existing_trailing_semicolons() {
    let jobs = [
        JobSpec::new(Job::new(1, 1, 1, " create table t1(a int);; "), false),
        JobSpec::new(Job::new(2, 1, 2, "create table t2(a int)"), false),
    ];
    assert_eq!(
        "create table t1(a int);; create table t2(a int);",
        build_query_string_from_jobs(&jobs)
    );
}

/// Go 的合并资格只取决于动作类型、ID 是否已分配及外键，不检查 JobState。
#[test]
fn merge_eligibility_does_not_depend_on_job_state() {
    let first = JobSpec::new(Job::new(1, 1, 1, "create table t1(a int)"), false);
    let mut second = JobSpec::new(Job::new(2, 1, 2, "create table t2(a int)"), false);
    second.job.state = JobState::Running;

    let merged = merge_create_table_jobs(vec![first, second]);
    assert_eq!(1, merged.len());
    assert_eq!(2, merged[0].merged_jobs.len());
}
