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

// Scheduler Manager 的轻量回归测试。
//
// 通过内存版 `TestTaskManager` 驱动真实 `Manager`/`BaseScheduler`，覆盖调度器排序、
// 终态任务清理、无需资源的任务状态、初始化失败，以及并发配置边界；不依赖 TestKit。

use crate::test_support::{TestExtension, TestTaskManager, task};
use crate::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

struct HoldingScheduler {
    task: Task,
    extension: Arc<TestExtension>,
}

impl Scheduler for HoldingScheduler {
    fn init(&self) -> Result<()> {
        Ok(())
    }

    fn schedule_once(&self) -> Result<bool> {
        Ok(false)
    }

    fn close(&self) {}

    fn task(&self) -> Task {
        self.task.clone()
    }

    fn extension(&self) -> Arc<dyn Extension> {
        self.extension.clone()
    }
}

/// 为指定任务类型注册使用测试扩展点的基础调度器工厂。
fn register_base_scheduler(task_type: &str) {
    RegisterSchedulerFactory(
        task_type,
        Arc::new(|task, param| {
            Arc::new(BaseScheduler::new(
                task,
                param,
                Arc::new(TestExtension::default()),
            ))
        }),
    );
}

/// 构造排序字段可控的任务；创建时间由 ID 派生，便于稳定验证排名规则。
fn manager_task(id: i64, task_type: &str, state: TaskState) -> Task {
    let mut task = task(id, state);
    task.base.task_type = task_type.to_owned();
    task.base.create_time = SystemTime::UNIX_EPOCH + Duration::from_secs(id as u64);
    task
}

#[test]
/// 验证管理器返回的调度器按任务排名排序，而非依赖哈希表迭代顺序。
fn test_manager_schedulers_ordered() {
    let task_type = "manager-order";
    register_base_scheduler(task_type);
    let task_manager = Arc::new(TestTaskManager::default());
    let mut low = manager_task(2, task_type, "waiting");
    low.base.priority = 512;
    let mut high = manager_task(1, task_type, "waiting");
    high.base.priority = 1;
    task_manager.insert_task(low.clone());
    task_manager.insert_task(high.clone());
    *task_manager.top_unfinished.lock().unwrap() = vec![low.base, high.base];
    let manager = Manager::new(task_manager, "server", None);
    manager.start().unwrap();
    manager.tick().unwrap();

    let ids = manager
        .schedulers()
        .into_iter()
        .map(|scheduler| scheduler.task().base.id)
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![1, 2]);
}

#[test]
/// 终态任务即使没有类型专属清理器，也应迁入历史记录。
fn test_scheduler_clean_task() {
    let task_manager = Arc::new(TestTaskManager::default());
    task_manager.insert_task(manager_task(1, "no-cleanup", TASK_STATE_REVERTED));
    let manager = Manager::new(task_manager.clone(), "server", None);
    assert_eq!(manager.clean_finished_tasks().unwrap(), 1);
    assert_eq!(task_manager.transferred_tasks.lock().unwrap().len(), 1);
}

#[derive(Default)]
struct ExpiredCleanerRecorder {
    calls: Mutex<Vec<String>>,
    fail: bool,
}

impl Cleaner for ExpiredCleanerRecorder {
    fn clean(&self, _task: &mut Task) -> Result<()> {
        Ok(())
    }

    fn expired_file_cleaner(&self) -> Option<&dyn ExpiredFileCleaner> {
        Some(self)
    }
}

impl ExpiredFileCleaner for ExpiredCleanerRecorder {
    fn clean_expired_files(
        &self,
        _context: &Context,
        _task_info_getter: &dyn TaskManager,
        cloud_storage_uri: &str,
    ) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(cloud_storage_uri.to_owned());
        if self.fail {
            Err(SchedulerError::new("expired cleanup failed"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn expired_file_cleanup_runs_capable_factories_and_counts_failures() {
    let successful = Arc::new(ExpiredCleanerRecorder::default());
    let failing = Arc::new(ExpiredCleanerRecorder {
        fail: true,
        ..Default::default()
    });
    RegisterCleanerFactory("success", {
        let cleaner = successful.clone();
        Arc::new(move || cleaner.clone())
    });
    RegisterCleanerFactory("failure", {
        let cleaner = failing.clone();
        Arc::new(move || cleaner.clone())
    });
    RegisterCleanerFactory("ordinary", Arc::new(|| Arc::new(FailingSingleCleaner)));
    let counter = &astersql_dxf_framework_dxfmetric::InitDistTaskMetrics().ScheduleEventCounter;
    let before = counter
        .with_label_values(&[
            "-",
            astersql_dxf_framework_dxfmetric::EventExpiredFileCleanupFailed,
        ])
        .get();
    let manager = Manager::new(Arc::new(TestTaskManager::default()), "server", None);
    manager.run_expired_file_clean(&Context::new(), "memstore://expired");

    assert_eq!(
        *successful.calls.lock().unwrap(),
        vec!["memstore://expired"]
    );
    assert_eq!(*failing.calls.lock().unwrap(), vec!["memstore://expired"]);
    assert_eq!(
        counter
            .with_label_values(&[
                "-",
                astersql_dxf_framework_dxfmetric::EventExpiredFileCleanupFailed
            ])
            .get()
            - before,
        1.0
    );
}

#[test]
/// Cancelling/Reverting/Pausing 属于无需执行资源的状态，启动时不得预留 slot。
fn test_manager_scheduler_not_allocate_slots() {
    let task_type = "manager-no-slots";
    let allocations = Arc::new(Mutex::new(HashMap::new()));
    RegisterSchedulerFactory(
        task_type,
        Arc::new({
            let allocations = allocations.clone();
            move |task, param| {
                allocations
                    .lock()
                    .unwrap()
                    .insert(task.base.id, param.allocated_slots);
                Arc::new(HoldingScheduler {
                    task,
                    extension: Arc::new(TestExtension::default()),
                })
            }
        }),
    );
    let task_manager = Arc::new(TestTaskManager::default());
    let states = [
        TASK_STATE_CANCELLING,
        TASK_STATE_REVERTING,
        TASK_STATE_PAUSING,
    ];
    let mut tasks = Vec::new();
    for (offset, state) in states.into_iter().enumerate() {
        let task = manager_task(1 + offset as i64, task_type, state);
        tasks.push(task.base.clone());
        task_manager.insert_task(task);
    }
    *task_manager.top_unfinished.lock().unwrap() = tasks;
    let manager = Manager::new(task_manager, "server", None);
    manager.start().unwrap();
    manager.tick().unwrap();
    assert_eq!(manager.scheduler_count(), 3);
    assert!(
        allocations
            .lock()
            .unwrap()
            .values()
            .all(|allocated| !allocated)
    );
}

#[test]
/// 非法 keyspace 使调度器初始化失败：不得加入运行表，并须将任务标记失败。
fn test_start_scheduler_cross_keyspace_runtime() {
    let task_type = "manager-keyspace";
    register_base_scheduler(task_type);
    let task_manager = Arc::new(TestTaskManager::default());
    let mut task = manager_task(7, task_type, TASK_STATE_PENDING);
    task.base.keyspace = "bad\0keyspace".to_owned();
    task_manager.insert_task(task.clone());
    *task_manager.top_unfinished.lock().unwrap() = vec![task.base];
    *task_manager.nodes.lock().unwrap() = vec![ManagedNode {
        id: "n1".to_owned(),
        role: String::new(),
        cpu_count: 8,
    }];
    task_manager
        .used_slots
        .lock()
        .unwrap()
        .insert("n1".to_owned(), 0);
    let manager = Manager::new(task_manager.clone(), "server", None);
    manager.start().unwrap();
    manager.tick().unwrap();
    assert_eq!(manager.scheduler_count(), 0);
    assert_eq!(task_manager.failed_tasks.lock().unwrap().len(), 1);
}

#[test]
fn manager_failed_task_updates_metric() {
    use astersql_dxf_framework_dxfmetric::InitDistTaskMetrics;

    let _metric_guard = crate::scheduler::FINISHED_TASK_METRIC_TEST_LOCK
        .lock()
        .unwrap();
    let counter = &InitDistTaskMetrics().FinishedTaskCounter;
    let all_before = counter.with_label_values(&["all"]).get();
    let failed_before = counter.with_label_values(&["failed"]).get();

    let task_manager = Arc::new(TestTaskManager::default());
    let unknown = manager_task(701, "go-commit-7d70c1c438-unknown", TASK_STATE_PENDING);
    task_manager.insert_task(unknown.clone());
    *task_manager.top_unfinished.lock().unwrap() = vec![unknown.base];
    let manager = Manager::new(task_manager.clone(), "server", None);
    manager.start().unwrap();
    manager.tick().unwrap();
    assert_eq!(task_manager.failed_tasks.lock().unwrap().len(), 1);
    assert_eq!(counter.with_label_values(&["all"]).get() - all_before, 1.0);
    assert_eq!(
        counter.with_label_values(&["failed"]).get() - failed_before,
        1.0
    );
}

#[test]
/// 达到调度器上限后，取消/回滚/修改/暂停任务仍须快速启动且不占 slot。
fn test_fast_respond_no_need_resource_task_when_schedulers_reach_limit() {
    SetMaxConcurrentTask(DEFAULT_MAX_CONCURRENT_TASKS).unwrap();
    let task_type = "manager-at-limit";
    let allocations = Arc::new(Mutex::new(HashMap::new()));
    RegisterSchedulerFactory(
        task_type,
        Arc::new({
            let allocations = allocations.clone();
            move |task, param| {
                allocations
                    .lock()
                    .unwrap()
                    .insert(task.base.id, param.allocated_slots);
                Arc::new(HoldingScheduler {
                    task,
                    extension: Arc::new(TestExtension::default()),
                })
            }
        }),
    );

    let task_manager = Arc::new(TestTaskManager::default());
    let mut resource_tasks = Vec::new();
    for id in 1..=DEFAULT_MAX_CONCURRENT_TASKS as i64 {
        let task = manager_task(id, task_type, TASK_STATE_PENDING);
        resource_tasks.push(task.base.clone());
        task_manager.insert_task(task);
    }
    *task_manager.nodes.lock().unwrap() = vec![ManagedNode {
        id: "n1".to_owned(),
        role: String::new(),
        cpu_count: DEFAULT_MAX_CONCURRENT_TASKS as i32,
    }];
    task_manager
        .used_slots
        .lock()
        .unwrap()
        .insert("n1".to_owned(), 0);
    *task_manager.top_unfinished.lock().unwrap() = resource_tasks;
    let manager = Manager::new(task_manager.clone(), "server", None);
    manager.start().unwrap();
    manager.tick().unwrap();
    assert_eq!(manager.scheduler_count(), DEFAULT_MAX_CONCURRENT_TASKS);

    let no_resource_states = [
        TASK_STATE_CANCELLING,
        TASK_STATE_REVERTING,
        TASK_STATE_MODIFYING,
        TASK_STATE_PAUSING,
    ];
    let mut no_resource_tasks = Vec::new();
    for (offset, state) in no_resource_states.into_iter().enumerate() {
        let task = manager_task(100 + offset as i64, task_type, state);
        no_resource_tasks.push(task.base.clone());
        task_manager.insert_task(task);
    }
    *task_manager.top_no_resource.lock().unwrap() = no_resource_tasks;
    manager.tick().unwrap();

    let allocations = allocations.lock().unwrap();
    for id in 100..104 {
        assert_eq!(allocations.get(&id), Some(&false));
    }
    assert_eq!(manager.scheduler_count(), DEFAULT_MAX_CONCURRENT_TASKS + 4);
}

#[test]
fn test_clean_drains_bounded_batches_and_keeps_pending_tasks() {
    let task_manager = Arc::new(TestTaskManager::default());
    *task_manager.cleanup_batch_size.lock().unwrap() = Some(2);
    for (index, state) in [
        TASK_STATE_FAILED,
        TASK_STATE_SUCCEED,
        TASK_STATE_REVERTED,
        TASK_STATE_FAILED,
        TASK_STATE_SUCCEED,
    ]
    .into_iter()
    .enumerate()
    {
        task_manager.insert_task(manager_task(index as i64 + 1, "bounded-cleanup", state));
    }
    task_manager.insert_task(manager_task(6, "bounded-cleanup", TASK_STATE_PENDING));
    let manager = Manager::new(task_manager.clone(), "server", None);
    for expected in [2, 2, 1, 0] {
        assert_eq!(manager.clean_finished_tasks().unwrap(), expected);
    }
    assert_eq!(task_manager.transferred_tasks.lock().unwrap().len(), 5);
    assert_eq!(
        task_manager.task_by_id(6).unwrap().base.state,
        TASK_STATE_PENDING
    );
}

struct FailingSingleCleaner;
impl Cleaner for FailingSingleCleaner {
    fn clean(&self, task: &mut Task) -> Result<()> {
        if task.base.id == 4 {
            return Err(SchedulerError::new("single cleanup failed"));
        }
        task.meta = b"redacted".to_vec();
        Ok(())
    }
}

#[test]
fn test_clean_single_failure_still_transfers_all_tasks_without_cleanup() {
    let task_type = "single-cleanup-failure-retains-unregistered";
    RegisterCleanerFactory(task_type, Arc::new(|| Arc::new(FailingSingleCleaner)));
    let batch = Arc::new(BatchCleanerRecorder::default());
    let batch_factory = batch.clone();
    RegisterCleanerFactory(
        "single-error-batch",
        Arc::new(move || batch_factory.clone()),
    );
    let task_manager = Arc::new(TestTaskManager::default());
    for (id, kind) in [
        (1, "no-cleanup"),
        (2, task_type),
        (3, "single-error-batch"),
        (4, task_type),
        (5, "single-error-batch"),
        (6, "no-cleanup"),
    ] {
        task_manager.insert_task(manager_task(id, kind, TASK_STATE_REVERTED));
    }
    let manager = Manager::new(task_manager.clone(), "server", None);
    assert_eq!(manager.clean_finished_tasks().unwrap(), 3);
    assert!(batch.batches.lock().unwrap().is_empty());
    let transferred = task_manager.transferred_tasks.lock().unwrap();
    let mut ids: Vec<_> = transferred.iter().map(|task| task.base.id).collect();
    ids.sort();
    assert_eq!(ids, vec![1, 2, 6]);
    assert_eq!(
        transferred
            .iter()
            .find(|task| task.base.id == 2)
            .unwrap()
            .meta,
        b"redacted"
    );
}

#[derive(Default)]
struct BatchCleanerRecorder {
    batches: Mutex<Vec<Vec<i64>>>,
    fail: bool,
}
impl Cleaner for BatchCleanerRecorder {
    fn clean(&self, _: &mut Task) -> Result<()> {
        panic!("batch capability must bypass single cleanup")
    }
    fn batch_cleaner(&self) -> Option<&dyn BatchCleaner> {
        Some(self)
    }
}
impl BatchCleaner for BatchCleanerRecorder {
    fn batch_clean(&self, tasks: &mut [Task]) -> Result<()> {
        self.batches
            .lock()
            .unwrap()
            .push(tasks.iter().map(|task| task.base.id).collect());
        for task in tasks {
            task.meta = b"batch-redacted".to_vec();
        }
        if self.fail {
            Err(SchedulerError::new("batch cleanup failed"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn clean_batches_by_capability_and_creates_one_instance_per_type() {
    let task_manager = Arc::new(TestTaskManager::default());
    let a = Arc::new(BatchCleanerRecorder::default());
    let b = Arc::new(BatchCleanerRecorder::default());
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    for (kind, recorder) in [
        ("batch-capability-a", a.clone()),
        ("batch-capability-b", b.clone()),
    ] {
        let calls = calls.clone();
        RegisterCleanerFactory(
            kind,
            Arc::new(move || {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                recorder.clone()
            }),
        );
    }
    RegisterCleanerFactory(
        "batch-capability-single",
        Arc::new(|| Arc::new(FailingSingleCleaner)),
    );
    for (id, kind) in [
        (1, "batch-capability-a"),
        (2, "batch-capability-b"),
        (3, "batch-capability-single"),
        (4, "batch-capability-a"),
        (5, "batch-capability-b"),
        (6, "no-cleanup"),
    ] {
        task_manager.insert_task(manager_task(id, kind, TASK_STATE_REVERTED));
    }
    assert_eq!(
        Manager::new(task_manager.clone(), "server", None)
            .clean_finished_tasks()
            .unwrap(),
        6
    );
    assert_eq!(*a.batches.lock().unwrap(), vec![vec![1, 4]]);
    assert_eq!(*b.batches.lock().unwrap(), vec![vec![2, 5]]);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    let transferred = task_manager.transferred_tasks.lock().unwrap();
    assert_eq!(
        transferred
            .iter()
            .find(|task| task.base.id == 1)
            .unwrap()
            .meta,
        b"batch-redacted"
    );
    assert_eq!(
        transferred
            .iter()
            .find(|task| task.base.id == 3)
            .unwrap()
            .meta,
        b"redacted"
    );
}

#[test]
fn clean_batch_failure_keeps_entire_group_and_stops_other_batches() {
    let task_manager = Arc::new(TestTaskManager::default());
    let a = Arc::new(BatchCleanerRecorder {
        fail: true,
        ..Default::default()
    });
    let b = Arc::new(BatchCleanerRecorder {
        fail: true,
        ..Default::default()
    });
    for (kind, recorder) in [
        ("batch-failure-a", a.clone()),
        ("batch-failure-b", b.clone()),
    ] {
        RegisterCleanerFactory(kind, Arc::new(move || recorder.clone()));
    }
    RegisterCleanerFactory(
        "batch-failure-single",
        Arc::new(|| Arc::new(FailingSingleCleaner)),
    );
    for (id, kind) in [
        (1, "batch-failure-a"),
        (2, "batch-failure-b"),
        (3, "batch-failure-single"),
        (4, "batch-failure-a"),
        (5, "batch-failure-b"),
        (6, "no-cleanup"),
    ] {
        task_manager.insert_task(manager_task(id, kind, TASK_STATE_REVERTED));
    }
    assert_eq!(
        Manager::new(task_manager.clone(), "server", None)
            .clean_finished_tasks()
            .unwrap(),
        2
    );
    assert_eq!(
        a.batches.lock().unwrap().len() + b.batches.lock().unwrap().len(),
        1
    );
    assert_eq!(task_manager.transferred_tasks.lock().unwrap()[0].base.id, 6);
    assert_eq!(task_manager.tasks.lock().unwrap().len(), 4);
    assert_eq!(
        task_manager
            .transferred_tasks
            .lock()
            .unwrap()
            .iter()
            .find(|task| task.base.id == 3)
            .unwrap()
            .meta,
        b"redacted"
    );
}

#[test]
fn clean_history_transfer_failure_propagates_and_is_retryable() {
    let task_manager = Arc::new(TestTaskManager::default());
    task_manager.insert_task(manager_task(1, "no-cleanup", TASK_STATE_REVERTED));
    *task_manager.transfer_error.lock().unwrap() = Some(SchedulerError::new("transfer failed"));
    let manager = Manager::new(task_manager.clone(), "server", None);
    assert_eq!(
        manager.clean_finished_tasks().unwrap_err().to_string(),
        "transfer failed"
    );
    assert!(task_manager.transferred_tasks.lock().unwrap().is_empty());
    *task_manager.transfer_error.lock().unwrap() = None;
    assert_eq!(manager.clean_finished_tasks().unwrap(), 1);
}
