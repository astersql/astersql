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

// DXF scheduler-overhead benchmark contract, ported from bench_test.go.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use astersql_dxf_framework_proto as proto;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BenchFlag {
    name: &'static str,
    description: &'static str,
}

impl BenchFlag {
    const fn new(name: &'static str, description: &'static str) -> Self {
        Self { name, description }
    }
}

const BENCH_FLAGS: [BenchFlag; 6] = [
    BenchFlag::new("max-concurrent-task", "max concurrent task"),
    BenchFlag::new("task-wait-duration", "task wait duration"),
    BenchFlag::new("scheduler-interval", "scheduler interval"),
    BenchFlag::new("task-executor-mgr-interval", "task executor mgr interval"),
    BenchFlag::new("task-meta-size", "task meta size"),
    BenchFlag::new("no-task", "no task"),
];

#[derive(Clone, Debug, Eq, PartialEq)]
struct BenchOptions {
    max_concurrent_tasks: i32,
    wait_duration: Duration,
    scheduler_interval: Duration,
    task_executor_manager_interval: Duration,
    task_meta_size: usize,
    no_task: bool,
}

impl Default for BenchOptions {
    fn default() -> Self {
        Self {
            // Go captures these values when package-level flags are registered.
            max_concurrent_tasks: proto::DefaultMaxConcurrentTask,
            wait_duration: Duration::from_secs(2 * 60),
            scheduler_interval: Duration::from_millis(500),
            task_executor_manager_interval: Duration::from_millis(300),
            task_meta_size: 1 << 10,
            no_task: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum BenchError {
    InvalidConcurrency(i32),
    ConcurrencyOverflow(i32),
    Operation(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BenchSubmission {
    task_key: String,
    task_type: proto::TaskType,
    required_slots: i32,
    target_scope: String,
    max_node_count: i32,
    meta: Vec<u8>,
}

struct BenchWorkload {
    options: BenchOptions,
    submission_count: usize,
    context_cpu_count: usize,
}

impl BenchWorkload {
    fn new(options: BenchOptions) -> Result<Self, BenchError> {
        if options.max_concurrent_tasks <= 0 {
            return Err(BenchError::InvalidConcurrency(options.max_concurrent_tasks));
        }
        let submission_count =
            options
                .max_concurrent_tasks
                .checked_mul(4)
                .ok_or(BenchError::ConcurrencyOverflow(
                    options.max_concurrent_tasks,
                ))?;
        let context_cpu_count =
            options
                .max_concurrent_tasks
                .checked_mul(2)
                .ok_or(BenchError::ConcurrencyOverflow(
                    options.max_concurrent_tasks,
                ))?;
        Ok(Self {
            options,
            submission_count: submission_count as usize,
            context_cpu_count: context_cpu_count as usize,
        })
    }

    fn context_cpu_count(&self) -> usize {
        self.context_cpu_count
    }

    fn submissions(&self) -> impl Iterator<Item = BenchSubmission> + '_ {
        let count = if self.options.no_task {
            0
        } else {
            self.submission_count
        };
        (0..count).map(|index| BenchSubmission {
            task_key: format!("task-{index:03}"),
            task_type: proto::TaskTypeExample,
            required_slots: 1,
            target_scope: String::new(),
            max_node_count: 0,
            meta: vec![0; self.options.task_meta_size],
        })
    }

    fn waited_task_keys(&self) -> impl Iterator<Item = String> + '_ {
        let count = if self.options.no_task {
            0
        } else {
            self.options.max_concurrent_tasks as usize
        };
        (0..count).map(|index| format!("task-{index:03}"))
    }

    fn initial_wait_duration(&self) -> Duration {
        if self.options.no_task {
            self.options.wait_duration
        } else {
            self.options.wait_duration.saturating_mul(2)
        }
    }
}

struct BenchScheduler;

impl BenchScheduler {
    fn on_tick(&self) {}

    fn next_step(&self, current: proto::Step) -> proto::Step {
        match current {
            proto::StepInit => proto::StepOne,
            proto::StepOne => proto::StepTwo,
            proto::StepTwo => proto::StepDone,
            // A missing Go map key yields the Step zero value.
            _ => 0,
        }
    }

    fn next_subtasks_batch(&self, task_key: &str) -> Vec<Vec<u8>> {
        vec![task_key.as_bytes().to_vec()]
    }

    fn eligible_instances(&self) -> Vec<String> {
        Vec::new()
    }

    fn is_retryable_error(&self) -> bool {
        false
    }

    fn on_done(&self) -> Result<(), BenchError> {
        Ok(())
    }
}

trait TaskCanceller: Send + Sync {
    fn cancel_task(&self, task_id: i64) -> Result<(), BenchError>;
}

#[derive(Clone, Default)]
struct Cancellation {
    state: Arc<(Mutex<bool>, Condvar)>,
}

impl Cancellation {
    fn cancel(&self) {
        let (cancelled, wake) = &*self.state;
        *cancelled.lock().unwrap() = true;
        wake.notify_all();
    }

    fn wait_cancelled(&self, timeout: Duration) -> bool {
        let (cancelled, wake) = &*self.state;
        let cancelled = cancelled.lock().unwrap();
        if *cancelled {
            return true;
        }
        *wake
            .wait_timeout_while(cancelled, timeout, |cancelled| !*cancelled)
            .unwrap()
            .0
    }
}

fn run_bench_subtask(
    cancellation: &Cancellation,
    wait_duration: Duration,
    task_id: i64,
    get_task_manager: impl FnOnce() -> Result<Arc<dyn TaskCanceller>, BenchError>,
) -> Result<(), BenchError> {
    if !cancellation.wait_cancelled(wait_duration) {
        return Ok(());
    }
    get_task_manager()?.cancel_task(task_id)
}

trait BenchEnvironment {
    fn enable_disable_dist_task_failpoint(&self) -> Result<(), BenchError>;
    fn open_tikv(&self, address: &str) -> Result<(), BenchError>;
    fn set_store_type_tikv(&self) -> Result<(), BenchError>;
    fn start_owner_manager(&self) -> Result<(), BenchError>;
    fn bootstrap_session(&self) -> Result<(), BenchError>;
    fn execute(&self, sql: &str) -> Result<(), BenchError>;
    fn close_domain(&self);
    fn close_owner_manager(&self);
    fn close_store(&self) -> Result<(), BenchError>;
    fn stop_views(&self);
}

struct PreparedBench<'a> {
    environment: &'a dyn BenchEnvironment,
    domain_open: bool,
    owner_started: bool,
    store_open: bool,
    views_started: bool,
    active: bool,
}

impl PreparedBench<'_> {
    fn close(mut self) -> Result<(), BenchError> {
        self.cleanup()
    }

    fn cleanup(&mut self) -> Result<(), BenchError> {
        if !self.active {
            return Ok(());
        }
        if self.domain_open {
            self.environment.close_domain();
        }
        if self.owner_started {
            self.environment.close_owner_manager();
        }
        let close_result = if self.store_open {
            self.environment.close_store()
        } else {
            Ok(())
        };
        if self.views_started {
            self.environment.stop_views();
        }
        self.active = false;
        close_result
    }
}

impl Drop for PreparedBench<'_> {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

fn prepare_for_bench_test(
    environment: &dyn BenchEnvironment,
    tikv_address: &str,
) -> Result<(), BenchError> {
    environment.enable_disable_dist_task_failpoint()?;
    environment.open_tikv(&format!("tikv://{tikv_address}"))?;
    let mut prepared = PreparedBench {
        environment,
        domain_open: false,
        owner_started: false,
        store_open: true,
        views_started: false,
        active: true,
    };
    environment.set_store_type_tikv()?;
    environment.start_owner_manager()?;
    prepared.owner_started = true;
    environment.bootstrap_session()?;
    prepared.domain_open = true;
    prepared.views_started = true;
    for table in [
        "mysql.tidb_global_task",
        "mysql.tidb_global_task_history",
        "mysql.tidb_background_subtask",
        "mysql.tidb_background_subtask_history",
    ] {
        environment.execute(&format!("delete from {table}"))?;
    }
    // The defer in Go's prepareForBenchTest runs before the helper returns.
    prepared.close()
}

trait BenchControl {
    fn start_status_server(&self, port: &str) -> Result<(), BenchError>;
    fn stop_status_server(&self);
    fn max_concurrent_tasks(&self) -> i32;
    fn set_max_concurrent_tasks(&self, value: i32);
    fn scheduler_interval(&self) -> Duration;
    fn set_scheduler_interval(&self, value: Duration);
    fn task_executor_manager_interval(&self) -> Duration;
    fn set_task_executor_manager_interval(&self, value: Duration);
    fn log(&self, message: String);
    fn new_test_context(
        &self,
        node_count: usize,
        cpu_count: usize,
        reduce_check_interval: bool,
    ) -> Result<(), BenchError>;
    fn register_task_type(&self, scheduler: &BenchScheduler) -> Result<(), BenchError>;
    fn submit_task(&self, submission: BenchSubmission) -> Result<(), BenchError>;
    fn sleep(&self, duration: Duration);
    fn wait_task_done_or_paused(&self, task_key: &str) -> Result<(), BenchError>;
}

struct BenchmarkCleanup<'a> {
    control: &'a dyn BenchControl,
    max_concurrent_tasks: i32,
    scheduler_interval: Duration,
    task_executor_manager_interval: Duration,
    active: bool,
}

impl<'a> BenchmarkCleanup<'a> {
    fn apply(control: &'a dyn BenchControl, options: &BenchOptions) -> Self {
        let cleanup = Self {
            control,
            max_concurrent_tasks: control.max_concurrent_tasks(),
            scheduler_interval: control.scheduler_interval(),
            task_executor_manager_interval: control.task_executor_manager_interval(),
            active: true,
        };
        control.set_max_concurrent_tasks(options.max_concurrent_tasks);
        control.set_scheduler_interval(options.scheduler_interval);
        control.set_task_executor_manager_interval(options.task_executor_manager_interval);
        cleanup
    }

    fn close(mut self) {
        self.cleanup();
    }

    fn cleanup(&mut self) {
        if !self.active {
            return;
        }
        // Go runs the function defer before testing.B cleanup callbacks.
        self.control.stop_status_server();
        self.control
            .set_max_concurrent_tasks(self.max_concurrent_tasks);
        self.control.set_scheduler_interval(self.scheduler_interval);
        self.control
            .set_task_executor_manager_interval(self.task_executor_manager_interval);
        self.active = false;
    }
}

impl Drop for BenchmarkCleanup<'_> {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn benchmark_scheduler_overhead(
    environment: &dyn BenchEnvironment,
    control: &dyn BenchControl,
    tikv_address: &str,
    options: BenchOptions,
) -> Result<(), BenchError> {
    let workload = BenchWorkload::new(options.clone())?;
    control.start_status_server("10080")?;
    let cleanup = BenchmarkCleanup::apply(control, &options);

    control.log(format!(
        "max concurrent task: {}",
        options.max_concurrent_tasks
    ));
    control.log(format!("taks wait duration: {:?}", options.wait_duration));
    control.log(format!("task meta size: {}", options.task_meta_size));
    control.log(format!(
        "scheduler interval: {:?}",
        options.scheduler_interval
    ));
    control.log(format!(
        "task executor mgr interval: {:?}",
        options.task_executor_manager_interval
    ));

    prepare_for_bench_test(environment, tikv_address)?;
    control.new_test_context(1, workload.context_cpu_count(), false)?;
    let scheduler = BenchScheduler;
    control.register_task_type(&scheduler)?;

    for submission in workload.submissions() {
        control.submit_task(submission)?;
    }
    control.sleep(workload.initial_wait_duration());
    for task_key in workload.waited_task_keys() {
        control.wait_task_done_or_paused(&task_key)?;
    }

    cleanup.close();
    Ok(())
}

#[test]
fn benchmark_flags_keep_go_names_descriptions_and_defaults() {
    let options = BenchOptions::default();

    assert_eq!(
        BENCH_FLAGS,
        [
            BenchFlag::new("max-concurrent-task", "max concurrent task"),
            BenchFlag::new("task-wait-duration", "task wait duration"),
            BenchFlag::new("scheduler-interval", "scheduler interval"),
            BenchFlag::new("task-executor-mgr-interval", "task executor mgr interval"),
            BenchFlag::new("task-meta-size", "task meta size"),
            BenchFlag::new("no-task", "no task"),
        ]
    );
    assert_eq!(
        options.max_concurrent_tasks,
        proto::DefaultMaxConcurrentTask
    );
    assert_eq!(options.wait_duration, Duration::from_secs(120));
    assert_eq!(options.scheduler_interval, Duration::from_millis(500));
    assert_eq!(
        options.task_executor_manager_interval,
        Duration::from_millis(300)
    );
    assert_eq!(options.task_meta_size, 1 << 10);
    assert!(!options.no_task);
}

#[test]
fn scheduler_workload_keeps_queue_width_keys_meta_and_wait_boundary() {
    let options = BenchOptions {
        max_concurrent_tasks: 3,
        wait_duration: Duration::from_millis(7),
        task_meta_size: 4,
        ..BenchOptions::default()
    };
    let workload = BenchWorkload::new(options.clone()).unwrap();
    let submissions = workload.submissions().collect::<Vec<_>>();

    assert_eq!(workload.context_cpu_count(), 6);
    assert_eq!(submissions.len(), 12);
    assert_eq!(submissions.first().unwrap().task_key, "task-000");
    assert_eq!(submissions.last().unwrap().task_key, "task-011");
    assert!(submissions.iter().all(|submission| {
        submission.task_type == proto::TaskTypeExample
            && submission.required_slots == 1
            && submission.target_scope.is_empty()
            && submission.max_node_count == 0
            && submission.meta == vec![0; 4]
    }));
    assert_eq!(
        workload.waited_task_keys().collect::<Vec<_>>(),
        ["task-000", "task-001", "task-002"]
    );
    assert_eq!(workload.initial_wait_duration(), Duration::from_millis(14));

    let no_task = BenchWorkload::new(BenchOptions {
        no_task: true,
        ..options
    })
    .unwrap();
    assert_eq!(no_task.submissions().count(), 0);
    assert_eq!(no_task.waited_task_keys().count(), 0);
    assert_eq!(no_task.initial_wait_duration(), Duration::from_millis(7));
}

#[test]
fn scheduler_workload_rejects_invalid_or_overflowing_concurrency() {
    assert!(matches!(
        BenchWorkload::new(BenchOptions {
            max_concurrent_tasks: 0,
            ..BenchOptions::default()
        }),
        Err(BenchError::InvalidConcurrency(0))
    ));
    assert!(matches!(
        BenchWorkload::new(BenchOptions {
            max_concurrent_tasks: i32::MAX,
            ..BenchOptions::default()
        }),
        Err(BenchError::ConcurrencyOverflow(i32::MAX))
    ));
}

#[test]
fn benchmark_scheduler_keeps_go_steps_and_task_key_meta() {
    let scheduler = BenchScheduler;

    scheduler.on_tick();
    assert_eq!(scheduler.next_step(proto::StepInit), proto::StepOne);
    assert_eq!(scheduler.next_step(proto::StepOne), proto::StepTwo);
    assert_eq!(scheduler.next_step(proto::StepTwo), proto::StepDone);
    assert_eq!(scheduler.next_step(proto::StepThree), 0);
    assert_eq!(
        scheduler.next_subtasks_batch("task-007"),
        vec![b"task-007".to_vec()]
    );
    assert!(scheduler.eligible_instances().is_empty());
    assert!(!scheduler.is_retryable_error());
    assert!(scheduler.on_done().is_ok());
}

struct RecordingBenchControlState {
    max_concurrent_tasks: i32,
    scheduler_interval: Duration,
    task_executor_manager_interval: Duration,
    events: Vec<String>,
    fail_submission: Option<String>,
}

struct RecordingBenchControl {
    state: Mutex<RecordingBenchControlState>,
}

impl RecordingBenchControl {
    fn new(fail_submission: Option<&str>) -> Self {
        Self {
            state: Mutex::new(RecordingBenchControlState {
                max_concurrent_tasks: 19,
                scheduler_interval: Duration::from_millis(900),
                task_executor_manager_interval: Duration::from_millis(800),
                events: Vec::new(),
                fail_submission: fail_submission.map(str::to_owned),
            }),
        }
    }
}

impl BenchControl for RecordingBenchControl {
    fn start_status_server(&self, port: &str) -> Result<(), BenchError> {
        self.state
            .lock()
            .unwrap()
            .events
            .push(format!("start-status:{port}"));
        Ok(())
    }

    fn stop_status_server(&self) {
        self.state.lock().unwrap().events.push("stop-status".into());
    }

    fn max_concurrent_tasks(&self) -> i32 {
        self.state.lock().unwrap().max_concurrent_tasks
    }

    fn set_max_concurrent_tasks(&self, value: i32) {
        let mut state = self.state.lock().unwrap();
        state.max_concurrent_tasks = value;
        state.events.push(format!("set-max:{value}"));
    }

    fn scheduler_interval(&self) -> Duration {
        self.state.lock().unwrap().scheduler_interval
    }

    fn set_scheduler_interval(&self, value: Duration) {
        let mut state = self.state.lock().unwrap();
        state.scheduler_interval = value;
        state.events.push(format!("set-scheduler:{value:?}"));
    }

    fn task_executor_manager_interval(&self) -> Duration {
        self.state.lock().unwrap().task_executor_manager_interval
    }

    fn set_task_executor_manager_interval(&self, value: Duration) {
        let mut state = self.state.lock().unwrap();
        state.task_executor_manager_interval = value;
        state.events.push(format!("set-executor:{value:?}"));
    }

    fn log(&self, message: String) {
        self.state
            .lock()
            .unwrap()
            .events
            .push(format!("log:{message}"));
    }

    fn new_test_context(
        &self,
        node_count: usize,
        cpu_count: usize,
        reduce_check_interval: bool,
    ) -> Result<(), BenchError> {
        self.state.lock().unwrap().events.push(format!(
            "context:{node_count}:{cpu_count}:{reduce_check_interval}"
        ));
        Ok(())
    }

    fn register_task_type(&self, scheduler: &BenchScheduler) -> Result<(), BenchError> {
        assert_eq!(scheduler.next_step(proto::StepInit), proto::StepOne);
        self.state
            .lock()
            .unwrap()
            .events
            .push("register-task-type".into());
        Ok(())
    }

    fn submit_task(&self, submission: BenchSubmission) -> Result<(), BenchError> {
        let mut state = self.state.lock().unwrap();
        state.events.push(format!(
            "submit:{}:{}",
            submission.task_key,
            submission.meta.len()
        ));
        if state.fail_submission.as_deref() == Some(submission.task_key.as_str()) {
            return Err(BenchError::Operation("submit failed".into()));
        }
        Ok(())
    }

    fn sleep(&self, duration: Duration) {
        self.state
            .lock()
            .unwrap()
            .events
            .push(format!("sleep:{duration:?}"));
    }

    fn wait_task_done_or_paused(&self, task_key: &str) -> Result<(), BenchError> {
        self.state
            .lock()
            .unwrap()
            .events
            .push(format!("wait:{task_key}"));
        Ok(())
    }
}

#[test]
fn scheduler_overhead_runs_full_go_workload_and_restores_settings() {
    let environment = RecordingBenchEnvironment::default();
    let control = RecordingBenchControl::new(None);
    benchmark_scheduler_overhead(
        &environment,
        &control,
        "pd:2379",
        BenchOptions {
            max_concurrent_tasks: 3,
            wait_duration: Duration::from_millis(2),
            scheduler_interval: Duration::from_millis(5),
            task_executor_manager_interval: Duration::from_millis(6),
            task_meta_size: 2,
            no_task: false,
        },
    )
    .unwrap();

    let state = control.state.lock().unwrap();
    assert_eq!(state.max_concurrent_tasks, 19);
    assert_eq!(state.scheduler_interval, Duration::from_millis(900));
    assert_eq!(
        state.task_executor_manager_interval,
        Duration::from_millis(800)
    );
    assert!(state.events.contains(&"context:1:6:false".into()));
    assert!(state.events.contains(&"register-task-type".into()));
    assert_eq!(
        state
            .events
            .iter()
            .filter(|event| event.starts_with("submit:"))
            .count(),
        12
    );
    assert_eq!(
        state
            .events
            .iter()
            .filter(|event| event.starts_with("wait:"))
            .count(),
        3
    );
    assert!(state.events.contains(&"sleep:4ms".into()));
    assert_eq!(
        &state.events[state.events.len() - 4..],
        [
            "stop-status",
            "set-max:19",
            "set-scheduler:900ms",
            "set-executor:800ms",
        ]
    );
}

#[test]
fn scheduler_overhead_restores_status_and_settings_after_submit_error() {
    let environment = RecordingBenchEnvironment::default();
    let control = RecordingBenchControl::new(Some("task-002"));
    let error = benchmark_scheduler_overhead(
        &environment,
        &control,
        "pd:2379",
        BenchOptions {
            max_concurrent_tasks: 3,
            wait_duration: Duration::from_millis(1),
            task_meta_size: 1,
            ..BenchOptions::default()
        },
    )
    .unwrap_err();

    assert_eq!(error, BenchError::Operation("submit failed".into()));
    let state = control.state.lock().unwrap();
    assert_eq!(state.max_concurrent_tasks, 19);
    assert_eq!(state.scheduler_interval, Duration::from_millis(900));
    assert_eq!(
        state.task_executor_manager_interval,
        Duration::from_millis(800)
    );
    assert!(state.events.contains(&"stop-status".into()));
}

#[derive(Default)]
struct RecordingTaskManager {
    cancelled_task_ids: Mutex<Vec<i64>>,
    failure: Option<&'static str>,
}

impl TaskCanceller for RecordingTaskManager {
    fn cancel_task(&self, task_id: i64) -> Result<(), BenchError> {
        self.cancelled_task_ids.lock().unwrap().push(task_id);
        match self.failure {
            Some(message) => Err(BenchError::Operation(message.to_owned())),
            None => Ok(()),
        }
    }
}

#[test]
fn subtask_wait_cancels_task_and_propagates_lookup_or_cancel_errors() {
    let cancellation = Cancellation::default();
    cancellation.cancel();
    let manager = Arc::new(RecordingTaskManager::default());
    run_bench_subtask(&cancellation, Duration::from_secs(120), 42, || {
        Ok(manager.clone())
    })
    .unwrap();
    assert_eq!(*manager.cancelled_task_ids.lock().unwrap(), [42]);

    let lookup_error = run_bench_subtask(&cancellation, Duration::from_secs(120), 43, || {
        Err(BenchError::Operation("manager unavailable".into()))
    })
    .unwrap_err();
    assert_eq!(
        lookup_error,
        BenchError::Operation("manager unavailable".into())
    );

    let failing_manager = Arc::new(RecordingTaskManager {
        failure: Some("cancel failed"),
        ..RecordingTaskManager::default()
    });
    let cancel_error = run_bench_subtask(&cancellation, Duration::from_secs(120), 44, || {
        Ok(failing_manager)
    })
    .unwrap_err();
    assert_eq!(cancel_error, BenchError::Operation("cancel failed".into()));
}

#[test]
fn subtask_wait_timeout_succeeds_without_loading_task_manager() {
    let cancellation = Cancellation::default();
    let manager_lookups = Mutex::new(0);

    run_bench_subtask(&cancellation, Duration::from_millis(1), 42, || {
        *manager_lookups.lock().unwrap() += 1;
        Ok(Arc::new(RecordingTaskManager::default()))
    })
    .unwrap();
    assert_eq!(*manager_lookups.lock().unwrap(), 0);
}

#[derive(Default)]
struct RecordingBenchEnvironment {
    events: Mutex<Vec<String>>,
    failing_sql: Option<&'static str>,
}

impl RecordingBenchEnvironment {
    fn record(&self, event: impl Into<String>) -> Result<(), BenchError> {
        self.events.lock().unwrap().push(event.into());
        Ok(())
    }
}

impl BenchEnvironment for RecordingBenchEnvironment {
    fn enable_disable_dist_task_failpoint(&self) -> Result<(), BenchError> {
        self.record("enable-failpoint")
    }

    fn open_tikv(&self, address: &str) -> Result<(), BenchError> {
        self.record(format!("open:{address}"))
    }

    fn set_store_type_tikv(&self) -> Result<(), BenchError> {
        self.record("store-type:tikv")
    }

    fn start_owner_manager(&self) -> Result<(), BenchError> {
        self.record("start-owner")
    }

    fn bootstrap_session(&self) -> Result<(), BenchError> {
        self.record("bootstrap-session")
    }

    fn execute(&self, sql: &str) -> Result<(), BenchError> {
        self.record(format!("sql:{sql}"))?;
        if self.failing_sql == Some(sql) {
            return Err(BenchError::Operation("sql failed".into()));
        }
        Ok(())
    }

    fn close_domain(&self) {
        self.events.lock().unwrap().push("close-domain".into());
    }

    fn close_owner_manager(&self) {
        self.events.lock().unwrap().push("close-owner".into());
    }

    fn close_store(&self) -> Result<(), BenchError> {
        self.record("close-store")
    }

    fn stop_views(&self) {
        self.events.lock().unwrap().push("stop-views".into());
    }
}

#[test]
fn prepare_for_bench_cleans_tables_and_releases_resources_in_go_order() {
    let environment = RecordingBenchEnvironment::default();
    prepare_for_bench_test(&environment, "upstream-pd:2379?disableGC=true").unwrap();
    assert_eq!(
        *environment.events.lock().unwrap(),
        [
            "enable-failpoint",
            "open:tikv://upstream-pd:2379?disableGC=true",
            "store-type:tikv",
            "start-owner",
            "bootstrap-session",
            "sql:delete from mysql.tidb_global_task",
            "sql:delete from mysql.tidb_global_task_history",
            "sql:delete from mysql.tidb_background_subtask",
            "sql:delete from mysql.tidb_background_subtask_history",
            "close-domain",
            "close-owner",
            "close-store",
            "stop-views",
        ]
    );
}

#[test]
fn prepare_for_bench_releases_acquired_resources_after_sql_error() {
    let environment = RecordingBenchEnvironment {
        failing_sql: Some("delete from mysql.tidb_global_task_history"),
        ..RecordingBenchEnvironment::default()
    };
    let error = prepare_for_bench_test(&environment, "pd:2379").unwrap_err();

    assert_eq!(error, BenchError::Operation("sql failed".into()));
    assert_eq!(
        &environment.events.lock().unwrap()[7..],
        ["close-domain", "close-owner", "close-store", "stop-views"]
    );
}
