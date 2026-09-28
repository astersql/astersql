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

// DXF 框架主路径集成测试：注册示例任务、提交/取消、扩缩容、GC、cleanup 与 runtime slots。
//
// 大部分用例归档为 Go 草稿字符串；可编译部分校验 MaxRuntimeSlots
// （最大运行时槽位，限制某 step 可用 CPU/内存）对 StepResource 的裁剪。

// register_example_task_with_dxf_ctx 对应 Go 的 registerExampleTaskWithDXFCtx。
// 它把 TestDXFContext 中的 T、MockCtrl、TestContext 拆出来转给通用注册函数。
// Copyright 2026 AsterSQL.
/// 归档 registerExampleTaskWithDXFCtx：从 TestDXFContext 拆参转通用注册。
const _GO_FRAMEWORK_REFERENCE: &str = r###"
pub fn register_example_task_with_dxf_ctx(
    c: &testutil::TestDXFContext,
    scheduler_ext: scheduler::Extension,
    run_subtask_fn: Option<fn(context::Context, &proto::Subtask) -> errors::Error>,
) {
    register_example_task(c.T, c.MockCtrl, scheduler_ext, c.TestContext, run_subtask_fn);
}
"###;

/// 校验 MaxRuntimeSlots 将 StepOne 的 CPU/内存容量从节点总量裁剪到上限。
#[test]
fn runtime_slot_limit_controls_step_resource() {
    use astersql_dxf_framework_proto::{ExtraParams, NewNodeResource, StepOne, TaskBase};
    let task = TaskBase {
        ID: 1,
        Key: "task".into(),
        Type: astersql_dxf_framework_proto::TaskTypeExample,
        State: astersql_dxf_framework_proto::TaskStateRunning,
        RequiredSlots: 8,
        Step: StepOne,
        Priority: astersql_dxf_framework_proto::NormalPriority,
        TargetScope: String::new(),
        CreateTime: std::time::SystemTime::UNIX_EPOCH,
        MaxNodeCount: 0,
        // 仅对 TargetSteps 中的 StepOne 施加 3 槽上限；CPU/Mem 应按比例缩小。
        ExtraParams: ExtraParams {
            MaxRuntimeSlots: 3,
            TargetSteps: vec![StepOne],
            ..Default::default()
        },
        Keyspace: String::new(),
    };
    let resource = NewNodeResource(16, 16_000, 100_000).GetStepResource(&task);
    assert_eq!(resource.CPU.Capacity(), 3);
    assert_eq!(resource.Mem.Capacity(), 3_000);
}

/// The common example-task registration used by the Go tests must preserve
/// the two-step 3+1 subtask script and its metadata.
#[test]
fn common_framework_scheduler_keeps_two_step_task_shape() {
    use astersql_dxf_framework_testutil::{
        GetMockBasicSchedulerExt, STEP_INIT, STEP_ONE, STEP_TWO, Task,
    };

    let extension = GetMockBasicSchedulerExt();
    let task = Task::default();
    assert_eq!(extension.next_step(STEP_INIT), STEP_ONE);
    assert_eq!(
        extension
            .next_subtasks_batch(&task, STEP_ONE)
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        extension
            .next_subtasks_batch(&task, STEP_TWO)
            .unwrap()
            .len(),
        1
    );
}

#[derive(Default)]
struct RecordingDxfRuntime {
    calls: std::sync::Mutex<Vec<String>>,
}

impl RecordingDxfRuntime {
    fn record(&self, event: &str) {
        self.calls.lock().unwrap().push(event.to_owned());
    }
}

impl astersql_dxf_framework_testutil::DxfRuntime for RecordingDxfRuntime {
    fn set_node_resource(
        &self,
        _resource: astersql_dxf_framework_testutil::NodeResource,
    ) -> Result<
        astersql_dxf_framework_testutil::NodeResource,
        astersql_dxf_framework_testutil::DxfError,
    > {
        self.record("resource");
        Ok(astersql_dxf_framework_testutil::NodeResource::for_cpu(1))
    }

    fn start_executor(
        &self,
        node_id: &str,
        _resource: astersql_dxf_framework_testutil::NodeResource,
    ) -> Result<(), astersql_dxf_framework_testutil::DxfError> {
        self.record(&format!("start-executor:{node_id}"));
        Ok(())
    }

    fn stop_executor(
        &self,
        node_id: &str,
    ) -> Result<(), astersql_dxf_framework_testutil::DxfError> {
        self.record(&format!("stop-executor:{node_id}"));
        Ok(())
    }

    fn cancel_executor(
        &self,
        node_id: &str,
    ) -> Result<(), astersql_dxf_framework_testutil::DxfError> {
        self.record(&format!("cancel-executor:{node_id}"));
        Ok(())
    }

    fn start_scheduler(
        &self,
        node_id: &str,
        _resource: astersql_dxf_framework_testutil::NodeResource,
    ) -> Result<(), astersql_dxf_framework_testutil::DxfError> {
        self.record(&format!("start-scheduler:{node_id}"));
        Ok(())
    }

    fn stop_scheduler(
        &self,
        node_id: &str,
    ) -> Result<(), astersql_dxf_framework_testutil::DxfError> {
        self.record(&format!("stop-scheduler:{node_id}"));
        Ok(())
    }

    fn cancel_scheduler(
        &self,
        node_id: &str,
    ) -> Result<(), astersql_dxf_framework_testutil::DxfError> {
        self.record(&format!("cancel-scheduler:{node_id}"));
        Ok(())
    }

    fn update_live_executor_ids(
        &self,
        node_ids: &[String],
    ) -> Result<(), astersql_dxf_framework_testutil::DxfError> {
        self.record(&format!("live:{node_ids:?}"));
        Ok(())
    }

    fn set_check_intervals(
        &self,
        _intervals: astersql_dxf_framework_testutil::CheckIntervals,
    ) -> Result<
        astersql_dxf_framework_testutil::CheckIntervals,
        astersql_dxf_framework_testutil::DxfError,
    > {
        self.record("intervals");
        Ok(astersql_dxf_framework_testutil::CheckIntervals {
            scheduler_running: std::time::Duration::from_secs(1),
            scheduler_finished: std::time::Duration::from_secs(1),
            cleanup: std::time::Duration::from_secs(1),
            task: std::time::Duration::from_secs(1),
            subtask: std::time::Duration::from_secs(1),
            max_subtask: std::time::Duration::from_secs(1),
            detect_modification: std::time::Duration::from_secs(1),
        })
    }
}

#[test]
fn framework_owner_change_and_scale_lifecycle_preserves_live_nodes() {
    use astersql_dxf_framework_testutil::NewTestDXFContext;
    use std::sync::Arc;

    let runtime = Arc::new(RecordingDxfRuntime::default());
    let context = NewTestDXFContext(runtime.clone(), 5, 16, true).unwrap();
    context.ChangeOwner().unwrap();
    context.ScaleOut(1).unwrap();
    context.ScaleIn(2).unwrap();
    assert_eq!(context.NodeCount(), 4);
    assert!(
        runtime
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("live:"))
    );
}

#[test]
fn framework_parallel_task_observation_keeps_distinct_task_and_step_counts() {
    use astersql_dxf_framework_testutil::{STEP_ONE, STEP_TWO, Step, Subtask, TestContext};
    use std::sync::Arc;
    use std::thread;

    let context = Arc::new(TestContext::default());
    let mut joins = Vec::new();
    for task_id in 1..=10 {
        let context = context.clone();
        joins.push(thread::spawn(move || {
            for subtask_id in 1..=3 {
                context.CollectSubtask(&Subtask {
                    id: subtask_id,
                    task_id,
                    step: STEP_ONE,
                    ..Default::default()
                });
            }
            context.CollectSubtask(&Subtask {
                id: 4,
                task_id,
                step: STEP_TWO,
                ..Default::default()
            });
        }));
    }
    for join in joins {
        join.join().unwrap();
    }
    assert_eq!(context.CollectedSubtaskCnt(1, STEP_ONE), 3);
    assert_eq!(context.CollectedSubtaskCnt(10, STEP_TWO), 1);
    assert_eq!(context.CollectedSubtaskCnt(1, Step(99)), 0);
}

#[test]
fn framework_executor_failure_is_returned_without_hiding_cleanup() {
    use astersql_dxf_framework_testutil::{
        DxfError, GetCommonStepExecutor, GetCommonTaskExecutorExt, STEP_ONE, Subtask, Task,
        run_registered_subtask,
    };
    use std::sync::Arc;

    let extension = GetCommonTaskExecutorExt(Arc::new(|task: &Task| {
        Ok(GetCommonStepExecutor(
            task.base.step,
            Arc::new(|_| Err(DxfError("run failed".into()))),
        ))
    }));
    let mut task = Task::default();
    task.base.step = STEP_ONE;
    let error = run_registered_subtask(&extension, &task, &Subtask::default()).unwrap_err();
    assert_eq!(error.to_string(), "run failed");
}

#[test]
fn framework_cleanup_routine_is_repeatable_after_task_transfer_error() {
    use astersql_dxf_framework_testutil::GetCommonCleanUpRoutine;

    let cleanup = GetCommonCleanUpRoutine();
    assert!(cleanup.cleanup(1).is_ok());
    assert!(cleanup.cleanup(2).is_ok());
}

#[test]
fn framework_keyspace_and_task_terminal_states_match_kernel_modes() {
    use astersql_dxf_framework_scheduler::{TASK_STATE_REVERTED, TASK_STATE_RUNNING, TaskBase};
    use astersql_dxf_framework_testutil::{TaskState, getTaskKS};

    assert_eq!(getTaskKS(false), "");
    assert_eq!(getTaskKS(true), "SYSTEM");
    assert!(
        !TaskBase {
            state: TASK_STATE_RUNNING,
            ..Default::default()
        }
        .is_done()
    );
    assert!(
        TaskBase {
            state: TASK_STATE_REVERTED,
            ..Default::default()
        }
        .is_done()
    );
    assert_eq!(TaskState::Running, TaskState::Running);
}

#[test]
fn framework_executor_factory_error_is_not_replaced_by_success() {
    use astersql_dxf_framework_testutil::{DxfError, GetTaskExecutorExt, Task};
    use std::sync::Arc;

    let extension = GetTaskExecutorExt(
        Arc::new(|_| Err(DxfError("init environment failed".into()))),
        Arc::new(|_| false),
    );
    let error = match extension.get_step_executor(&Task::default()) {
        Ok(_) => panic!("executor factory unexpectedly succeeded"),
        Err(error) => error,
    };
    assert_eq!(error.to_string(), "init environment failed");
}

#[test]
fn framework_cancel_context_reports_cancellation_to_waiters() {
    use astersql_dxf_framework_scheduler::Context;

    let context = Context::default();
    assert!(!context.is_cancelled());
    context.cancel();
    assert!(context.is_cancelled());
}

#[test]
fn framework_runtime_slots_and_prepare_mode_keep_legacy_defaults() {
    use astersql_dxf_framework_proto::{
        ExtraParams, PrepareModeDisabled, PrepareModeExt, StepOne, TaskBase,
    };

    let task = TaskBase {
        ID: 0,
        Key: String::new(),
        Type: astersql_dxf_framework_proto::TaskTypeExample,
        State: astersql_dxf_framework_proto::TaskStatePending,
        RequiredSlots: 9,
        Step: StepOne,
        Priority: astersql_dxf_framework_proto::NormalPriority,
        TargetScope: String::new(),
        CreateTime: std::time::SystemTime::UNIX_EPOCH,
        MaxNodeCount: 0,
        ExtraParams: ExtraParams {
            PrepareMode: PrepareModeDisabled,
            ..ExtraParams::default()
        },
        Keyspace: String::new(),
    };
    assert_eq!(
        astersql_dxf_framework_proto::NewNodeResource(16, 16_000, 1)
            .GetStepResource(&task)
            .CPU
            .Capacity(),
        9
    );
    assert_eq!(PrepareModeDisabled.String(), "disabled");
}
/// 归档主框架用例：owner 漂移、扩缩容、取消、GC、cleanup、PrepareMode 与 runtime slots。
const _GO_FRAMEWORK_REMAINDER: &str = r####"

// register_example_task 对应 Go 的 registerExampleTask。
// 当调用方不传 runSubtaskFn 时，默认收集 StepOne/StepTwo subtask；executorExt 和 cleanup routine 都沿用 testutil mock。
pub fn register_example_task(
    t: testing::TB,
    ctrl: &gomock::Controller,
    scheduler_ext: scheduler::Extension,
    test_context: &testutil::TestContext,
    run_subtask_fn: Option<fn(context::Context, &proto::Subtask) -> errors::Error>,
) {
    let run_subtask_fn = run_subtask_fn.unwrap_or_else(|| get_common_subtask_run_fn(test_context));
    let executor_ext = testutil::GetCommonTaskExecutorExt(ctrl, |task: &proto::Task| {
        Ok(testutil::GetCommonStepExecutor(ctrl, task.Step, run_subtask_fn))
    });
    testutil::RegisterExampleTask(t, scheduler_ext, executor_ext, testutil::GetCommonCleanUpRoutine(ctrl));
}

// get_common_subtask_run_fn 对应 Go 的 getCommonSubtaskRunFn。
// 只接受 StepOne/StepTwo，其它 step 按 Go 原逻辑 panic，方便发现 scheduler 走错阶段。
pub fn get_common_subtask_run_fn(test_ctx: &testutil::TestContext) -> fn(context::Context, &proto::Subtask) -> errors::Error {
    move |_ctx, subtask| {
        match subtask.Step {
            proto::StepOne | proto::StepTwo => test_ctx.CollectSubtask(subtask),
            _ => panic!("invalid step"),
        }
        Ok(())
    }
}

// submit_task_and_check_success_for_basic 对应 Go helper。
// 目标 scope 仍来自 handle.GetTargetScope，预期 StepOne 为 3 个 subtask，StepTwo 为 1 个 subtask。
pub fn submit_task_and_check_success_for_basic(
    ctx: context::Context,
    t: &testing::T,
    task_key: &str,
    test_context: &testutil::TestContext,
) -> i64 {
    let scope = handle::GetTargetScope();
    submit_task_and_check_success(
        ctx,
        t,
        task_key,
        scope,
        test_context,
        map! {
            proto::StepOne => 3,
            proto::StepTwo => 1,
        },
    )
}

// submit_task_and_check_success 对应 Go 的 submitTaskAndCheckSuccess。
// 提交任务后等待完成，并按 step 校验 TestContext 收集到的 subtask 数量。
pub fn submit_task_and_check_success(
    ctx: context::Context,
    t: &testing::T,
    task_key: &str,
    target_scope: &str,
    test_context: &testutil::TestContext,
    subtask_cnts: map::Map<proto::Step, i32>,
) -> i64 {
    let task = testutil::SubmitAndWaitTask(ctx, t, task_key, target_scope, 1);
    require::Equal(t, proto::TaskStateSucceed, task.State);
    for (step, cnt) in subtask_cnts {
        require::Equal(t, cnt, test_context.CollectedSubtaskCnt(task.ID, step));
    }
    task.ID
}

// test_random_owner_change_with_multiple_tasks 对应 Go 的 TestRandomOwnerChangeWithMultipleTasks。
// 多个任务并发提交，同时随机触发 owner change，验证框架在 owner 漂移期间仍能完成任务。
#[test]
pub fn test_random_owner_change_with_multiple_tasks() {
    let c = testutil::NewTestDXFContext(t, 5, 16, true);
    register_example_task(t, c.MockCtrl, testutil::GetMockBasicSchedulerExt(c.MockCtrl), c.TestContext, None);
    let wg = util::WaitGroupWrapper::new();
    for i in 0..10 {
        let task_key = format!("key{}", i);
        wg.Run(|| submit_task_and_check_success_for_basic(c.Ctx, t, &task_key, c.TestContext));
    }
    wg.Run(|| {
        let seed = time::Now().UnixNano();
        t.Logf("seed in change owner loop: {}", seed);
        let random = rand::New(rand::NewSource(seed));
        for _ in 0..3 {
            c.ChangeOwner();
            time::Sleep(time::Duration(random.Int63n(3 * time::Second)));
        }
    });
    wg.Wait();
}

// test_framework_scale_in_and_out 对应 Go 的 TestFrameworkScaleInAndOut。
// 任务提交和节点扩缩容并发进行，保留随机 seed 日志以便复现。
#[test]
pub fn test_framework_scale_in_and_out() {
    let c = testutil::NewTestDXFContext(t, 5, 16, true);
    let seed = time::Now().UnixNano();
    t.Logf("seed: {}", seed);
    let random = rand::New(rand::NewSource(seed));
    register_example_task(t, c.MockCtrl, testutil::GetMockBasicSchedulerExt(c.MockCtrl), c.TestContext, None);
    let wg = util::WaitGroupWrapper::new();
    for i in 0..12 {
        let task_key = format!("key{}", i);
        wg.Run(|| submit_task_and_check_success_for_basic(c.Ctx, t, &task_key, c.TestContext));
    }
    wg.Run(|| {
        for _ in 0..3 {
            if random.Intn(2) == 0 {
                c.ScaleOut(1);
            } else {
                c.ScaleIn(1);
            }
            time::Sleep(time::Duration(random.Int63n(3 * time::Second)));
        }
    });
    wg.Wait();
}

// test_framework_with_query 对应 Go 的 TestFrameworkWithQuery。
// Runs SQL while background tasks execute to verify DXF/SQL coexistence.
#[test]
pub fn test_framework_with_query() {
    let c = testutil::NewTestDXFContext(t, 2, 16, true);
    register_example_task(t, c.MockCtrl, testutil::GetMockBasicSchedulerExt(c.MockCtrl), c.TestContext, None);
    let wg = util::WaitGroupWrapper::new();
    wg.Run(|| submit_task_and_check_success_for_basic(c.Ctx, t, "key1", c.TestContext));
    let tk = testkit::NewTestKit(t, c.Store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t");
    tk.MustExec("create table t(a int not null, b int not null)");
    let (rs, err) = tk.Exec("select ifnull(a,b) from t");
    require::NoError(t, err);
    require::Greater(t, rs.Fields().len(), 0);
    require::Equal(t, "ifnull(a,b)", rs.Fields()[0].Column.Name.L);
    require::NoError(t, rs.Close());
    wg.Wait();
}

// test_framework_cancel_task 对应 Go 的 TestFrameworkCancelTask。
// afterRunSubtask failpoint 第一次命中时取消当前任务，最终应进入 Reverted。
#[test]
pub fn test_framework_cancel_task() {
    let c = testutil::NewTestDXFContext(t, 2, 16, true);
    register_example_task(t, c.MockCtrl, testutil::GetMockBasicSchedulerExt(c.MockCtrl), c.TestContext, None);
    let counter = atomic::Int32::new(0);
    testfailpoint::EnableCall(
        t,
        "github.com/pingcap/tidb/pkg/dxf/framework/taskexecutor/afterRunSubtask",
        |e: taskexecutor::TaskExecutor, _err: &mut errors::Error, _ctx: context::Context| {
            if counter.Add(1) == 1 {
                require::NoError(t, c.TaskMgr.CancelTask(c.Ctx, e.GetTaskBase().ID));
            }
        },
    );
    let scope = handle::GetTargetScope();
    let task = testutil::SubmitAndWaitTask(c.Ctx, t, "key1", scope, 1);
    require::Equal(t, proto::TaskStateReverted, task.State);
}

// test_framework_sub_task_init_env_failed 对应 Go 的 TestFrameworkSubTaskInitEnvFailed。
// mock StepExecutor.Init 持续返回错误，任务应回滚到 Reverted。
#[test]
pub fn test_framework_sub_task_init_env_failed() {
    let c = testutil::NewTestDXFContext(t, 1, 16, true);
    let scheduler_ext = testutil::GetMockBasicSchedulerExt(c.MockCtrl);
    let step_exec = mockexecute::NewMockStepExecutor(c.MockCtrl);
    step_exec.EXPECT().Init(gomock::Any()).Return(errors::New("mockExecSubtaskInitEnvErr")).AnyTimes();
    let executor_ext = testutil::GetCommonTaskExecutorExt(c.MockCtrl, |_task: &proto::Task| Ok(step_exec));
    testutil::RegisterExampleTask(t, scheduler_ext, executor_ext, testutil::GetCommonCleanUpRoutine(c.MockCtrl));
    let scope = handle::GetTargetScope();
    let task = testutil::SubmitAndWaitTask(c.Ctx, t, "key1", scope, 1);
    require::Equal(t, proto::TaskStateReverted, task.State);
}

// test_owner_change_when_schedule 对应 Go 的 TestOwnerChangeWhenSchedule。
// mockOwnerChange failpoint 第一次命中时异步切 owner，并短暂 sleep 等待调度竞争窗口。
#[test]
pub fn test_owner_change_when_schedule() {
    let c = testutil::NewTestDXFContext(t, 3, 16, true);
    register_example_task(t, c.MockCtrl, testutil::GetMockBasicSchedulerExt(c.MockCtrl), c.TestContext, None);
    let counter = atomic::Int32::new(0);
    require::NoError(t, failpoint::EnableCall("github.com/pingcap/tidb/pkg/dxf/framework/scheduler/mockOwnerChange", || {
        if counter.Add(1) == 1 {
            c.AsyncChangeOwner();
            time::Sleep(time::Second);
        }
    }));
    t.Cleanup(|| require::NoError(t, failpoint::Disable("github.com/pingcap/tidb/pkg/dxf/framework/scheduler/mockOwnerChange")));
    submit_task_and_check_success_for_basic(c.Ctx, t, "😊", c.TestContext);
}

// test_gc 对应 Go 的 TestGC。
// 通过两个 failpoint 缩短历史 subtask 保留时间和 GC 间隔，先确认历史表有记录，再放行 GC 后确认清空。
#[test]
pub fn test_gc() {
    let ch = chan::make::<()>();
    testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/dxf/framework/storage/subtaskHistoryKeepSeconds", |interval: &mut i32| {
        *interval = 1;
    });
    testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/dxf/framework/scheduler/historySubtaskTableGcInterval", |interval: &mut time::Duration| {
        *interval = time::Second;
        ch.recv();
    });
    let c = testutil::NewTestDXFContext(t, 3, 16, true);
    register_example_task(t, c.MockCtrl, testutil::GetMockBasicSchedulerExt(c.MockCtrl), c.TestContext, None);
    submit_task_and_check_success_for_basic(c.Ctx, t, "😊", c.TestContext);
    let (mgr, err) = storage::GetTaskManager();
    require::NoError(t, err);
    require::Eventually(t, || testutil::GetSubtasksFromHistory(c.Ctx, mgr).unwrap_or(0) == 4, 10 * time::Second, 500 * time::Millisecond);
    ch.send(());
    require::Eventually(t, || testutil::GetSubtasksFromHistory(c.Ctx, mgr).unwrap_or(1) == 0, 10 * time::Second, 500 * time::Millisecond);
}

// test_framework_run_subtask_cancel_or_failed 对应 Go 的 TestFrameworkRunSubtaskCancelOrFailed。
// 两个子测试分别注入取消错误和普通执行错误，最终都应让任务回滚。
#[test]
pub fn test_framework_run_subtask_cancel_or_failed() {
    let c = testutil::NewTestDXFContext(t, 3, 16, true);
    let scope = handle::GetTargetScope();
    register_example_task(t, c.MockCtrl, testutil::GetMockBasicSchedulerExt(c.MockCtrl), c.TestContext, None);

    t.Run("meet cancel on run subtask", |t| {
        let counter = atomic::Int32::new(0);
        testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/dxf/framework/taskexecutor/afterRunSubtask", |e: taskexecutor::TaskExecutor, err_p: &mut errors::Error, _ctx: context::Context| {
            if counter.Add(1) == 1 {
                e.CancelRunningSubtask();
                *err_p = taskexecutor::ErrCancelSubtask;
            }
        });
        let task = testutil::SubmitAndWaitTask(c.Ctx, t, "key1", scope, 1);
        require::Equal(t, proto::TaskStateReverted, task.State);
    });

    t.Run("meet some error on run subtask", |t| {
        let counter = atomic::Int32::new(0);
        testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/dxf/framework/taskexecutor/afterRunSubtask", |_e, err_p: &mut errors::Error, _ctx| {
            if counter.Add(1) == 1 {
                *err_p = errors::New("MockExecutorRunErr");
            }
        });
        let task = testutil::SubmitAndWaitTask(c.Ctx, t, "key2", scope, 1);
        require::Equal(t, proto::TaskStateReverted, task.State);
    });
}

// test_framework_clean_up_routine 对应 Go 的 TestFrameworkCleanUpRoutine。
// 临时缩短 DefaultCleanUpInterval，校验正常 cleanup 与 transfer err 场景都能保留历史信息。
#[test]
pub fn test_framework_clean_up_routine() {
    let bak = scheduler::DefaultCleanUpInterval;
    defer(|| scheduler::DefaultCleanUpInterval = bak);
    scheduler::DefaultCleanUpInterval = 500 * time::Millisecond;
    let c = testutil::NewTestDXFContext(t, 3, 16, true);
    register_example_task(t, c.MockCtrl, testutil::GetMockBasicSchedulerExt(c.MockCtrl), c.TestContext, None);
    let ch = chan::make_buffered::<()>(1);
    testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/dxf/framework/scheduler/WaitCleanUpFinished", || ch.send(()));

    submit_task_and_check_success_for_basic(c.Ctx, t, "key1", c.TestContext);
    ch.recv();
    let (mgr, err) = storage::GetTaskManager();
    require::NoError(t, err);
    require::NotEmpty(t, mgr.GetTaskByKeyWithHistory(c.Ctx, "key1"));
    require::NotEmpty(t, testutil::GetSubtasksFromHistory(c.Ctx, mgr));

    testfailpoint::Enable(t, "github.com/pingcap/tidb/pkg/dxf/framework/scheduler/mockTransferErr", "1*return()");
    submit_task_and_check_success_for_basic(c.Ctx, t, "key2", c.TestContext);
    ch.recv();
    require::NotEmpty(t, mgr.GetTaskByKeyWithHistory(c.Ctx, "key1"));
    require::NotEmpty(t, testutil::GetSubtasksFromHistory(c.Ctx, mgr));
}

// test_task_cancelled_before_update_task 对应 Go 的 TestTaskCancelledBeforeUpdateTask。
// scheduler 更新 task 前通过 failpoint 取消任务，验证最终状态为 Reverted。
#[test]
pub fn test_task_cancelled_before_update_task() {
    let c = testutil::NewTestDXFContext(t, 1, 16, true);
    register_example_task(t, c.MockCtrl, testutil::GetMockBasicSchedulerExt(c.MockCtrl), c.TestContext, None);
    let counter = atomic::Int32::new(0);
    testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/dxf/framework/scheduler/cancelBeforeUpdateTask", |task_id: i64| {
        if counter.Add(1) == 1 {
            require::NoError(t, c.TaskMgr.CancelTask(c.Ctx, task_id));
        }
    });
    let scope = handle::GetTargetScope();
    let task = testutil::SubmitAndWaitTask(c.Ctx, t, "key1", scope, 1);
    require::Equal(t, proto::TaskStateReverted, task.State);
}

// test_dxf_always_enabled_on_next_gen 对应 Go 的 TestDXFAlwaysEnabledOnNextGen。
// Classic 跳过；NextGen 下校验 tidb_enable_dist_task 固定开启且不可关闭。
#[test]
pub fn test_dxf_always_enabled_on_next_gen() {
    if kerneltype::IsClassic() {
        t.Skip("This test is only for next-gen TiDB");
    }
    let store = testkit::CreateMockStore(t);
    let tk = testkit::NewTestKit(t, store);
    tk.MustQuery("select @@global.tidb_enable_dist_task").Equal(testkit::Rows("1"));
    require::ErrorContains(
        t,
        tk.ExecToErr("set global tidb_enable_dist_task=0"),
        "setting tidb_enable_dist_task is not supported in the next generation of TiDB",
    );
}

// test_max_runtime_slots 对应 Go 的 TestMaxRuntimeSlots。
// 三个子测试分别覆盖 target steps 限流、PrepareModeRequired 的 OnPrepare 更新传播、PrepareModeDisabled 的兼容路径。
#[test]
pub fn test_max_runtime_slots() {
    t.Run("limit-runtime-slots-in-target-steps", |t| {
        let c = testutil::NewTestDXFContext(t, 1, 16, true);
        register_example_task(t, c.MockCtrl, testutil::GetMockBasicSchedulerExt(c.MockCtrl), c.TestContext, None);
        let call_count = atomic::Int32::new(0);
        testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/dxf/framework/taskexecutor/beforeSetFrameworkInfo", |rc: &proto::StepResource| {
            let val = call_count.Add(1);
            if val == 1 {
                require::Equal(t, 12, rc.CPU.Capacity() as i32);
            } else {
                require::Equal(t, 16, rc.CPU.Capacity() as i32);
            }
        });
        testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/dxf/framework/storage/beforeSubmitTask", |_required_slots: &mut i32, params: &mut proto::ExtraParams| {
            params.MaxRuntimeSlots = 12;
            params.TargetSteps = vec![proto::StepOne];
        });
        let scope = handle::GetTargetScope();
        let (cpu_count, err) = c.TaskMgr.GetCPUCountOfNode(c.Ctx);
        require::NoError(t, err);
        require::Equal(t, 16, cpu_count);
        let task = testutil::SubmitAndWaitTask(c.Ctx, t, "key1", scope, 16);
        require::Equal(t, proto::TaskStateSucceed, task.State);
        require::EqualValues(t, 2, call_count.Load());
    });

    t.Run("prepare-mode-required-propagates-onprepare-updates", |t| {
        let c = testutil::NewTestDXFContext(t, 1, 16, true);
        const PREPARE_META: &str = r#"{"prepare":"done"}"#;
        const PREPARE_SLOTS: i32 = 7;
        const PREPARE_NODE_CAP: i32 = 1;
        const INITIAL_TASK_META: &str = r#"{"prepare":"init"}"#;
        let on_prepare_called = atomic::Int32::new(0);
        let step_one_task_seen = atomic::Int32::new(0);

        // Go 用 mock scheduler extension 编排 Prepared -> StepOne -> Done，并校验 OnPrepare 改写 task 字段能传给后续 subtask。
        let step_transition = map! {
            proto::StepPrepared => proto::StepOne,
            proto::StepOne => proto::StepDone,
        };
        let scheduler_ext = mockDispatch::NewMockExtension(c.MockCtrl);
        scheduler_ext.EXPECT().OnTick(gomock::Any(), gomock::Any()).Return().AnyTimes();
        scheduler_ext.EXPECT().GetEligibleInstances(gomock::Any(), gomock::Any()).Return(None, None).AnyTimes();
        scheduler_ext.EXPECT().IsRetryableErr(gomock::Any()).Return(false).AnyTimes();
        scheduler_ext.EXPECT().GetNextStep(gomock::Any()).DoAndReturn(|task: &proto::TaskBase| {
            let next_step = step_transition[task.Step];
            require::True(t, next_step != proto::StepUnspecified, format!("unexpected step: {}", task.Step));
            next_step
        }).AnyTimes();
        scheduler_ext.EXPECT().OnPrepare(gomock::Any(), gomock::Any(), gomock::Any()).DoAndReturn(|_ctx, _handle, task: &mut proto::Task| {
            on_prepare_called.Add(1);
            task.Meta = PREPARE_META.as_bytes().to_vec();
            task.RequiredSlots = PREPARE_SLOTS;
            task.MaxNodeCount = PREPARE_NODE_CAP;
            Ok(())
        }).Times(1);
        scheduler_ext.EXPECT().OnNextSubtasksBatch(gomock::Any(), gomock::Any(), gomock::Any(), gomock::Any(), gomock::Any()).DoAndReturn(|_ctx, _handle, task: &proto::Task, _nodes, next_step| {
            require::Equal(t, proto::StepOne, next_step);
            require::Equal(t, proto::StepPrepared, task.Step);
            require::Equal(t, PREPARE_META.as_bytes(), task.Meta);
            require::Equal(t, PREPARE_SLOTS, task.RequiredSlots);
            require::Equal(t, PREPARE_NODE_CAP, task.MaxNodeCount);
            Ok(vec![b"step-one-subtask".to_vec()])
        }).Times(1);
        scheduler_ext.EXPECT().OnDone(gomock::Any(), gomock::Any(), gomock::Any()).Return(None).Times(1);

        let executor_ext = testutil::GetCommonTaskExecutorExt(c.MockCtrl, |task: &proto::Task| {
            if task.Step == proto::StepOne {
                step_one_task_seen.Add(1);
                require::Equal(t, PREPARE_META.as_bytes(), task.Meta);
                require::Equal(t, PREPARE_SLOTS, task.RequiredSlots);
                require::Equal(t, PREPARE_NODE_CAP, task.MaxNodeCount);
            }
            Ok(testutil::GetCommonStepExecutor(c.MockCtrl, task.Step, |_ctx, subtask| {
                require::Equal(t, proto::StepOne, subtask.Step);
                Ok(())
            }))
        });
        testutil::RegisterExampleTask(t, scheduler_ext, executor_ext, testutil::GetCommonCleanUpRoutine(c.MockCtrl));
        testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/dxf/framework/storage/beforeSubmitTask", |_required_slots, params: &mut proto::ExtraParams| {
            params.PrepareMode = proto::PrepareModeRequired;
        });
        let scope = handle::GetTargetScope();
        let (task, err) = handle::SubmitTask(c.Ctx, "prepare-mode-required", proto::TaskTypeExample, c.Store.GetKeyspace(), 1, scope, 0, INITIAL_TASK_META.as_bytes());
        require::NoError(t, err);
        let done_task = testutil::WaitTaskDone(c.Ctx, t, task.Key);
        require::Equal(t, proto::TaskStateSucceed, done_task.State);
        require::EqualValues(t, 1, on_prepare_called.Load());
        require::EqualValues(t, 1, step_one_task_seen.Load());
    });

    t.Run("prepare-mode-disabled-keeps-backward-compatible-path", |t| {
        let c = testutil::NewTestDXFContext(t, 1, 16, true);
        const INITIAL_TASK_META: &str = r#"{"prepare":"old-disabled"}"#;
        let step_transition = map! {
            proto::StepInit => proto::StepOne,
            proto::StepOne => proto::StepDone,
        };
        let scheduler_ext = mockDispatch::NewMockExtension(c.MockCtrl);
        scheduler_ext.EXPECT().OnTick(gomock::Any(), gomock::Any()).Return().AnyTimes();
        scheduler_ext.EXPECT().GetEligibleInstances(gomock::Any(), gomock::Any()).Return(None, None).AnyTimes();
        scheduler_ext.EXPECT().IsRetryableErr(gomock::Any()).Return(false).AnyTimes();
        scheduler_ext.EXPECT().GetNextStep(gomock::Any()).DoAndReturn(|task: &proto::TaskBase| step_transition[task.Step]).AnyTimes();
        scheduler_ext.EXPECT().OnPrepare(gomock::Any(), gomock::Any(), gomock::Any()).Times(0);
        scheduler_ext.EXPECT().OnNextSubtasksBatch(gomock::Any(), gomock::Any(), gomock::Any(), gomock::Any(), gomock::Any()).DoAndReturn(|_ctx, _handle, task: &proto::Task, _nodes, next_step| {
            require::Equal(t, proto::StepOne, next_step);
            require::Equal(t, proto::StepInit, task.Step);
            require::Equal(t, INITIAL_TASK_META.as_bytes(), task.Meta);
            Ok(vec![b"step-one-subtask".to_vec()])
        }).Times(1);
        scheduler_ext.EXPECT().OnDone(gomock::Any(), gomock::Any(), gomock::Any()).Return(None).Times(1);
        let executor_ext = testutil::GetCommonTaskExecutorExt(c.MockCtrl, |task: &proto::Task| {
            if task.Step == proto::StepOne {
                require::Equal(t, INITIAL_TASK_META.as_bytes(), task.Meta);
            }
            Ok(testutil::GetCommonStepExecutor(c.MockCtrl, task.Step, |_ctx, subtask| {
                require::Equal(t, proto::StepOne, subtask.Step);
                Ok(())
            }))
        });
        testutil::RegisterExampleTask(t, scheduler_ext, executor_ext, testutil::GetCommonCleanUpRoutine(c.MockCtrl));
        let scope = handle::GetTargetScope();
        let (task, err) = handle::SubmitTask(c.Ctx, "prepare-mode-disabled", proto::TaskTypeExample, c.Store.GetKeyspace(), 1, scope, 0, INITIAL_TASK_META.as_bytes());
        require::NoError(t, err);
        let done_task = testutil::WaitTaskDone(c.Ctx, t, task.Key);
        require::Equal(t, proto::TaskStateSucceed, done_task.State);
    });
}
"####;
