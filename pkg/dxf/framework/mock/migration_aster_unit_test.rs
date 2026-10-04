// Copyright 2026 AsterSQL.

// DXF 框架 mock 的迁移回归测试。
//
// 覆盖 GoMock 风格回调容器及规划、存储、调度和执行侧替身的关键契约，
// 确保参数与返回值按原样转发、生命周期方法派发到独立处理器，且调用次数可观测。

use crate::{Handler, NewMockManager, NewMockPipelineSpec, NewMockScheduler, NewMockTaskExecutor};
use astersql_dxf_framework_proto as proto;
use astersql_dxf_framework_storage as storage;
use std::sync::Arc;
use std::sync::mpsc::sync_channel;
use std::time::Duration;
use std::time::SystemTime;

use crate::NewMockCleaner;

#[test]
// 清理回调接收可变任务引用，其修改必须像 Go 指针参数一样保留到调用方。
fn cleanup_mock_preserves_go_mutable_task_pointer_semantics() {
    let mut cleanup = NewMockCleaner(&());
    cleanup.Clean.set(Box::new(|_, task| {
        task.Meta = b"redacted".to_vec();
        Ok(())
    }));

    let mut task = proto::Task {
        TaskBase: proto::TaskBase {
            ID: 0,
            Key: String::new(),
            Type: proto::TaskTypeExample,
            State: proto::TaskStatePending,
            Step: proto::StepInit,
            Priority: proto::NormalPriority,
            RequiredSlots: 0,
            TargetScope: String::new(),
            CreateTime: SystemTime::UNIX_EPOCH,
            MaxNodeCount: 0,
            ExtraParams: proto::ExtraParams::default(),
            Keyspace: String::new(),
        },
        SchedulerID: String::new(),
        StartTime: SystemTime::UNIX_EPOCH,
        StateUpdateTime: SystemTime::UNIX_EPOCH,
        Meta: Vec::new(),
        Error: None,
        ModifyParam: proto::ModifyParam {
            PrevState: proto::TaskStatePending,
            Modifications: Vec::new(),
        },
    };
    cleanup.Clean((), &mut task).unwrap();

    assert_eq!(task.Meta, b"redacted");
    assert_eq!(cleanup.Clean.call_count(), 1);
}

#[test]
// 用户回调执行期间不得持有容器锁，否则回调内重设自身期望会发生死锁。
fn handler_does_not_hold_callback_lock_during_callback() {
    let handler: Arc<Handler<dyn FnMut() + Send>> = Arc::new(Handler::default());
    let reentrant = Arc::clone(&handler);
    handler.set(Box::new(move || {
        reentrant.set(Box::new(|| {}));
    }));

    let (done_tx, done_rx) = sync_channel(1);
    let caller = Arc::clone(&handler);
    std::thread::spawn(move || {
        caller.invoke(
            "handler_does_not_hold_callback_lock_during_callback",
            |callback| callback(),
        );
        done_tx.send(()).unwrap();
    });

    assert!(done_rx.recv_timeout(Duration::from_secs(1)).is_ok());
}

#[test]
// PipelineSpec 必须完整转发 PlanCtx，不能在 mock 边界丢失任务键等规划信息。
fn plan_mock_forwards_plan_context_to_pipeline_handler() {
    let pipeline = NewMockPipelineSpec(&());
    pipeline
        .ToSubtaskMeta
        .set(Box::new(|context| Ok(context.task_key.into_bytes())));

    let result = pipeline.ToSubtaskMeta(astersql_dxf_framework_planner::PlanCtx {
        task_key: "task-1".to_owned(),
        ..Default::default()
    });

    assert_eq!(result.unwrap(), b"task-1");
    assert_eq!(pipeline.ToSubtaskMeta.call_count(), 1);
}

#[test]
// 存储 Manager 替身应把上下文交给处理器，并原样返回处理器给出的结果。
fn storage_manager_mock_forwards_arguments_and_result() {
    let manager = NewMockManager(&());
    manager.GetCPUCountOfNode.set(Box::new(
        |context| if context == () { Ok(8) } else { Ok(0) },
    ));

    assert_eq!(manager.GetCPUCountOfNode(()).unwrap(), 8);
    assert_eq!(manager.GetCPUCountOfNode.call_count(), 1);
}

#[test]
// 调度器的初始化与调度入口分别派发到对应生命周期处理器。
fn scheduler_mock_dispatches_lifecycle_handlers() {
    let scheduler = NewMockScheduler::<(), _>(&());
    scheduler.Init.set(Box::new(|| Ok(())));
    scheduler.ScheduleTask.set(Box::new(|| {}));

    scheduler.Init().unwrap();
    scheduler.ScheduleTask();

    assert_eq!(scheduler.Init.call_count(), 1);
    assert_eq!(scheduler.ScheduleTask.call_count(), 1);
}

#[test]
// 任务执行器的取消入口必须派发到已配置的 Cancel 处理器。
fn task_executor_mock_dispatches_cancel_handler() {
    let executor = NewMockTaskExecutor(&());
    executor.Cancel.set(Box::new(|| {}));

    executor.Cancel();

    assert_eq!(executor.Cancel.call_count(), 1);
}
