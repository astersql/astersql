// Copyright 2026 AsterSQL.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use astersql_errors as errors;
use astersql_kv as kv;
use astersql_util_memory as memory;

use crate::executor_with_retry::{
    CoordinatorFactory, CoordinatorRegistry, CoordinatorUniqueId, MppRecoveryConfig,
    NewExecutorWithRetry, SharedMppCoordinator,
};
use crate::recovery_handler::{HandlerImpl, RecoveryInfo};

struct FailingNextCoordinator;

struct TestStatusReporter;

impl kv::MppStatusReporter for TestStatusReporter {
    fn ReportStatus(&self, _: kv::ReportStatusRequest) -> Result<(), errors::SharedError> {
        Ok(())
    }
}

impl kv::Response for FailingNextCoordinator {
    fn Next(
        &mut self,
        _: &kv::Context,
    ) -> Result<Option<Box<dyn kv::ResultSubset>>, errors::SharedError> {
        Err(errors::New("original mpp error"))
    }

    fn Close(&mut self) -> Result<(), errors::SharedError> {
        Ok(())
    }
}

impl kv::MppCoordinator for FailingNextCoordinator {
    fn Execute(&mut self, _: &kv::Context) -> Result<Vec<kv::KeyRange>, errors::SharedError> {
        Ok(vec![])
    }

    fn ReportStatus(&mut self, _: kv::ReportStatusRequest) -> Result<(), errors::SharedError> {
        Ok(())
    }

    fn StatusReporter(&self) -> Arc<dyn kv::MppStatusReporter> {
        Arc::new(TestStatusReporter)
    }

    fn IsClosed(&self) -> bool {
        false
    }

    fn GetNodeCnt(&self) -> i32 {
        1
    }
}

struct FailsSecondBuild {
    builds: AtomicUsize,
}

impl CoordinatorFactory for FailsSecondBuild {
    fn Build(&self, _: u64) -> Result<Box<dyn kv::MppCoordinator>, errors::SharedError> {
        if self.builds.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(Box::new(FailingNextCoordinator))
        } else {
            Err(errors::New("rebuild error"))
        }
    }
}

#[derive(Default)]
struct TestRegistry;

impl CoordinatorRegistry for TestRegistry {
    fn Register(
        &self,
        _: CoordinatorUniqueId,
        _: SharedMppCoordinator,
        _: Arc<dyn kv::MppStatusReporter>,
    ) -> Result<(), errors::SharedError> {
        Ok(())
    }

    fn Unregister(&self, _: CoordinatorUniqueId) {}
}

struct AcceptRecovery;

impl HandlerImpl for AcceptRecovery {
    fn chooseHandlerImpl(&self, _: &errors::SharedError) -> bool {
        true
    }

    fn doRecovery(&self, _: &RecoveryInfo) -> Result<(), errors::SharedError> {
        Ok(())
    }
}

#[test]
fn recovery_rebuild_failure_returns_the_original_mpp_error() {
    let mut parent = memory::tracker::NewTracker(1, 0);
    let mut executor = NewExecutorWithRetry(
        kv::Context::todo(),
        &mut parent,
        kv::MPPQueryID::default(),
        Arc::new(AtomicU64::new(0)),
        Arc::new(FailsSecondBuild {
            builds: AtomicUsize::new(0),
        }),
        Arc::new(TestRegistry),
        MppRecoveryConfig {
            enabled: true,
            holder_capacity: 2,
            ..Default::default()
        },
    )
    .expect("initial coordinator");
    executor.recovery_handler_mut().handlers = vec![Box::new(AcceptRecovery)];

    let error = match kv::Response::Next(&mut executor, &kv::Context::todo()) {
        Err(error) => error,
        Ok(_) => panic!("rebuild failure must return an error"),
    };
    assert_eq!(error.to_string(), "original mpp error");
}
