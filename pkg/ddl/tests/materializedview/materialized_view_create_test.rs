// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use crate::job_worker::{
    DurableJobExecutor, DurableJobSession, DurableJobStep, JobLease, JobWorker,
    TransactionOperation, WorkerType,
};
use astersql_meta_model::group_3::{Job, JobState};

struct Lease;

impl JobLease for Lease {
    fn is_owner(&self) -> bool {
        true
    }

    fn is_cancelled(&self) -> bool {
        false
    }
}

#[derive(Default)]
struct AtomicSession {
    begins: usize,
    commits: usize,
    rollbacks: usize,
    job_reads: usize,
    job_updates: usize,
    pending_metadata: bool,
    durable_metadata: bool,
    concurrent_job_update: bool,
}

impl DurableJobSession for AtomicSession {
    fn query(&mut self, _: &str, label: &str) -> Result<Vec<Vec<String>>, String> {
        match label {
            "get_job" => {
                self.job_reads += 1;
                let bytes = if self.concurrent_job_update && self.job_reads == 2 {
                    "changed"
                } else {
                    "expected"
                };
                Ok(vec![vec![bytes.to_owned()]])
            }
            "mutate-mview-metadata" => {
                self.pending_metadata = true;
                Ok(Vec::new())
            }
            "update_job" => {
                self.job_updates += 1;
                Ok(Vec::new())
            }
            label => panic!("unexpected query label {label}"),
        }
    }

    fn begin(&mut self) -> Result<(), String> {
        self.begins += 1;
        Ok(())
    }

    fn commit(&mut self) -> Result<(), String> {
        self.commits += 1;
        self.durable_metadata |= self.pending_metadata;
        self.pending_metadata = false;
        Ok(())
    }

    fn rollback(&mut self) {
        self.rollbacks += 1;
        self.pending_metadata = false;
    }

    fn transaction_size(&mut self) -> Result<usize, String> {
        Ok(0)
    }

    fn with_transaction(&mut self, _: TransactionOperation) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }
}

struct FailingMViewStep(&'static str);

impl DurableJobExecutor for FailingMViewStep {
    fn runnable(&mut self, _: &mut dyn DurableJobSession, _: &Job) -> Result<bool, String> {
        Ok(true)
    }

    fn recover(&mut self, _: &Job, _: &dyn JobLease) -> Result<(), String> {
        Ok(())
    }

    fn step(
        &mut self,
        session: &mut dyn DurableJobSession,
        job: &mut Job,
    ) -> Result<DurableJobStep, String> {
        session.query("mutate materialized view metadata", "mutate-mview-metadata")?;
        job.state = JobState::Rollingback;
        job.error = Some(self.0.to_owned());
        job.error_count += 1;
        job.last_schema_version = 91;
        Ok(DurableJobStep {
            schema_version: 91,
            update_raw_args: false,
            removed: false,
            must_rollback_transaction: true,
        })
    }

    fn wait_synced(&mut self, _: &Job, _: i64, _: &dyn JobLease) -> Result<(), String> {
        Ok(())
    }
}

fn run_failed_step(
    action: u8,
    error: &'static str,
    concurrent_job_update: bool,
) -> (Result<i64, String>, AtomicSession, Job) {
    let mut session = AtomicSession {
        concurrent_job_update,
        ..Default::default()
    };
    let mut job = Job {
        id: 42,
        tp: action,
        state: JobState::Running,
        ..Default::default()
    };
    let result = JobWorker::new(WorkerType::General).transit_persisted_job_step(
        &mut session,
        &Lease,
        &mut FailingMViewStep(error),
        &mut job,
        b"expected",
    );
    (result, session, job)
}

#[test]
fn mview_cleanup_rollback_discards_metadata_and_retries_schema_version() {
    for (action, error) in [
        (
            astersql_meta_model::group_3::ACTION_CREATE_MATERIALIZED_VIEW,
            "refresh info cleanup failed",
        ),
        (
            astersql_meta_model::group_3::ACTION_CREATE_MATERIALIZED_VIEW_LOG,
            "purge info cleanup failed",
        ),
        (
            astersql_meta_model::group_3::ACTION_CREATE_MATERIALIZED_VIEW,
            "rollback schema version failed",
        ),
        (
            astersql_meta_model::group_3::ACTION_CREATE_MATERIALIZED_VIEW_LOG,
            "rollback schema version failed",
        ),
        (
            astersql_meta_model::group_3::ACTION_DROP_MATERIALIZED_VIEW,
            "refresh info delete failed",
        ),
        (
            astersql_meta_model::group_3::ACTION_DROP_MATERIALIZED_VIEW_LOG,
            "purge info delete failed",
        ),
    ] {
        let (result, session, job) = run_failed_step(action, error, false);
        assert_eq!(result.unwrap(), 0, "action {action}");
        assert_eq!(session.begins, 2, "action {action}");
        assert_eq!(session.rollbacks, 1, "action {action}");
        assert_eq!(session.commits, 1, "action {action}");
        assert_eq!(session.job_reads, 2, "action {action}");
        assert_eq!(session.job_updates, 1, "action {action}");
        assert!(!session.durable_metadata, "action {action}");
        assert_eq!(job.state, JobState::Rollingback, "action {action}");
        assert_eq!(job.error.as_deref(), Some(error), "action {action}");
        assert_eq!(job.last_schema_version, 91, "action {action}");
    }
}

#[test]
fn mview_cleanup_rollback_notification_failure_is_atomic() {
    for action in [
        astersql_meta_model::group_3::ACTION_DROP_MATERIALIZED_VIEW,
        astersql_meta_model::group_3::ACTION_DROP_MATERIALIZED_VIEW_LOG,
    ] {
        let (result, session, job) = run_failed_step(action, "schema notification failed", false);
        assert_eq!(result.unwrap(), 0, "action {action}");
        assert!(!session.durable_metadata, "action {action}");
        assert_eq!(session.job_updates, 1, "action {action}");
        assert_eq!(job.error_count, 1, "action {action}");
    }
}

#[test]
fn mview_cleanup_rollback_rechecks_job_meta_after_reopen() {
    let (result, session, _) = run_failed_step(
        astersql_meta_model::group_3::ACTION_CREATE_MATERIALIZED_VIEW,
        "refresh info cleanup failed",
        true,
    );
    assert_eq!(result.unwrap_err(), "job meta changed by others");
    assert_eq!(session.begins, 2);
    assert_eq!(session.rollbacks, 2);
    assert_eq!(session.commits, 0);
    assert_eq!(session.job_updates, 0);
    assert!(!session.durable_metadata);
}
