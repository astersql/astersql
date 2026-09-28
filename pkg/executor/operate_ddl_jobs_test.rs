// Copyright 2026 AsterSQL.

use crate::operate_ddl_jobs::{AlterDDLJobBackend, AlterDDLJobExec};
use std::cell::Cell;

#[derive(Clone, Debug, Eq, PartialEq)]
struct TestError(&'static str);

#[derive(Default)]
struct TestJob;

#[derive(Default)]
struct MockBackend {
    begin_calls: usize,
    commit_failure_checks: Cell<usize>,
    rollback_calls: usize,
    release_calls: usize,
}

impl AlterDDLJobBackend for MockBackend {
    type Context = ();
    type Session = ();
    type Job = TestJob;
    type Error = TestError;

    fn system_session(&mut self) -> Result<Self::Session, Self::Error> {
        Ok(())
    }

    fn release_system_session(&mut self, _session: Self::Session) {
        self.release_calls += 1;
    }

    fn begin(
        &mut self,
        _ctx: &mut Self::Context,
        _session: &mut Self::Session,
    ) -> Result<(), Self::Error> {
        self.begin_calls += 1;
        Ok(())
    }

    fn get_job(
        &mut self,
        _ctx: &mut Self::Context,
        _session: &mut Self::Session,
        _job_id: i64,
    ) -> Result<Self::Job, Self::Error> {
        Ok(TestJob)
    }

    fn is_alterable(&self, _job: &Self::Job) -> bool {
        true
    }

    fn operation_name(&self, _job: &Self::Job) -> String {
        "ADD INDEX".to_owned()
    }

    fn is_next_generation_add_index(&self, _job: &Self::Job) -> bool {
        false
    }

    fn unsupported_operation(&self, _operation: &str) -> Self::Error {
        TestError("unsupported operation")
    }

    fn unsupported_next_generation_add_index(&self) -> Self::Error {
        TestError("unsupported next-generation add index")
    }

    fn set_concurrency(&self, _job: &mut Self::Job, _value: i64) {}

    fn set_batch_size(&self, _job: &mut Self::Job, _value: i64) {}

    fn set_max_write_speed(&self, _job: &mut Self::Job, _value: i64) {}

    fn set_admin_operator_end_user(&self, _job: &mut Self::Job) {}

    fn update_job(
        &mut self,
        _ctx: &mut Self::Context,
        _session: &mut Self::Session,
        _job: &Self::Job,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn inject_commit_failure(&self) -> Option<Self::Error> {
        self.commit_failure_checks
            .set(self.commit_failure_checks.get() + 1);
        Some(TestError("mock commit failed on admin alter ddl jobs"))
    }

    fn commit(
        &mut self,
        _ctx: &mut Self::Context,
        _session: &mut Self::Session,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn rollback(&mut self, _session: &mut Self::Session) {
        self.rollback_calls += 1;
    }
}

#[test]
fn commit_failpoint_rolls_back_and_returns_without_retrying() {
    let mut executor = AlterDDLJobExec {
        backend: MockBackend::default(),
        job_id: 42,
        alter_options: Vec::new(),
    };

    let error = executor.Open(&mut ()).unwrap_err();

    assert_eq!(
        error,
        TestError("mock commit failed on admin alter ddl jobs")
    );
    assert_eq!(executor.backend.begin_calls, 1);
    assert_eq!(executor.backend.commit_failure_checks.get(), 1);
    assert_eq!(executor.backend.rollback_calls, 1);
    assert_eq!(executor.backend.release_calls, 1);
}
