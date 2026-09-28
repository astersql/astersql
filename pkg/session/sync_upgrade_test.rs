// Copyright 2026 AsterSQL.

use crate::sync_upgrade::{
    IsUpgradingClusterState, OwnerOp, ServerState, SyncNormalRunning, SyncUpgradeRuntime,
    SyncUpgradeState, isUpgradingClusterStateWithRetry,
};
use std::collections::VecDeque;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestError(&'static str);

struct TestRuntime {
    owner_results: VecDeque<Result<OwnerOp, TestError>>,
    owner_timeouts: Vec<Duration>,
    global_results: VecDeque<Result<ServerState, TestError>>,
    global_timeouts: Vec<Duration>,
    updates: Vec<(ServerState, Duration)>,
    resume_result: Option<(Vec<TestError>, Option<TestError>)>,
    task_manager_available: bool,
    adjust_result: Result<(), TestError>,
    warnings: Vec<(&'static str, Option<TestError>)>,
    states: Vec<(i64, i64, bool)>,
    sleeps: Vec<Duration>,
}

impl Default for TestRuntime {
    fn default() -> Self {
        Self {
            owner_results: VecDeque::new(),
            owner_timeouts: Vec::new(),
            global_results: VecDeque::new(),
            global_timeouts: Vec::new(),
            updates: Vec::new(),
            resume_result: None,
            task_manager_available: false,
            adjust_result: Ok(()),
            warnings: Vec::new(),
            states: Vec::new(),
            sleeps: Vec::new(),
        }
    }
}

impl SyncUpgradeRuntime for TestRuntime {
    type Error = TestError;

    fn update_global_state(
        &mut self,
        state: ServerState,
        timeout: Duration,
    ) -> Result<(), Self::Error> {
        self.updates.push((state, timeout));
        Ok(())
    }

    fn owner_operation(&mut self, timeout: Duration) -> Result<OwnerOp, Self::Error> {
        self.owner_timeouts.push(timeout);
        self.owner_results.pop_front().expect("owner result")
    }

    fn resume_all_jobs(&mut self) -> (Vec<Self::Error>, Option<Self::Error>) {
        self.resume_result.take().unwrap_or_default()
    }

    fn task_manager_available(&mut self) -> bool {
        self.task_manager_available
    }

    fn adjust_task_overflow_concurrency(&mut self) -> Result<(), Self::Error> {
        self.adjust_result.clone()
    }

    fn global_state(&mut self, timeout: Duration) -> Result<ServerState, Self::Error> {
        self.global_timeouts.push(timeout);
        self.global_results
            .pop_front()
            .expect("global state result")
    }

    fn timeout_error(&mut self, _timeout: Duration) -> Self::Error {
        TestError("timeout")
    }

    fn log_warning(&mut self, message: &str, error: Option<&Self::Error>) {
        let message = match message {
            "get owner op failed" => "get owner op failed",
            "DDL owner has not synced upgrading state" => {
                "DDL owner has not synced upgrading state"
            }
            "resume all paused jobs failed" => "resume all paused jobs failed",
            "resume the job failed" => "resume the job failed",
            "cannot adjust task overflow concurrency" => "cannot adjust task overflow concurrency",
            "get global state failed" => "get global state failed",
            "get global state timed out" => "get global state timed out",
            _ => "other",
        };
        self.warnings.push((message, error.cloned()));
    }

    fn log_state(&mut self, old_version: i64, new_version: i64, upgrading: bool) {
        self.states.push((old_version, new_version, upgrading));
    }

    fn sleep(&mut self, duration: Duration) {
        self.sleeps.push(duration);
    }
}

#[test]
fn sync_upgrade_state_matches_go_warning_for_unsynced_owner() {
    let mut runtime = TestRuntime {
        owner_results: VecDeque::from([
            Ok(OwnerOp {
                synced_upgrading_state: false,
            }),
            Ok(OwnerOp {
                synced_upgrading_state: true,
            }),
        ]),
        ..TestRuntime::default()
    };

    SyncUpgradeState(&mut runtime, Duration::from_secs(1)).unwrap();

    assert_eq!(runtime.warnings, vec![("get owner op failed", None)]);
    assert_eq!(runtime.sleeps, vec![Duration::from_millis(200)]);
    assert_eq!(runtime.owner_timeouts.len(), 2);
    assert!(
        runtime
            .owner_timeouts
            .iter()
            .all(|timeout| *timeout <= Duration::from_secs(3))
    );
    assert_eq!(
        runtime.updates,
        vec![(ServerState::Upgrading, Duration::from_secs(1))]
    );
}

#[test]
fn sync_normal_running_preserves_all_go_errors_and_continues() {
    let mut runtime = TestRuntime {
        resume_result: Some((
            vec![TestError("job one"), TestError("job two")],
            Some(TestError("resume all")),
        )),
        task_manager_available: true,
        adjust_result: Err(TestError("adjust")),
        ..TestRuntime::default()
    };

    SyncNormalRunning(&mut runtime).unwrap();

    assert_eq!(
        runtime.warnings,
        vec![
            (
                "resume all paused jobs failed",
                Some(TestError("resume all"))
            ),
            ("resume the job failed", Some(TestError("job one"))),
            ("resume the job failed", Some(TestError("job two"))),
            (
                "cannot adjust task overflow concurrency",
                Some(TestError("adjust"))
            ),
        ]
    );
    assert_eq!(
        runtime.updates,
        vec![(ServerState::NormalRunning, Duration::from_secs(3))]
    );
}

#[test]
fn upgrading_state_query_uses_three_second_timeout() {
    let mut runtime = TestRuntime {
        global_results: VecDeque::from([
            Ok(ServerState::Upgrading),
            Ok(ServerState::NormalRunning),
        ]),
        ..TestRuntime::default()
    };

    assert_eq!(IsUpgradingClusterState(&mut runtime), Ok(true));
    assert_eq!(IsUpgradingClusterState(&mut runtime), Ok(false));
    assert_eq!(runtime.global_timeouts, vec![Duration::from_secs(3); 2]);
}

#[test]
fn upgrading_state_retry_logs_periodic_error_then_success() {
    let mut runtime = TestRuntime {
        global_results: VecDeque::from([Err(TestError("read")), Ok(ServerState::Upgrading)]),
        ..TestRuntime::default()
    };

    isUpgradingClusterStateWithRetry(&mut runtime, 10, 11, Duration::from_secs(1));

    assert_eq!(
        runtime.warnings,
        vec![("get global state failed", Some(TestError("read")))]
    );
    assert_eq!(runtime.states, vec![(10, 11, true)]);
    assert_eq!(runtime.sleeps, vec![Duration::from_millis(200)]);
}
