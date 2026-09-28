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

// MockStepExecutor 迁移回归与行为测试。
//
// 覆盖生命周期（Init/Cleanup）期望派发、子任务元数据突变、任务 meta/资源变更回调，
// 以及框架访问器（Step/Resource/Meter/Checkpoint/Summary）的可 mock 性。
// limitations under the License.

#![allow(non_snake_case)]

use crate::NewMockStepExecutor;
use anyhow::anyhow;
use execute::{Context, SetFrameworkInfo, StepExecFrameworkInfo, StepExecutor, SubtaskSummary};
use proto::step::{StepOne, StepTwo};
use proto::subtask::{NewAllocatable, NewSubtask, StepResource};
use proto::task::{ExtraParams, Task, TaskBase, TaskStatePending};
use proto::r#type::TaskTypeExample;
use std::any::Any;
use std::sync::Arc;
use std::time::SystemTime;

/// Init 成功、Cleanup 按期望返回错误；校验 EXPECT/ISGOMOCK 与参数未取消。
#[test]
fn lifecycle_expectations_dispatch_arguments_and_errors() {
    let mut executor = NewMockStepExecutor();
    assert_eq!(executor.ISGOMOCK(), ());
    assert!(std::ptr::eq(executor.EXPECT(), &mut executor));

    // Init：上下文未取消时返回 Ok。
    executor
        .expect_Init()
        .withf(|ctx| !ctx.is_cancelled())
        .times(1)
        .returning(|_| Ok(()));
    // Cleanup：强制返回错误，验证错误字符串透传。
    executor
        .expect_Cleanup()
        .times(1)
        .returning(|_| Err(anyhow!("cleanup failed")));

    let ctx = Context::new();
    StepExecutor::Init(&mut executor, ctx.clone()).unwrap();
    let err = StepExecutor::Cleanup(&mut executor, ctx).unwrap_err();
    assert_eq!(err.to_string(), "cleanup failed");
}

/// RunSubtask 可原地改 Meta；TaskMetaModified / ResourceModified 按期望匹配参数。
#[test]
fn subtask_and_modification_callbacks_preserve_mutation_and_values() {
    let mut executor = NewMockStepExecutor();
    // 子任务执行：将 Meta 从 before 改为 after。
    executor
        .expect_RunSubtask()
        .withf(|ctx, subtask| !ctx.is_cancelled() && subtask.Meta == b"before")
        .times(1)
        .returning(|_, subtask| {
            subtask.Meta = b"after".to_vec();
            Ok(())
        });
    // 任务元数据变更回调。
    executor
        .expect_TaskMetaModified()
        .withf(|ctx, meta| !ctx.is_cancelled() && meta == b"task-meta")
        .times(1)
        .returning(|_, _| Ok(()));
    // 步骤资源（CPU/Mem）变更回调；Capacity 表示可分配容量。
    executor
        .expect_ResourceModified()
        .withf(|ctx, resource| !ctx.is_cancelled() && resource.CPU.Capacity() == 8)
        .times(1)
        .returning(|_, _| Ok(()));

    let ctx = Context::new();
    let mut subtask = NewSubtask(
        StepOne,
        42,
        TaskTypeExample,
        "executor-1".to_string(),
        4,
        b"before".to_vec(),
        1,
    );
    StepExecutor::RunSubtask(&mut executor, ctx.clone(), &mut subtask).unwrap();
    assert_eq!(subtask.Meta, b"after");

    StepExecutor::TaskMetaModified(&mut executor, ctx.clone(), b"task-meta".to_vec()).unwrap();
    let resource = StepResource {
        CPU: NewAllocatable(8),
        Mem: NewAllocatable(4096),
    };
    StepExecutor::ResourceModified(&mut executor, ctx, &resource).unwrap();
}

/// 框架信息访问器、检查点读写回调与实时摘要均可按期望返回。
/// Checkpoint 用于子任务断点续跑；RealtimeSummary 汇报当前进度。
#[test]
fn framework_accessors_callbacks_and_summary_are_mockable() {
    let mut executor = NewMockStepExecutor();
    let resource = Arc::new(StepResource {
        CPU: NewAllocatable(6),
        Mem: NewAllocatable(8192),
    });
    let meter = Arc::new(metering::Recorder::new(7, "ks", "example"));

    // StepExecFrameworkInfo 访问器期望。
    executor.expect_restricted().times(1).return_const(());
    executor.expect_GetStep().times(1).return_const(StepTwo);
    executor.expect_GetResource().times(1).return_once({
        let resource = Arc::clone(&resource);
        move || Some(resource)
    });
    executor
        .expect_SetResource()
        .withf(|resource| resource.CPU.Capacity() == 6)
        .times(1)
        .return_const(());
    executor.expect_GetMeterRecorder().times(1).return_once({
        let meter = Arc::clone(&meter);
        move || Some(meter)
    });
    // 检查点：读函数按 subtask_id 返回字符串。
    executor
        .expect_GetCheckpointFunc()
        .times(1)
        .return_once(|| {
            Some(Arc::new(|_ctx: Context, subtask_id: i64| {
                Ok(format!("checkpoint-{subtask_id}"))
            }))
        });
    // 检查点：写函数校验 subtask_id 与装箱值。
    executor
        .expect_GetCheckpointUpdateFunc()
        .times(1)
        .return_once(|| {
            Some(Arc::new(
                |_ctx: Context, subtask_id: i64, value: Box<dyn Any + Send + Sync>| {
                    assert_eq!(subtask_id, 9);
                    assert_eq!(*value.downcast::<u64>().unwrap(), 88);
                    Ok(())
                },
            ))
        });
    // 泄漏静态摘要供 RealtimeSummary 返回引用。
    let summary: &'static SubtaskSummary = Box::leak(Box::new(SubtaskSummary::default()));
    executor
        .expect_RealtimeSummary()
        .times(1)
        .return_const(Some(summary));
    executor.expect_ResetSummary().times(1).return_const(());

    // 实际调用并断言返回值与副作用。
    executor.restricted();
    assert_eq!(executor.GetStep(), StepTwo);
    assert!(Arc::ptr_eq(&executor.GetResource().unwrap(), &resource));
    executor.SetResource(Arc::clone(&resource));
    assert!(Arc::ptr_eq(&executor.GetMeterRecorder().unwrap(), &meter));

    let checkpoint = executor.GetCheckpointFunc().unwrap();
    assert_eq!(checkpoint(Context::new(), 9).unwrap(), "checkpoint-9");
    let update = executor.GetCheckpointUpdateFunc().unwrap();
    update(Context::new(), 9, Box::new(88_u64)).unwrap();
    assert_eq!(executor.RealtimeSummary().unwrap().Progresses.len(), 0);
    executor.ResetSummary();
}

/// Rust 用显式方法承接 Go embedding/reflection 注入；所有框架字段都必须原样传给 mock。
#[test]
fn framework_info_injection_dispatches_complete_go_equivalent_state() {
    let mut executor = NewMockStepExecutor();
    let resource = Arc::new(StepResource {
        CPU: NewAllocatable(12),
        Mem: NewAllocatable(16384),
    });
    executor
        .expect_SetFrameworkInfo()
        .withf(|info| {
            info.GetStep() == StepTwo
                && info
                    .GetResource()
                    .is_some_and(|resource| resource.CPU.Capacity() == 12)
                && info.GetMeterRecorder().is_some()
                && info.GetCheckpointUpdateFunc().is_some()
                && info.GetCheckpointFunc().is_some()
        })
        .times(1)
        .return_const(());

    let task = Task {
        TaskBase: TaskBase {
            ID: 7,
            Key: "mock-framework-info".to_string(),
            Type: TaskTypeExample,
            State: TaskStatePending,
            Step: StepTwo,
            Priority: 512,
            RequiredSlots: 12,
            TargetScope: "background".to_string(),
            CreateTime: SystemTime::UNIX_EPOCH,
            MaxNodeCount: 1,
            ExtraParams: ExtraParams::default(),
            Keyspace: "ks".to_string(),
        },
        SchedulerID: String::new(),
        StartTime: SystemTime::UNIX_EPOCH,
        StateUpdateTime: SystemTime::UNIX_EPOCH,
        Meta: Vec::new(),
        Error: None,
        ModifyParam: proto::modify::ModifyParam {
            PrevState: TaskStatePending,
            Modifications: Vec::new(),
        },
    };
    let update =
        Arc::new(|_ctx: Context, _subtask_id: i64, _value: Box<dyn Any + Send + Sync>| Ok(()));
    let get = Arc::new(|_ctx: Context, subtask_id: i64| Ok(format!("checkpoint-{subtask_id}")));

    SetFrameworkInfo(
        Some(&mut executor),
        &task,
        resource,
        Some(update),
        Some(get),
    );
}
