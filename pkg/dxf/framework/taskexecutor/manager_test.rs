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

// `Manager` 单元测试。
//
// 用内存 `FakeTaskTable` 与可阻塞的 `ChannelExecutor` 覆盖执行器登记、
// 启动失败、完整生命周期、slot 抢占以及 InitMeta 重试行为。

// Ported from pkg/dxf/framework/taskexecutor/manager_test.go, driving the
// real `Manager` against a small in-memory `TaskTable` fake instead of
// gomock. Cross-keyspace runtime acquisition is owned by the Rust `dxfutil`
// boundary and runtime validation by `task_executor`; this standalone Manager
// contract has no session/server handle from which it could acquire a runtime.
// The task-lifecycle, slot-allocation/preemption, executor-registry, retry and
// cancellation scenarios are exercised here against the real Manager.

// Fake 表与抢占场景用 HashMap 存任务/执行器。
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::{
    ClearTaskExecutors, Context, ExecutorError, NewManager, NodeResource, Param, RegisterTaskType,
    RegistryLockForTest, Result, Task, TaskBase, TaskExecInfo, TaskExecutor, TaskState, TaskTable,
};

#[derive(Default)]
/// 内存假任务表：队列化 exec_info / 结果，记录 Pause/Cancel/Fail 调用。
struct FakeTaskTable {
    /// `GetTaskExecInfoByExecID` 返回队列。
    exec_info: Mutex<Vec<Result<Vec<TaskExecInfo>>>>,
    /// 按 ID 存放的任务。
    tasks: Mutex<HashMap<i64, Task>>,
    /// InitMeta 调用次数。
    init_meta_calls: AtomicUsize,
    /// InitMeta 预设返回值队列。
    init_meta_results: Mutex<Vec<Result<()>>>,
    /// PauseSubtasks 收到的任务 ID。
    pause_calls: Mutex<Vec<i64>>,
    /// PauseSubtasks 返回值队列。
    pause_results: Mutex<Vec<Result<()>>>,
    /// CancelSubtask 收到的任务 ID。
    cancel_calls: Mutex<Vec<i64>>,
    /// FailSubtask 调用记录。
    fail_subtask_calls: Mutex<Vec<(i64, ExecutorError)>>,
}
impl FakeTaskTable {
    /// 写入/覆盖一个任务。
    fn set_task(&self, task: TaskBase) {
        self.tasks.lock().unwrap().insert(
            task.ID,
            Task {
                TaskBase: task,
                Meta: vec![],
            },
        );
    }
    /// 追加一次 GetTaskExecInfo 返回值。
    fn push_exec_info(&self, infos: Result<Vec<TaskExecInfo>>) {
        self.exec_info.lock().unwrap().push(infos);
    }
}
impl TaskTable for FakeTaskTable {
    fn GetTaskByID(&self, _: &Context, id: i64) -> Result<Task> {
        self.tasks
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| ExecutorError("task not found".into()))
    }
    fn GetTaskExecInfoByExecID(&self, _: &Context, _: &str) -> Result<Vec<TaskExecInfo>> {
        let mut queue = self.exec_info.lock().unwrap();
        if queue.is_empty() {
            Ok(vec![])
        } else {
            queue.remove(0)
        }
    }
    fn InitMeta(&self, _: &Context, _: &str, _: &str) -> Result<()> {
        self.init_meta_calls.fetch_add(1, Ordering::SeqCst);
        let mut results = self.init_meta_results.lock().unwrap();
        if results.is_empty() {
            Ok(())
        } else {
            results.remove(0)
        }
    }
    fn PauseSubtasks(&self, _: &Context, _: &str, id: i64) -> Result<()> {
        self.pause_calls.lock().unwrap().push(id);
        let mut results = self.pause_results.lock().unwrap();
        if results.is_empty() {
            Ok(())
        } else {
            results.remove(0)
        }
    }
    fn CancelSubtask(&self, _: &Context, _: &str, id: i64) -> Result<()> {
        self.cancel_calls.lock().unwrap().push(id);
        Ok(())
    }
    fn FailSubtask(&self, _: &Context, _: &str, id: i64, error: &ExecutorError) -> Result<()> {
        self.fail_subtask_calls
            .lock()
            .unwrap()
            .push((id, error.clone()));
        Ok(())
    }
}

/// A `TaskExecutor` whose `Run` blocks on a channel until the test signals
/// it to stop, mirroring Go's `DoAndReturn(func() { <-ch })` pattern.
/// Run 阻塞在 channel 上的测试执行器，便于控制生命周期。
struct ChannelExecutor {
    /// 关联任务基础信息。
    base: TaskBase,
    /// Run 阻塞接收端；take 后只阻塞一次。
    stop: Mutex<Option<mpsc::Receiver<Result<()>>>>,
    /// Init 预设结果。
    init_result: Result<()>,
    /// Init 调用计数。
    init_calls: AtomicUsize,
    /// Run 调用计数。
    run_calls: AtomicUsize,
    /// Close 调用计数。
    close_calls: AtomicUsize,
    /// Cancel 调用计数。
    cancel_calls: AtomicUsize,
    /// IsRetryableError 返回值。
    retryable: bool,
}
impl ChannelExecutor {
    /// 构造成功 Init 的阻塞执行器。
    fn new(base: TaskBase, stop: mpsc::Receiver<Result<()>>) -> Arc<Self> {
        Arc::new(Self {
            base,
            stop: Mutex::new(Some(stop)),
            init_result: Ok(()),
            init_calls: AtomicUsize::new(0),
            run_calls: AtomicUsize::new(0),
            close_calls: AtomicUsize::new(0),
            cancel_calls: AtomicUsize::new(0),
            retryable: false,
        })
    }
    /// 构造 Init 失败的执行器。
    fn with_init_error(base: TaskBase, error: ExecutorError, retryable: bool) -> Arc<Self> {
        let (_tx, rx) = mpsc::channel();
        Arc::new(Self {
            base,
            stop: Mutex::new(Some(rx)),
            init_result: Err(error),
            init_calls: AtomicUsize::new(0),
            run_calls: AtomicUsize::new(0),
            close_calls: AtomicUsize::new(0),
            cancel_calls: AtomicUsize::new(0),
            retryable,
        })
    }
}
impl TaskExecutor for ChannelExecutor {
    fn Init(&self, _: &Context) -> Result<()> {
        self.init_calls.fetch_add(1, Ordering::SeqCst);
        self.init_result.clone()
    }
    fn Run(&self) {
        self.run_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(rx) = self.stop.lock().unwrap().take() {
            let _ = rx.recv();
        }
    }
    fn GetTaskBase(&self) -> TaskBase {
        self.base.clone()
    }
    fn CancelRunningSubtask(&self) {}
    fn Cancel(&self) {
        self.cancel_calls.fetch_add(1, Ordering::SeqCst);
    }
    fn Close(&self) {
        self.close_calls.fetch_add(1, Ordering::SeqCst);
    }
    fn IsRetryableError(&self, _: &ExecutorError) -> bool {
        self.retryable
    }
}

/// 构造常用测试任务。
fn base(id: i64, state: TaskState, slots: i32) -> TaskBase {
    TaskBase {
        ID: id,
        State: state,
        Step: 1,
        Type: "type".into(),
        RequiredSlots: slots,
        ..Default::default()
    }
}

#[test]
/// 验证 add/del/cancel 与 Pausing/Reverting 处理。
fn test_manage_task_executor_registry() {
    let table = Arc::new(FakeTaskTable::default());
    let m = NewManager(
        Context::Background(),
        "test",
        table.clone(),
        NodeResource::default(),
    )
    .unwrap();

    let executor1: Arc<dyn TaskExecutor> =
        ChannelExecutor::new(base(1, TaskState::Running, 0), mpsc::channel().1);
    m.addTaskExecutor(executor1.clone());
    assert!(m.isExecutorStarted(1));

    let executor2: Arc<dyn TaskExecutor> =
        ChannelExecutor::new(base(2, TaskState::Running, 0), mpsc::channel().1);
    m.addTaskExecutor(executor2.clone());
    assert!(m.isExecutorStarted(2));

    m.delTaskExecutor(&*executor1);
    assert!(!m.isExecutorStarted(1));

    m.cancelTaskExecutors(&[base(2, TaskState::Running, 0)]);
    m.cancelRunningSubtaskOf(2);

    // handlePausingTask: success then failure.
    m.addTaskExecutor(executor1.clone());
    assert!(m.handlePausingTask(1).is_ok());
    assert_eq!(vec![1], *table.pause_calls.lock().unwrap());
    table
        .pause_results
        .lock()
        .unwrap()
        .push(Err(ExecutorError("pause failed".into())));
    assert!(
        m.handlePausingTask(1)
            .unwrap_err()
            .0
            .contains("pause failed")
    );

    // handleRevertingTask cancels the running subtask and cancels via table.
    assert!(m.handleRevertingTask(1).is_ok());
    assert_eq!(vec![1], *table.cancel_calls.lock().unwrap());
}

#[test]
/// 未注册类型应 FailSubtask 并释放 slot。
fn test_handle_executable_tasks_type_not_found_fails_subtask() {
    let _guard = RegistryLockForTest();
    ClearTaskExecutors();
    let table = Arc::new(FakeTaskTable::default());
    let task = base(1, TaskState::Running, 6);
    table.set_task(task.clone());
    let m = NewManager(
        Context::Background(),
        "test",
        table.clone(),
        NodeResource {
            TotalCPU: 16,
            ..Default::default()
        },
    )
    .unwrap();

    assert!(!m.startTaskExecutor(&task));
    assert_eq!(1, table.fail_subtask_calls.lock().unwrap().len());
    assert_eq!(16, m.slotManager.availableSlots());
    ClearTaskExecutors();
}

#[test]
/// Init 失败：不可重试记 Fail；可重试不记 Fail，均释放 slot。
fn test_handle_executable_tasks_init_failed_retryable_and_non_retryable() {
    let _guard = RegistryLockForTest();
    ClearTaskExecutors();
    let table = Arc::new(FakeTaskTable::default());
    let task = base(1, TaskState::Running, 6);
    table.set_task(task.clone());
    let m = NewManager(
        Context::Background(),
        "test",
        table.clone(),
        NodeResource {
            TotalCPU: 16,
            ..Default::default()
        },
    )
    .unwrap();

    // Non-retryable init failure: slot is freed and FailSubtask is called.
    RegisterTaskType(
        "type".to_string(),
        Arc::new(move |_: Context, task: Task, _: Param| {
            ChannelExecutor::with_init_error(
                task.TaskBase,
                ExecutorError("executor init failed".into()),
                false,
            ) as Arc<dyn TaskExecutor>
        }),
    );
    assert!(!m.startTaskExecutor(&task));
    assert_eq!(1, table.fail_subtask_calls.lock().unwrap().len());
    assert_eq!(16, m.slotManager.availableSlots());
    assert!(!m.isExecutorStarted(1));

    // Retryable init failure: slot is still freed, but FailSubtask is not
    // called.
    RegisterTaskType(
        "type".to_string(),
        Arc::new(move |_: Context, task: Task, _: Param| {
            ChannelExecutor::with_init_error(
                task.TaskBase,
                ExecutorError("executor init failed".into()),
                true,
            ) as Arc<dyn TaskExecutor>
        }),
    );
    assert!(!m.startTaskExecutor(&task));
    assert_eq!(1, table.fail_subtask_calls.lock().unwrap().len());
    assert_eq!(16, m.slotManager.availableSlots());
    ClearTaskExecutors();
}

#[test]
/// 完整生命周期：分配 → Run → 退出 → Close 并归还 slot。
fn test_handle_executable_tasks_start_run_close_lifecycle() {
    let _guard = RegistryLockForTest();
    ClearTaskExecutors();
    let table = Arc::new(FakeTaskTable::default());
    let task = base(1, TaskState::Running, 6);
    table.set_task(task.clone());
    let m = NewManager(
        Context::Background(),
        "test",
        table.clone(),
        NodeResource {
            TotalCPU: 16,
            ..Default::default()
        },
    )
    .unwrap();

    let (tx, rx) = mpsc::channel();
    let executor = ChannelExecutor::new(task.clone(), rx);
    let captured = executor.clone();
    RegisterTaskType(
        "type".to_string(),
        Arc::new(move |_: Context, _: Task, _: Param| captured.clone() as Arc<dyn TaskExecutor>),
    );

    m.handleExecutableTasks(&[TaskExecInfo {
        TaskBase: task.clone(),
    }]);
    wait_until(|| m.isExecutorStarted(1));
    assert_eq!(10, m.slotManager.availableSlots());
    assert_eq!(1, executor.init_calls.load(Ordering::SeqCst));

    tx.send(Ok(())).unwrap();
    wait_until(|| !m.isExecutorStarted(1));
    assert_eq!(16, m.slotManager.availableSlots());
    assert_eq!(1, executor.close_calls.load(Ordering::SeqCst));
    ClearTaskExecutors();
}

#[test]
/// 高优任务暂时无法分配且无需抢占时，Manager 仍应继续尝试后续任务。
fn test_handle_executable_tasks_continues_after_unallocatable_task() {
    let _guard = RegistryLockForTest();
    ClearTaskExecutors();
    let table = Arc::new(FakeTaskTable::default());
    let first = base(1, TaskState::Running, 8);
    let second = base(2, TaskState::Running, 1);
    table.set_task(first.clone());
    table.set_task(second.clone());
    let manager = NewManager(
        Context::Background(),
        "test",
        table,
        NodeResource {
            TotalCPU: 4,
            ..Default::default()
        },
    )
    .unwrap();
    let (tx, rx) = mpsc::channel();
    let executor = ChannelExecutor::new(second.clone(), rx);
    let captured = executor.clone();
    RegisterTaskType(
        "type".to_string(),
        Arc::new(move |_: Context, _: Task, _: Param| captured.clone() as Arc<dyn TaskExecutor>),
    );

    manager.handleExecutableTasks(&[
        TaskExecInfo { TaskBase: first },
        TaskExecInfo { TaskBase: second },
    ]);
    wait_until(|| manager.isExecutorStarted(2));
    assert_eq!(1, executor.init_calls.load(Ordering::SeqCst));
    tx.send(Ok(())).unwrap();
    wait_until(|| !manager.isExecutorStarted(2));
    ClearTaskExecutors();
}

/// 短轮询等待条件成立，超时则断言失败。
fn wait_until(mut condition: impl FnMut() -> bool) {
    for _ in 0..100 {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(condition(), "condition did not become true within timeout");
}

#[test]
/// handleTasks：错误 noop、Pausing、启动、去重、Reverting、结束。
fn test_manager_handle_tasks_full_lifecycle() {
    let _guard = RegistryLockForTest();
    ClearTaskExecutors();
    let table = Arc::new(FakeTaskTable::default());
    let m = NewManager(
        Context::Background(),
        "test",
        table.clone(),
        NodeResource {
            TotalCPU: 16,
            ..Default::default()
        },
    )
    .unwrap();

    // failed to get tasks: no-op.
    table.push_exec_info(Err(ExecutorError("mock err".into())));
    m.handleTasks();
    assert!(!m.isExecutorStarted(1));

    // pausing task: handled directly, no executor registered.
    table.push_exec_info(Ok(vec![TaskExecInfo {
        TaskBase: base(1, TaskState::Pausing, 0),
    }]));
    m.handleTasks();
    assert_eq!(vec![1], *table.pause_calls.lock().unwrap());

    // running task: starts an executor.
    let task1 = base(1, TaskState::Running, 1);
    table.set_task(task1.clone());
    let (tx, rx) = mpsc::channel();
    let executor = ChannelExecutor::new(task1.clone(), rx);
    let captured = executor.clone();
    RegisterTaskType(
        "type".to_string(),
        Arc::new(move |_: Context, _: Task, _: Param| captured.clone() as Arc<dyn TaskExecutor>),
    );
    table.push_exec_info(Ok(vec![TaskExecInfo {
        TaskBase: task1.clone(),
    }]));
    m.handleTasks();
    wait_until(|| m.isExecutorStarted(1));

    // handling again while already started must not start a second one.
    table.push_exec_info(Ok(vec![TaskExecInfo {
        TaskBase: task1.clone(),
    }]));
    m.handleTasks();
    assert_eq!(1, executor.init_calls.load(Ordering::SeqCst));

    // task moves to reverting: cancels the running subtask and the task.
    let mut reverting = task1.clone();
    reverting.State = TaskState::Reverting;
    table.push_exec_info(Ok(vec![TaskExecInfo {
        TaskBase: reverting,
    }]));
    m.handleTasks();
    assert_eq!(vec![1], *table.cancel_calls.lock().unwrap());
    assert!(m.isExecutorStarted(1));

    // finish: executor exits and is removed.
    tx.send(Ok(())).unwrap();
    wait_until(|| !m.isExecutorStarted(1));
    assert_eq!(1, executor.close_calls.load(Ordering::SeqCst));
    ClearTaskExecutors();
}

#[test]
/// Manager 层抢占：高优 Cancel 低优，释放后再分配。
fn test_slot_manager_preemption_in_manager() {
    let _guard = RegistryLockForTest();
    ClearTaskExecutors();
    let table = Arc::new(FakeTaskTable::default());
    let m = NewManager(
        Context::Background(),
        "test",
        table.clone(),
        NodeResource {
            TotalCPU: 10,
            ..Default::default()
        },
    )
    .unwrap();

    let task1 = base(1, TaskState::Running, 10);
    let task2 = base(2, TaskState::Running, 1);
    let mut task3 = base(3, TaskState::Running, 1);
    task3.Priority = -1;
    for t in [&task1, &task2, &task3] {
        table.set_task(t.clone());
    }

    let executors: Arc<Mutex<HashMap<i64, (Arc<ChannelExecutor>, mpsc::Sender<Result<()>>)>>> =
        Arc::new(Mutex::new(HashMap::new()));
    {
        let executors = executors.clone();
        RegisterTaskType(
            "type".to_string(),
            Arc::new(move |_: Context, task: Task, _: Param| {
                let (tx, rx) = mpsc::channel();
                let executor = ChannelExecutor::new(task.TaskBase.clone(), rx);
                executors
                    .lock()
                    .unwrap()
                    .insert(task.TaskBase.ID, (executor.clone(), tx));
                executor as Arc<dyn TaskExecutor>
            }),
        );
    }

    // task1 alone consumes all 10 slots; task2 cannot be allocated.
    m.handleExecutableTasks(&[
        TaskExecInfo {
            TaskBase: task1.clone(),
        },
        TaskExecInfo {
            TaskBase: task2.clone(),
        },
    ]);
    wait_until(|| m.isExecutorStarted(1));
    assert_eq!(0, m.slotManager.availableSlots());
    assert!(!m.isExecutorStarted(2));

    // task3 (higher priority) preempts task1: task1.Cancel() is invoked, but
    // it keeps running until it exits on its own.
    m.handleExecutableTasks(&[
        TaskExecInfo {
            TaskBase: task3.clone(),
        },
        TaskExecInfo {
            TaskBase: task2.clone(),
        },
    ]);
    assert_eq!(
        1,
        executors.lock().unwrap()[&1]
            .0
            .cancel_calls
            .load(Ordering::SeqCst)
    );
    assert!(m.isExecutorStarted(1));

    // task1 exits (as if it observed the cancellation): task3 gets its slot.
    executors.lock().unwrap()[&1].1.send(Ok(())).unwrap();
    wait_until(|| !m.isExecutorStarted(1));
    assert_eq!(10, m.slotManager.availableSlots());

    m.handleExecutableTasks(&[
        TaskExecInfo {
            TaskBase: task3.clone(),
        },
        TaskExecInfo {
            TaskBase: task2.clone(),
        },
    ]);
    wait_until(|| m.isExecutorStarted(3) && m.isExecutorStarted(2));
    assert_eq!(8, m.slotManager.availableSlots());

    executors.lock().unwrap()[&2].1.send(Ok(())).unwrap();
    executors.lock().unwrap()[&3].1.send(Ok(())).unwrap();
    wait_until(|| !m.isExecutorStarted(2) && !m.isExecutorStarted(3));
    assert_eq!(10, m.slotManager.availableSlots());
    ClearTaskExecutors();
}

#[test]
/// InitMeta：首次成功；失败后重试直至成功。
fn test_manager_init_meta_retries_then_succeeds() {
    let table = Arc::new(FakeTaskTable::default());
    let m = NewManager(
        Context::Background(),
        "test",
        table.clone(),
        NodeResource::default(),
    )
    .unwrap();
    assert!(m.InitMeta().is_ok());
    assert_eq!(1, table.init_meta_calls.load(Ordering::SeqCst));

    table
        .init_meta_results
        .lock()
        .unwrap()
        .push(Err(ExecutorError("mock err".into())));
    assert!(m.InitMeta().is_ok());
    assert_eq!(3, table.init_meta_calls.load(Ordering::SeqCst));
}

#[test]
/// InitMeta：三次皆失败则返回错误。
fn test_manager_init_meta_exhausts_retries_and_returns_error() {
    let table = Arc::new(FakeTaskTable::default());
    let m = NewManager(
        Context::Background(),
        "test",
        table.clone(),
        NodeResource::default(),
    )
    .unwrap();
    for _ in 0..3 {
        table
            .init_meta_results
            .lock()
            .unwrap()
            .push(Err(ExecutorError("mock err".into())));
    }
    let err = m.InitMeta();
    assert_eq!(Err(ExecutorError("mock err".into())), err);
    assert_eq!(3, table.init_meta_calls.load(Ordering::SeqCst));
}

#[test]
/// 上下文已取消时失败一次即停止重试。
fn test_manager_init_meta_stops_retrying_once_context_cancelled() {
    let table = Arc::new(FakeTaskTable::default());
    let ctx = Context::Background();
    let m = NewManager(ctx.clone(), "test", table.clone(), NodeResource::default()).unwrap();
    ctx.Cancel();
    table
        .init_meta_results
        .lock()
        .unwrap()
        .push(Err(ExecutorError("mock err".into())));
    let err = m.InitMeta();
    assert_eq!(Err(ExecutorError("context canceled".into())), err);
    // The retry loop checks `ctx.Done()` right after the first failed attempt,
    // returns the cancellation error, and skips the remaining retries.
    assert_eq!(1, table.init_meta_calls.load(Ordering::SeqCst));
}

#[test]
/// 与 Go 的 ticker/select 一致：取消后 Stop 不应等待 90 秒恢复周期结束。
fn test_manager_stop_interrupts_background_interval_waits() {
    let table = Arc::new(FakeTaskTable::default());
    let manager = NewManager(
        Context::Background(),
        "test",
        table,
        NodeResource::default(),
    )
    .unwrap();
    manager.Start().unwrap();

    let (stopped_tx, stopped_rx) = mpsc::channel();
    std::thread::spawn(move || {
        manager.Stop();
        stopped_tx.send(()).unwrap();
    });

    stopped_rx
        .recv_timeout(Duration::from_millis(500))
        .expect("Stop must wake both manager loops immediately after cancellation");
}
