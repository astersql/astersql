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

// BaseTaskExecutor 单元测试：用脚本化内存假对象驱动 Run / 参数变更 / 负载均衡。
//
// `Run` 驱动真实的 balance/summary/param 监控线程，并在 step 变化时重建执行器；
// 直接调用对应方法的测试同时覆盖其可独立验证的边界分支。
// 子任务（subtask）是任务在某一步骤上的可调度执行单元。

// Ported from pkg/dxf/framework/taskexecutor/task_executor_test.go, driving
// the real `BaseTaskExecutor` against a small in-memory `TaskTable`/\
// `Extension`/`StepExecutor` fake instead of gomock. The production Run loop
// starts the three subtask monitors and recreates the step executor when the
// task advances to a new step; direct method tests cover their edge branches.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use crate::{
    BaseStepExecutor, Context, ErrCancelSubtask, ErrNonIdempotentSubtask, ExecutorError, Extension,
    NewBaseTaskExecutor, NewParamForTest, Result, SetSubtaskCheckIntervalForTest, StepExecutor,
    StepResource, Subtask, SubtaskBase, SubtaskState, Task, TaskBase, TaskState, TaskTable,
    newSlotManager,
};

/// 按调用顺序弹出脚本化响应的假 TaskTable（超出脚本则 panic，模拟 gomock）。
/// Drives `BaseTaskExecutor::Run` against scripted, per-call responses,
/// mirroring the strict, ordered `gomock.Any()` expectations in Go's test
/// (a call beyond the scripted queue panics, just like an unmet gomock
/// expectation would fail the test).
#[derive(Default)]
struct FakeTaskTable {
    get_task: Mutex<VecDeque<Result<Task>>>,
    get_first_subtask: Mutex<VecDeque<Result<Option<Subtask>>>>,
    start_subtask: Mutex<VecDeque<Result<()>>>,
    finish_subtask_calls: Mutex<Vec<(i64, Vec<u8>)>>,
    update_state_calls: Mutex<Vec<(i64, SubtaskState, Option<ExecutorError>)>>,
    fail_subtask_calls: Mutex<Vec<(i64, ExecutorError)>>,
    get_subtasks_running: Mutex<VecDeque<Result<Vec<Subtask>>>>,
    running_back_to_pending_calls: Mutex<Vec<Vec<i64>>>,
}

impl FakeTaskTable {
    /// 入队一次成功的 GetTaskByID 响应。
    fn push_task(&self, task: Task) {
        self.get_task.lock().unwrap().push_back(Ok(task));
    }
    /// 入队一次失败的 GetTaskByID 响应。
    fn push_task_err(&self, err: ExecutorError) {
        self.get_task.lock().unwrap().push_back(Err(err));
    }
    /// 入队一次 GetFirstSubtaskInStates 成功响应。
    fn push_subtask(&self, subtask: Option<Subtask>) {
        self.get_first_subtask
            .lock()
            .unwrap()
            .push_back(Ok(subtask));
    }
    /// 入队一次 GetFirstSubtaskInStates 错误。
    fn push_subtask_err(&self, err: ExecutorError) {
        self.get_first_subtask.lock().unwrap().push_back(Err(err));
    }
}

/// 从各队列弹出脚本；缺省行为对 Start/GetRunning 返回 Ok 空。
impl TaskTable for FakeTaskTable {
    fn GetTaskByID(&self, _: &Context, _: i64) -> Result<Task> {
        self.get_task
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected GetTaskByID call")
    }
    fn GetFirstSubtaskInStates(
        &self,
        _: &Context,
        _: &str,
        _: i64,
        _: crate::Step,
        _: &[SubtaskState],
    ) -> Result<Option<Subtask>> {
        self.get_first_subtask
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected GetFirstSubtaskInStates call")
    }
    fn StartSubtask(&self, _: &Context, _: i64, _: &str) -> Result<()> {
        self.start_subtask
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(()))
    }
    fn FinishSubtask(&self, _: &Context, _: &str, id: i64, meta: &[u8]) -> Result<()> {
        self.finish_subtask_calls
            .lock()
            .unwrap()
            .push((id, meta.to_vec()));
        Ok(())
    }
    fn UpdateSubtaskStateAndError(
        &self,
        _: &Context,
        _: &str,
        id: i64,
        state: SubtaskState,
        error: Option<&ExecutorError>,
    ) -> Result<()> {
        self.update_state_calls
            .lock()
            .unwrap()
            .push((id, state, error.cloned()));
        Ok(())
    }
    fn FailSubtask(&self, _: &Context, _: &str, id: i64, error: &ExecutorError) -> Result<()> {
        self.fail_subtask_calls
            .lock()
            .unwrap()
            .push((id, error.clone()));
        Ok(())
    }
    fn GetSubtasksByExecIDAndStepAndStates(
        &self,
        _: &Context,
        _: &str,
        _: i64,
        _: crate::Step,
        _: &[SubtaskState],
    ) -> Result<Vec<Subtask>> {
        self.get_subtasks_running
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(vec![]))
    }
    fn RunningSubtasksBack2Pending(&self, _: &Context, bases: &[crate::SubtaskBase]) -> Result<()> {
        self.running_back_to_pending_calls
            .lock()
            .unwrap()
            .push(bases.iter().map(|b| b.ID).collect());
        Ok(())
    }
}

/// 脚本化 Extension：步骤执行器、幂等与可重试判定。
/// Scripts `Extension::GetStepExecutor`/`IsIdempotent`/`IsRetryableError`.
#[derive(Default)]
struct FakeExtension {
    step_executor: Mutex<Option<Result<Arc<dyn StepExecutor>>>>,
    idempotent: Mutex<VecDeque<bool>>,
    retryable: Mutex<VecDeque<bool>>,
}
impl Extension for FakeExtension {
    fn IsIdempotent(&self, _: &Subtask) -> bool {
        self.idempotent.lock().unwrap().pop_front().unwrap_or(true)
    }
    fn GetStepExecutor(&self, _: &Task) -> Result<Arc<dyn StepExecutor>> {
        self.step_executor
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| Ok(Arc::new(BaseStepExecutor)))
    }
    fn IsRetryableError(&self, _: &ExecutorError) -> bool {
        self.retryable.lock().unwrap().pop_front().unwrap_or(false)
    }
}

/// 脚本化 StepExecutor，并统计 Init/Cleanup 等调用次数。
/// Scripts `StepExecutor` callbacks and records how many times each was
/// invoked.
#[derive(Default)]
struct FakeStepExecutor {
    init_result: Mutex<VecDeque<Result<()>>>,
    run_subtask_result:
        Mutex<VecDeque<Box<dyn FnMut(&Context, &mut Subtask) -> Result<()> + Send>>>,
    cleanup_result: Mutex<VecDeque<Result<()>>>,
    init_calls: AtomicUsize,
    cleanup_calls: AtomicUsize,
    resource_modified_calls: Mutex<Vec<StepResource>>,
    resource_modified_result: Mutex<VecDeque<Result<()>>>,
    task_meta_modified_calls: Mutex<Vec<Vec<u8>>>,
    task_meta_modified_result: Mutex<VecDeque<Result<()>>>,
}
impl StepExecutor for FakeStepExecutor {
    fn Init(&self, _: &Context) -> Result<()> {
        self.init_calls.fetch_add(1, Ordering::SeqCst);
        self.init_result
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(()))
    }
    fn RunSubtask(&self, ctx: &Context, subtask: &mut Subtask) -> Result<()> {
        if let Some(mut action) = self.run_subtask_result.lock().unwrap().pop_front() {
            action(ctx, subtask)
        } else {
            Ok(())
        }
    }
    fn Cleanup(&self, _: &Context) -> Result<()> {
        self.cleanup_calls.fetch_add(1, Ordering::SeqCst);
        self.cleanup_result
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(()))
    }
    fn ResourceModified(&self, _: &Context, resource: &StepResource) -> Result<()> {
        self.resource_modified_calls
            .lock()
            .unwrap()
            .push(resource.clone());
        self.resource_modified_result
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(()))
    }
    fn TaskMetaModified(&self, _: &Context, meta: &[u8]) -> Result<()> {
        self.task_meta_modified_calls
            .lock()
            .unwrap()
            .push(meta.to_vec());
        self.task_meta_modified_result
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(()))
    }
}

/// 构造 ID=1、Running、Step=1、RequiredSlots=10 的示例任务。
fn task1() -> Task {
    Task {
        TaskBase: TaskBase {
            ID: 1,
            Type: "example".into(),
            State: TaskState::Running,
            Step: 1,
            RequiredSlots: 10,
            ..Default::default()
        },
        Meta: vec![],
    }
}
/// 克隆任务并置为 Succeed（用于让 Run 循环退出）。
fn succeed(task: &Task) -> Task {
    let mut t = task.clone();
    t.TaskBase.State = TaskState::Succeed;
    t
}
/// ID=1 的 Pending 子任务。
fn pending_subtask1() -> Subtask {
    Subtask {
        SubtaskBase: SubtaskBase {
            ID: 1,
            Step: 1,
            State: SubtaskState::Pending,
            ExecID: "id".into(),
            ..Default::default()
        },
        Meta: vec![],
    }
}
/// ID=2 的 Running 子任务（模拟上次遗留）。
fn running_subtask2() -> Subtask {
    Subtask {
        SubtaskBase: SubtaskBase {
            ID: 2,
            Step: 1,
            State: SubtaskState::Running,
            ExecID: "id".into(),
            ..Default::default()
        },
        Meta: vec![],
    }
}

/// 单测夹具：假表/扩展/步骤执行器 + BaseTaskExecutor，并锁住全局间隔。
struct Env {
    table: Arc<FakeTaskTable>,
    ext: Arc<FakeExtension>,
    step: Arc<FakeStepExecutor>,
    executor: Arc<crate::BaseTaskExecutor>,
    _guard: (Duration, Duration),
    // `SubtaskCheckInterval`/`MaxSubtaskCheckInterval` are process-wide
    // atomics; hold the shared registry/interval lock for this whole `Env`'s
    // lifetime so no other test in this crate's (parallel) test binary can
    // restore them to their slow defaults while this one is mid-run.
    _interval_lock: std::sync::MutexGuard<'static, ()>,
}
/// 缩短子任务检查间隔并构造默认 Env。
fn new_env() -> Env {
    // 压低轮询退避，避免无子任务空转拖慢套件。
    // Shrink the poll backoff so a run of several no-subtask iterations
    // (Go's `ReduceCheckInterval`) does not slow the test suite down.
    let interval_lock = crate::RegistryLockForTest();
    let guard = SetSubtaskCheckIntervalForTest(Duration::from_millis(1), Duration::from_millis(1));
    let table = Arc::new(FakeTaskTable::default());
    let ext = Arc::new(FakeExtension::default());
    let step = Arc::new(FakeStepExecutor::default());
    *ext.step_executor.lock().unwrap() = Some(Ok(step.clone() as Arc<dyn StepExecutor>));
    let param = NewParamForTest(
        table.clone(),
        Arc::new(newSlotManager(16)),
        crate::NodeResource::default(),
        "id",
        ext.clone() as Arc<dyn Extension>,
    );
    let executor = NewBaseTaskExecutor(Context::Background(), task1(), param);
    Env {
        table,
        ext,
        step,
        executor,
        _guard: guard,
        _interval_lock: interval_lock,
    }
}
/// 恢复进程级子任务检查间隔。
impl Drop for Env {
    fn drop(&mut self) {
        SetSubtaskCheckIntervalForTest(self._guard.0, self._guard.1);
    }
}

#[test]
/// Cancel 后 Run 应立即退出且不 Init 步骤执行器。
fn test_context_done_when_run_exits_immediately() {
    let e = new_env();
    e.executor.Cancel();
    e.executor.Run();
    assert_eq!(0, e.step.init_calls.load(Ordering::SeqCst));
}

#[test]
/// GetTaskByID 报 not found 时 Run 退出。
fn test_task_not_found_when_run_exits() {
    let e = new_env();
    e.table
        .push_task_err(ExecutorError("task not found".into()));
    e.executor.Run();
}

#[test]
/// 任务非 Running（Succeed/Reverting）时 Run 退出。
fn test_task_state_not_running_when_run_exits() {
    let e = new_env();
    let mut reverting = task1();
    reverting.TaskBase.State = TaskState::Reverting;
    e.table.push_task(succeed(&task1()));
    e.executor.Run();
    e.table.push_task(reverting);
    e.executor.Run();
}

#[test]
/// Modifying 与 Running 一样允许执行当前步骤。
fn test_task_modifying_state_runs_subtasks() {
    let e = new_env();
    let mut modifying = task1();
    modifying.TaskBase.State = TaskState::Modifying;
    e.table.push_task(modifying);
    e.table.push_subtask(Some(pending_subtask1()));
    e.table.push_task(succeed(&task1()));
    e.executor.Run();
    assert_eq!(1, e.table.finish_subtask_calls.lock().unwrap().len());
}

#[test]
/// 普通 GetTaskByID 刷新错误重试，任务不存在错误退出。
fn test_get_task_by_id_error_exits_run_immediately() {
    // Transient refresh errors are retried by the Run loop; the later
    // terminal task response is then observed.
    let e = new_env();
    e.table.push_task_err(ExecutorError("some err".into()));
    e.table.push_task_err(ExecutorError("some err".into()));
    e.table.push_task(succeed(&task1()));
    e.executor.Run();
    assert_eq!(0, e.table.get_task.lock().unwrap().len());
}

#[test]
/// GetFirstSubtaskInStates 出错时可重试直至任务 Succeed。
fn test_retry_on_error_of_get_first_subtask_in_states() {
    let e = new_env();
    for _ in 0..3 {
        e.table.push_task(task1());
        e.table.push_subtask_err(ExecutorError("some err".into()));
    }
    e.table.push_task(succeed(&task1()));
    e.executor.Run();
}

#[test]
/// 构造步骤执行器失败会 FailSubtask。
fn test_get_step_executor_failed_fails_subtask() {
    let e = new_env();
    e.table.push_task(task1());
    e.table.push_subtask(Some(pending_subtask1()));
    *e.ext.step_executor.lock().unwrap() = Some(Err(ExecutorError("constructor not found".into())));
    e.table.push_task(succeed(&task1()));
    e.executor.Run();
    let calls = e.table.fail_subtask_calls.lock().unwrap();
    assert_eq!(1, calls.len());
    assert_eq!(1, calls[0].0);
    assert_eq!("constructor not found", calls[0].1.0);
}

#[test]
/// Init 不可重试错误 → FailSubtask。
fn test_non_retryable_step_executor_init_error_fails_subtask() {
    let e = new_env();
    e.table.push_task(task1());
    e.table.push_subtask(Some(pending_subtask1()));
    e.step
        .init_result
        .lock()
        .unwrap()
        .push_back(Err(ExecutorError("init error".into())));
    e.ext.retryable.lock().unwrap().push_back(false);
    e.table.push_task(succeed(&task1()));
    e.executor.Run();
    let calls = e.table.fail_subtask_calls.lock().unwrap();
    assert_eq!(1, calls.len());
    assert_eq!("init error", calls[0].1.0);
}

#[test]
/// Init 可重试错误不 FailSubtask。
fn test_retryable_step_executor_init_error_does_not_fail_subtask() {
    let e = new_env();
    e.table.push_task(task1());
    e.table.push_subtask(Some(pending_subtask1()));
    e.step
        .init_result
        .lock()
        .unwrap()
        .push_back(Err(ExecutorError("init error".into())));
    e.ext.retryable.lock().unwrap().push_back(true);
    e.table.push_task(succeed(&task1()));
    e.executor.Run();
    assert!(e.table.fail_subtask_calls.lock().unwrap().is_empty());
}

#[test]
/// 成功跑完一个 Pending 子任务并 FinishSubtask。
fn test_run_one_subtask_success() {
    let e = new_env();
    e.table.push_task(task1());
    e.table.push_subtask(Some(pending_subtask1()));
    e.table.push_task(succeed(&task1()));
    e.executor.Run();
    let finished = e.table.finish_subtask_calls.lock().unwrap();
    assert_eq!(vec![(1, vec![])], *finished);
}

#[test]
fn run_subtask_result_meta_reaches_finish_subtask() {
    let e = new_env();
    e.table.push_task(task1());
    e.table.push_subtask(Some(pending_subtask1()));
    e.step
        .run_subtask_result
        .lock()
        .unwrap()
        .push_back(Box::new(|_, subtask| {
            subtask.Meta = br#"{"result":"sorted"}"#.to_vec();
            Ok(())
        }));
    e.table.push_task(succeed(&task1()));
    e.executor.Run();
    assert_eq!(
        vec![(1, br#"{"result":"sorted"}"#.to_vec())],
        *e.table.finish_subtask_calls.lock().unwrap(),
    );
}

#[test]
/// 不可重试的 RunSubtask 错误将状态更新为 Failed。
fn test_run_one_subtask_failed_non_retryable() {
    let e = new_env();
    e.table.push_task(task1());
    e.table.push_subtask(Some(pending_subtask1()));
    e.step
        .run_subtask_result
        .lock()
        .unwrap()
        .push_back(Box::new(|_, _| {
            Err(ExecutorError("run subtask error".into()))
        }));
    e.ext.retryable.lock().unwrap().push_back(false);
    e.table.push_task(succeed(&task1()));
    e.executor.Run();
    let updates = e.table.update_state_calls.lock().unwrap();
    assert_eq!(1, updates.len());
    assert_eq!((1, SubtaskState::Failed), (updates[0].0, updates[0].1));
}

#[test]
/// RunSubtask panic 不得逸出 Run，应失败一个子任务并清理步骤执行器。
fn test_run_subtask_panic_fails_subtask_and_cleans_up() {
    let e = new_env();
    e.table.push_task(task1());
    e.table.push_subtask(Some(pending_subtask1()));
    e.step
        .run_subtask_result
        .lock()
        .unwrap()
        .push_back(Box::new(|_, _| panic!("run subtask panic")));

    e.executor.Run();

    let failures = e.table.fail_subtask_calls.lock().unwrap();
    assert_eq!(1, failures.len());
    assert_eq!(1, failures[0].0);
    assert!(failures[0].1.0.contains("run subtask panic"));
    assert_eq!(1, e.step.cleanup_calls.load(Ordering::SeqCst));
}

#[test]
/// 可重试失败后，幂等子任务再次执行直至成功。
fn test_run_one_subtask_failed_retryable_succeeds_after_retry() {
    let e = new_env();
    e.table.push_task(task1());
    e.table.push_subtask(Some(pending_subtask1()));
    // 前两次失败可重试；随后同一 Running 子任务因幂等再跑直至成功。
    // First attempt fails with a retryable error; the Run loop polls again,
    // finds the same subtask still Running, and (because it's idempotent)
    // runs it again -- twice more failing, then succeeding.
    for _ in 0..2 {
        e.step
            .run_subtask_result
            .lock()
            .unwrap()
            .push_back(Box::new(|_, _| {
                Err(ExecutorError("run subtask error".into()))
            }));
    }
    e.ext.retryable.lock().unwrap().extend([true, true, true]);
    e.ext.idempotent.lock().unwrap().extend([true, true]);
    let mut running = pending_subtask1();
    running.SubtaskBase.State = SubtaskState::Running;
    e.table.push_task(task1());
    e.table.push_subtask(Some(running.clone()));
    e.table.push_task(task1());
    e.table.push_subtask(Some(running));
    e.table.push_task(succeed(&task1()));
    e.executor.Run();
    assert_eq!(1, e.table.finish_subtask_calls.lock().unwrap().len());
    assert!(e.table.fail_subtask_calls.lock().unwrap().is_empty());
}

#[test]
/// 连续跑 5 个子任务后因多次无子任务退出；Run 退出时 Cleanup。
fn test_run_subtasks_one_by_one_then_exit_due_to_no_subtask() {
    let e = new_env();
    for i in 1..=5_i64 {
        e.table.push_task(task1());
        e.table.push_subtask(Some(Subtask {
            SubtaskBase: SubtaskBase {
                ID: i,
                Step: 1,
                State: SubtaskState::Pending,
                ExecID: "id".into(),
                ..Default::default()
            },
            Meta: vec![],
        }));
    }
    for _ in 0..8 {
        e.table.push_task(task1());
        e.table.push_subtask(None);
    }
    e.executor.Run();
    assert_eq!(5, e.table.finish_subtask_calls.lock().unwrap().len());
    assert_eq!(1, e.step.init_calls.load(Ordering::SeqCst));
    // Go defers cleanStepExecutor from Run; a later Close must be idempotent.
    assert_eq!(1, e.step.cleanup_calls.load(Ordering::SeqCst));
    e.executor.Close();
    assert_eq!(1, e.step.cleanup_calls.load(Ordering::SeqCst));
}

#[test]
/// 遗留 Running 且非幂等 → 标 Failed（ErrNonIdempotentSubtask）。
fn test_previous_left_non_idempotent_subtask_running_is_failed() {
    let e = new_env();
    e.table.push_task(task1());
    e.table.push_subtask(Some(running_subtask2()));
    e.ext.idempotent.lock().unwrap().push_back(false);
    e.table.push_task(succeed(&task1()));
    e.executor.Run();
    let updates = e.table.update_state_calls.lock().unwrap();
    assert_eq!(1, updates.len());
    assert_eq!(2, updates[0].0);
    assert_eq!(SubtaskState::Failed, updates[0].1);
    assert_eq!(Some(ErrNonIdempotentSubtask()), updates[0].2);
}

#[test]
/// 遗留 Running 且幂等 → 再跑并 Finish。
fn test_previous_left_idempotent_subtask_running_is_run_again() {
    let e = new_env();
    e.table.push_task(task1());
    e.table.push_subtask(Some(running_subtask2()));
    e.ext.idempotent.lock().unwrap().push_back(true);
    e.table.push_task(succeed(&task1()));
    e.executor.Run();
    assert_eq!(1, e.table.finish_subtask_calls.lock().unwrap().len());
    assert!(e.table.update_state_calls.lock().unwrap().is_empty());
}

#[test]
/// 运行中 CancelRunningSubtask → 状态 Canceled。
fn test_subtask_cancelled_during_running() {
    let e = new_env();
    let (logger, logs) = astersql_lightning_log::testlogger::MakeTestLogger([]);
    *e.executor.sampleLogger.write().unwrap() = logger;
    e.table.push_task(task1());
    e.table.push_subtask(Some(pending_subtask1()));
    let executor = e.executor.clone();
    e.step
        .run_subtask_result
        .lock()
        .unwrap()
        .push_back(Box::new(move |_, _| {
            executor.CancelRunningSubtask();
            Err(ErrCancelSubtask())
        }));
    e.table
        .push_task_err(ExecutorError("task not found".into()));
    e.executor.Run();
    let lines = logs.lines();
    assert_eq!(1, lines.len(), "{lines:?}");
    assert!(lines[0].contains("subtask run canceled"));
    assert!(lines[0].contains("INFO"));
    assert!(!lines[0].contains("run subtask failed"));
    let updates = e.table.update_state_calls.lock().unwrap();
    assert_eq!(1, updates.len());
    assert_eq!(SubtaskState::Canceled, updates[0].1);
    assert!(updates[0].2.is_none());
}

#[test]
/// CancelRunningSubtask 必须真正取消传给 StepExecutor 的子上下文。
fn test_cancel_running_subtask_propagates_to_step_context() {
    let e = new_env();
    e.table.push_task(task1());
    e.table.push_subtask(Some(pending_subtask1()));
    e.table
        .push_task_err(ExecutorError("task not found".into()));
    let (started_tx, started_rx) = mpsc::channel();
    let (cancelled_tx, cancelled_rx) = mpsc::channel();
    e.step
        .run_subtask_result
        .lock()
        .unwrap()
        .push_back(Box::new(move |ctx, _| {
            started_tx.send(()).unwrap();
            while !ctx.Done() {
                std::thread::yield_now();
            }
            cancelled_tx.send(()).unwrap();
            Err(ErrCancelSubtask())
        }));
    let executor = e.executor.clone();
    let thread = std::thread::spawn(move || executor.Run());
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("step executor did not start");
    e.executor.CancelRunningSubtask();
    cancelled_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("CancelRunningSubtask did not cancel the step context");
    thread.join().unwrap();
    let updates = e.table.update_state_calls.lock().unwrap();
    assert_eq!(1, updates.len());
    assert_eq!(SubtaskState::Canceled, updates[0].1);
}

#[test]
/// 仅 executor.Cancel（未 CancelRunningSubtask）走非重试 Failed 分支。
fn test_task_executor_cancelled_during_subtask_running() {
    let e = new_env();
    let (logger, logs) = astersql_lightning_log::testlogger::MakeTestLogger([]);
    *e.executor.sampleLogger.write().unwrap() = logger;
    e.table.push_task(task1());
    e.table.push_subtask(Some(pending_subtask1()));
    let executor = e.executor.clone();
    e.step
        .run_subtask_result
        .lock()
        .unwrap()
        .push_back(Box::new(move |_, _| {
            executor.Cancel();
            Err(ExecutorError("context canceled".into()))
        }));
    e.executor.Run();
    let lines = logs.lines();
    assert_eq!(1, lines.len(), "{lines:?}");
    assert!(lines[0].contains("subtask run canceled"));
    assert!(lines[0].contains("INFO"));
    assert!(lines[0].contains("context canceled"));
    // Executor-level cancellation is graceful shutdown; Go leaves the
    // subtask state unchanged unless the explicit running-subtask cancel
    // cause was used.
    let updates = e.table.update_state_calls.lock().unwrap();
    assert!(updates.is_empty());
}

#[test]
/// startSubtask 三次均失败视为子任务已被调度走，不 Finish。
fn test_subtask_scheduled_away_right_before_start() {
    // startSubtask 经 retry 共 3 次；全部失败才算「已消失」。
    // `startSubtask` retries through `BaseTaskExecutor::retry` (3 attempts);
    // script all 3 to fail so the subtask really is treated as "gone".
    let e = new_env();
    e.table.push_task(task1());
    e.table.push_subtask(Some(pending_subtask1()));
    for _ in 0..3 {
        e.table
            .start_subtask
            .lock()
            .unwrap()
            .push_back(Err(ExecutorError("subtask not found".into())));
    }
    e.table.push_task(succeed(&task1()));
    e.executor.Run();
    assert!(e.table.finish_subtask_calls.lock().unwrap().is_empty());
}

#[test]
/// 连续无子任务若干次后退出，且未 Init。
fn test_no_subtask_to_run_exits_the_loop_after_some_time() {
    let e = new_env();
    for _ in 0..8 {
        e.table.push_task(task1());
        e.table.push_subtask(None);
    }
    e.executor.Run();
    assert_eq!(0, e.step.init_calls.load(Ordering::SeqCst));
}

#[test]
/// 中间成功跑过子任务后，无子任务计数重置，仍可再空转退出。
fn test_no_subtask_check_counter_resets_after_a_subtask_runs() {
    let e = new_env();
    for _ in 0..4 {
        e.table.push_task(task1());
        e.table.push_subtask(None);
    }
    e.table.push_task(task1());
    e.table.push_subtask(Some(pending_subtask1()));
    for _ in 0..8 {
        e.table.push_task(task1());
        e.table.push_subtask(None);
    }
    e.executor.Run();
    assert_eq!(1, e.table.finish_subtask_calls.lock().unwrap().len());
    assert_eq!(1, e.step.cleanup_calls.load(Ordering::SeqCst));
}

#[test]
/// Close 时 Cleanup 失败不得 panic。
fn test_step_executor_cleanup_failure_keeps_running() {
    // Cleanup 只在 Close 走一次，失败应被吞掉。
    // Cleanup only runs once, from `Close`, so a failure there must not be
    // surfaced as a panic or otherwise interrupt the caller.
    let e = new_env();
    e.step
        .cleanup_result
        .lock()
        .unwrap()
        .push_back(Err(ExecutorError("some error".into())));
    e.executor.Close();
}

// -- detectAndHandleParamModify：资源/元数据变更通知相关用例。
// -- detectAndHandleParamModify: the richer resource/meta-notification logic
// only lives in this standalone method (see module doc comment).

#[test]
/// 参数修改检测循环在 Cancel 时立即返回。
fn test_detect_and_handle_param_modify_loop_breaks_on_cancel() {
    let e = new_env();
    e.executor.Cancel();
    e.executor.detectAndHandleParamModifyLoop(&e.executor.Ctx());
}

#[test]
/// 任务参数未变时 RequiredSlots 保持不变。
fn test_detect_and_handle_param_modify_no_change() {
    let e = new_env();
    e.table.push_task(task1());
    e.executor
        .detectAndHandleParamModify(&Context::Background())
        .unwrap();
    assert_eq!(10, e.executor.GetTaskBase().RequiredSlots);
}

#[test]
/// RequiredSlots 变小：归还槽位并通知 ResourceModified。
fn test_required_slots_become_smaller_apply_successfully() {
    let e = new_env();
    e.executor.SetStepExecutorForTest(e.step.clone());
    e.executor.Param.slotMgr.alloc(&e.executor.GetTaskBase());
    assert_eq!(6, e.executor.Param.slotMgr.availableSlots());
    let mut latest = task1();
    latest.TaskBase.RequiredSlots = 4;
    e.table.push_task(latest);
    e.executor
        .detectAndHandleParamModify(&Context::Background())
        .unwrap();
    assert_eq!(4, e.executor.GetTaskBase().RequiredSlots);
    assert_eq!(12, e.executor.Param.slotMgr.availableSlots());
    assert_eq!(
        vec![StepResource { CPU: 4, Memory: 0 }],
        *e.step.resource_modified_calls.lock().unwrap()
    );
}

#[test]
/// ResourceModified 收到的内存应与 CPU slot 按节点比例折算。
fn test_required_slots_resource_uses_node_proportion() {
    let e = new_env();
    let param = NewParamForTest(
        e.table.clone(),
        Arc::new(newSlotManager(16)),
        crate::NodeResource {
            TotalCPU: 10,
            TotalMem: 100,
            TotalDisk: 0,
        },
        "id",
        e.ext.clone() as Arc<dyn Extension>,
    );
    let executor = NewBaseTaskExecutor(Context::Background(), task1(), param);
    executor.SetStepExecutorForTest(e.step.clone());
    executor.Param.slotMgr.alloc(&executor.GetTaskBase());
    let mut latest = task1();
    latest.TaskBase.RequiredSlots = 4;
    e.table.push_task(latest);
    executor
        .detectAndHandleParamModify(&Context::Background())
        .unwrap();
    assert_eq!(
        vec![StepResource { CPU: 4, Memory: 40 }],
        *e.step.resource_modified_calls.lock().unwrap()
    );
}

#[test]
/// ResourceModified 失败则跳过槽位交换。
fn test_required_slots_become_smaller_but_resource_modified_fails() {
    let e = new_env();
    e.executor.SetStepExecutorForTest(e.step.clone());
    e.executor.Param.slotMgr.alloc(&e.executor.GetTaskBase());
    let mut latest = task1();
    latest.TaskBase.RequiredSlots = 4;
    e.table.push_task(latest);
    e.step
        .resource_modified_result
        .lock()
        .unwrap()
        .push_back(Err(ExecutorError("some error".into())));
    e.executor
        .detectAndHandleParamModify(&Context::Background())
        .unwrap();
    // ResourceModified 失败则整次交换跳过，槽位不变。
    // Since ResourceModified failed, the exchange is skipped entirely, so
    // RequiredSlots and available slots are unchanged.
    assert_eq!(10, e.executor.GetTaskBase().RequiredSlots);
    assert_eq!(6, e.executor.Param.slotMgr.availableSlots());
}

#[test]
/// 槽位不足时不能放大 RequiredSlots。
fn test_required_slots_become_larger_but_not_enough_slots() {
    let e = new_env();
    e.executor.Param.slotMgr.alloc(&e.executor.GetTaskBase());
    e.executor.Param.slotMgr.alloc(&TaskBase {
        ID: 2,
        RequiredSlots: 4,
        ..Default::default()
    });
    assert_eq!(2, e.executor.Param.slotMgr.availableSlots());
    let mut latest = task1();
    latest.TaskBase.RequiredSlots = 14;
    e.table.push_task(latest);
    e.executor
        .detectAndHandleParamModify(&Context::Background())
        .unwrap();
    assert_eq!(10, e.executor.GetTaskBase().RequiredSlots);
    assert_eq!(2, e.executor.Param.slotMgr.availableSlots());
}

#[test]
/// 槽位充足时成功放大 RequiredSlots。
fn test_required_slots_become_larger_apply_successfully() {
    let e = new_env();
    e.executor.SetStepExecutorForTest(e.step.clone());
    e.executor.Param.slotMgr.alloc(&e.executor.GetTaskBase());
    e.executor.Param.slotMgr.alloc(&TaskBase {
        ID: 2,
        RequiredSlots: 4,
        ..Default::default()
    });
    assert_eq!(2, e.executor.Param.slotMgr.availableSlots());
    let mut latest = task1();
    latest.TaskBase.RequiredSlots = 12;
    e.table.push_task(latest);
    e.executor
        .detectAndHandleParamModify(&Context::Background())
        .unwrap();
    assert_eq!(12, e.executor.GetTaskBase().RequiredSlots);
    assert_eq!(0, e.executor.Param.slotMgr.availableSlots());
}

#[test]
/// 放大后通知失败会回滚槽位交换。
fn test_required_slots_become_larger_notify_fails_reverts_exchange() {
    let e = new_env();
    e.executor.SetStepExecutorForTest(e.step.clone());
    e.executor.Param.slotMgr.alloc(&e.executor.GetTaskBase());
    e.executor.Param.slotMgr.alloc(&TaskBase {
        ID: 2,
        RequiredSlots: 4,
        ..Default::default()
    });
    let mut latest = task1();
    latest.TaskBase.RequiredSlots = 12;
    e.table.push_task(latest);
    e.step
        .resource_modified_result
        .lock()
        .unwrap()
        .push_back(Err(ExecutorError("some error".into())));
    e.executor
        .detectAndHandleParamModify(&Context::Background())
        .unwrap();
    assert_eq!(10, e.executor.GetTaskBase().RequiredSlots);
    assert_eq!(2, e.executor.Param.slotMgr.availableSlots());
}

#[test]
/// 任务 Meta 变更会回调 TaskMetaModified。
fn test_task_meta_modified_apply_successfully() {
    let e = new_env();
    e.executor.SetStepExecutorForTest(e.step.clone());
    let mut latest = task1();
    latest.Meta = b"modified".to_vec();
    e.table.push_task(latest);
    e.executor
        .detectAndHandleParamModify(&Context::Background())
        .unwrap();
    assert_eq!(
        b"modified".to_vec(),
        *e.step
            .task_meta_modified_calls
            .lock()
            .unwrap()
            .first()
            .unwrap()
    );
}

#[test]
/// TaskMetaModified 失败向上返回错误。
fn test_task_meta_modified_notify_fails_is_reported() {
    let e = new_env();
    e.executor.SetStepExecutorForTest(e.step.clone());
    let mut latest = task1();
    latest.Meta = b"modified".to_vec();
    e.table.push_task(latest);
    e.step
        .task_meta_modified_result
        .lock()
        .unwrap()
        .push_back(Err(ExecutorError("some error".into())));
    let err = e
        .executor
        .detectAndHandleParamModify(&Context::Background());
    assert!(err.is_err());
}

// -- checkBalanceSubtask：子任务所有权再均衡。
// -- checkBalanceSubtask.

#[test]
/// 已 Cancel 时 checkBalanceSubtask 为空操作。
fn test_check_balance_subtask_context_cancelled_is_noop() {
    let e = new_env();
    e.executor.Cancel();
    e.executor
        .checkBalanceSubtask(&e.executor.Ctx(), &Context::Background());
}

#[test]
/// 无 Running 子任务表示当前子任务已被调度走，取消本地执行。
fn test_check_balance_subtask_no_running_subtasks_cancels() {
    let e = new_env();
    e.table
        .get_subtasks_running
        .lock()
        .unwrap()
        .push_back(Ok(vec![]));
    let cancel_ctx = Context::Background();
    e.executor
        .checkBalanceSubtask(&Context::Background(), &cancel_ctx);
    assert!(cancel_ctx.Done());
}

#[test]
/// 额外的幂等 Running 子任务应回退为 Pending。
fn test_check_balance_subtask_moves_extra_idempotent_subtask_back_to_pending() {
    let e = new_env();
    e.table
        .get_subtasks_running
        .lock()
        .unwrap()
        .push_back(Ok(vec![
            Subtask {
                SubtaskBase: SubtaskBase {
                    ID: 0,
                    ExecID: "id".into(),
                    ..Default::default()
                },
                Meta: vec![],
            },
            Subtask {
                SubtaskBase: SubtaskBase {
                    ID: 99,
                    ExecID: "id".into(),
                    ..Default::default()
                },
                Meta: vec![],
            },
        ]));
    e.ext.idempotent.lock().unwrap().push_back(true);
    let cancel_ctx = Context::Background();
    e.executor
        .checkBalanceSubtask(&Context::Background(), &cancel_ctx);
    assert!(!cancel_ctx.Done());
    assert_eq!(
        vec![vec![99]],
        *e.table.running_back_to_pending_calls.lock().unwrap()
    );
}

#[test]
/// 额外的非幂等 Running 子任务不可重跑，应标记 Failed。
fn test_check_balance_subtask_fails_extra_non_idempotent_subtask() {
    let e = new_env();
    e.table
        .get_subtasks_running
        .lock()
        .unwrap()
        .push_back(Ok(vec![
            Subtask {
                SubtaskBase: SubtaskBase {
                    ID: 0,
                    ExecID: "id".into(),
                    ..Default::default()
                },
                Meta: vec![],
            },
            Subtask {
                SubtaskBase: SubtaskBase {
                    ID: 99,
                    ExecID: "id".into(),
                    ..Default::default()
                },
                Meta: vec![],
            },
        ]));
    e.ext.idempotent.lock().unwrap().push_back(false);
    e.executor
        .checkBalanceSubtask(&Context::Background(), &Context::Background());
    let updates = e.table.update_state_calls.lock().unwrap();
    assert_eq!(
        (99, SubtaskState::Failed, Some(ErrNonIdempotentSubtask())),
        updates[0]
    );
    assert!(
        e.table
            .running_back_to_pending_calls
            .lock()
            .unwrap()
            .is_empty()
    );
}

#[test]
/// 全部 Running 子任务属本 ExecID 时不取消。
fn test_check_balance_subtask_does_not_cancel_when_all_owned_by_this_exec_id() {
    let e = new_env();
    e.table
        .get_subtasks_running
        .lock()
        .unwrap()
        .push_back(Ok(vec![Subtask {
            SubtaskBase: SubtaskBase {
                ID: 0,
                ExecID: "id".into(),
                ..Default::default()
            },
            Meta: vec![],
        }]));
    let cancel_ctx = Context::Background();
    e.executor
        .checkBalanceSubtask(&Context::Background(), &cancel_ctx);
    assert!(!cancel_ctx.Done());
}

#[test]
/// Init 成功且 GetTaskTable 返回同一假表。
fn test_init_and_get_task_table() {
    let e = new_env();
    assert!(e.executor.Init(&Context::Background()).is_ok());
    // GetTaskTable 应返回构造时注入的同一 FakeTaskTable。
    // `GetTaskTable` should hand back the same fake we constructed the
    // executor with.
    e.table.push_task_err(ExecutorError("probe".into()));
    e.executor
        .GetTaskTable()
        .GetTaskByID(&Context::Background(), 0)
        .unwrap_err();
}

#[test]
/// 非空 keyspace 没有 runtime 时，Init 必须拒绝启动执行器。
fn test_init_requires_task_runtime_for_keyspace() {
    let e = new_env();
    let mut task = task1();
    task.TaskBase.Keyspace = "analytics".into();
    let param = NewParamForTest(
        e.table.clone(),
        Arc::new(newSlotManager(16)),
        crate::NodeResource::default(),
        "id",
        e.ext.clone() as Arc<dyn Extension>,
    );
    let executor = NewBaseTaskExecutor(Context::Background(), task, param);
    assert_eq!(
        "task runtime is unavailable",
        executor.Init(&Context::Background()).unwrap_err().0
    );
}

#[test]
/// IsRetryableError 委托给 Extension。
fn test_is_retryable_error_delegates_to_extension() {
    let e = new_env();
    e.ext.retryable.lock().unwrap().push_back(true);
    assert!(e.executor.IsRetryableError(&ExecutorError("x".into())));
    e.ext.retryable.lock().unwrap().push_back(false);
    assert!(!e.executor.IsRetryableError(&ExecutorError("x".into())));
}

#[test]
/// TaskBase::GetRuntimeSlots 返回 RequiredSlots。
fn test_task_base_get_runtime_slots() {
    let base = TaskBase {
        RequiredSlots: 7,
        ..Default::default()
    };
    assert_eq!(7, base.GetRuntimeSlots());
}

#[test]
fn test_subtask_run_logs_failure_and_no_error_on_success() {
    for error in [Some(ExecutorError("disk failed".into())), None] {
        let e = new_env();
        let (logger, logs) = astersql_lightning_log::testlogger::MakeTestLogger([]);
        *e.executor.sampleLogger.write().unwrap() = logger;
        e.table.push_task(task1());
        e.table.push_subtask(Some(pending_subtask1()));
        let result = error.clone();
        e.step
            .run_subtask_result
            .lock()
            .unwrap()
            .push_back(Box::new(move |_, _| match result.clone() {
                Some(error) => Err(error),
                None => Ok(()),
            }));
        e.table
            .push_task_err(ExecutorError("task not found".into()));
        e.executor.Run();
        let lines = logs.lines();
        if let Some(error) = error {
            assert_eq!(1, lines.len(), "{lines:?}");
            assert!(lines[0].contains("run subtask failed"));
            assert!(lines[0].contains("ERROR"));
            assert!(lines[0].contains(&error.0));
            let updates = e.table.update_state_calls.lock().unwrap();
            assert_eq!(1, updates.len());
            assert_eq!(SubtaskState::Failed, updates[0].1);
            assert_eq!(Some(error), updates[0].2);
        } else {
            assert!(lines.is_empty());
            assert_eq!(1, e.table.finish_subtask_calls.lock().unwrap().len());
            assert!(e.table.update_state_calls.lock().unwrap().is_empty());
        }
    }
}

struct CompleteSummaryStep;
impl StepExecutor for CompleteSummaryStep {
    fn RealtimeSummaryJSON(&self) -> Option<String> {
        Some(r#"{"row_count":3,"bytes":14,"get_request_count":5,"put_request_count":0}"#.into())
    }
}

struct CompleteSummaryTable {
    fail: bool,
    calls: Mutex<Vec<(String, String)>>,
}
impl TaskTable for CompleteSummaryTable {
    fn GetTaskByID(&self, _: &Context, _: i64) -> Result<Task> {
        Ok(task1())
    }
    fn UpdateSubtaskSummaryJSON(&self, _: &Context, _: i64, summary: &str) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(("summary".into(), summary.into()));
        if self.fail {
            Err(ExecutorError("cannot persist full summary".into()))
        } else {
            Ok(())
        }
    }
    fn FinishSubtask(&self, _: &Context, _: &str, _: i64, _: &[u8]) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(("finish".into(), String::new()));
        Ok(())
    }
}
fn complete_summary_executor(table: Arc<CompleteSummaryTable>) -> Arc<crate::BaseTaskExecutor> {
    let env = new_env();
    let base = NewBaseTaskExecutor(
        Context::Background(),
        task1(),
        NewParamForTest(
            table,
            Arc::new(newSlotManager(1)),
            crate::NodeResource::default(),
            "node",
            env.ext.clone(),
        ),
    );
    base.SetStepExecutorForTest(Arc::new(CompleteSummaryStep));
    base
}

#[test]
fn complete_summary_is_persisted_before_subtask_success() {
    let table = Arc::new(CompleteSummaryTable {
        fail: false,
        calls: Default::default(),
    });
    complete_summary_executor(table.clone())
        .finishSubtask(&Context::Background(), &pending_subtask1())
        .unwrap();
    let calls = table.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "summary");
    assert_eq!(
        calls[0].1,
        CompleteSummaryStep.RealtimeSummaryJSON().unwrap()
    );
    assert_eq!(calls[1].0, "finish");
}

#[test]
fn complete_summary_persistence_failure_prevents_subtask_success() {
    let table = Arc::new(CompleteSummaryTable {
        fail: true,
        calls: Default::default(),
    });
    let error = complete_summary_executor(table.clone())
        .finishSubtask(&Context::Background(), &pending_subtask1())
        .unwrap_err();
    assert_eq!(error.0, "cannot persist full summary");
    let calls = table.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(calls.iter().all(|(name, _)| name == "summary"));
}
