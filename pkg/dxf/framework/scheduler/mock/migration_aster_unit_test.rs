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

// MockExtension 行为契约测试（对齐 GoMock 期望语义）。
//
// 验证构造标记、标量返回、meta/批量子任务回调、OnPrepare 可变任务
// 以及 OnTick 分发等路径。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use anyhow::anyhow;

use crate::proto::{Modification, Task, TaskBase};
use crate::scheduler_mock::NewMockExtension;

#[derive(Default)]
/// 测试用 TaskHandle：会话回调直接执行，previous meta 返回固定字节。
struct TestHandle;

impl storage_crate::SessionExecutor for TestHandle {
    fn WithNewSession<F>(&self, callback: F) -> Result<(), storage_crate::Error>
    where
        F: FnOnce(storage_crate::sessionctx::Context) -> Result<(), storage_crate::Error>,
    {
        callback(storage_crate::sessionctx::Context::default())
    }

    fn WithNewTxn<F>(
        &self,
        _context: storage_crate::Context,
        callback: F,
    ) -> Result<(), storage_crate::Error>
    where
        F: FnOnce(storage_crate::sessionctx::Context) -> Result<(), storage_crate::Error>,
    {
        callback(storage_crate::sessionctx::Context::default())
    }
}

impl storage_crate::TaskHandle for TestHandle {
    fn GetPreviousSubtaskMetas(
        &self,
        task_id: i64,
        step: storage_crate::proto::Step,
    ) -> Result<Vec<Vec<u8>>, storage_crate::Error> {
        Ok(vec![format!("{task_id}:{step}").into_bytes()])
    }

    fn GetPreviousSubtaskSummary(
        &self,
        _task_id: i64,
        _step: storage_crate::proto::Step,
    ) -> Result<Vec<storage_crate::execute::SubtaskSummary>, storage_crate::Error> {
        Ok(Vec::new())
    }
}

/// 构造带默认字段的 TaskBase 测试夹具。
fn task_base(id: i64, step: i64) -> TaskBase {
    TaskBase {
        ID: id,
        Key: format!("task-{id}"),
        Type: proto_crate::r#type::TaskTypeExample,
        State: proto_crate::task::TaskStatePending,
        Step: step,
        Priority: proto_crate::task::NormalPriority,
        RequiredSlots: 4,
        TargetScope: "background".to_owned(),
        CreateTime: SystemTime::UNIX_EPOCH,
        MaxNodeCount: 2,
        ExtraParams: proto_crate::task::ExtraParams::default(),
        Keyspace: "ks".to_owned(),
    }
}

/// 构造完整 Task 测试夹具。
fn task(id: i64, step: i64) -> Task {
    Task {
        TaskBase: task_base(id, step),
        SchedulerID: String::new(),
        StartTime: SystemTime::UNIX_EPOCH,
        StateUpdateTime: SystemTime::UNIX_EPOCH,
        Meta: b"before".to_vec(),
        Error: None,
        ModifyParam: proto_crate::modify::ModifyParam {
            PrevState: proto_crate::task::TaskStatePending,
            Modifications: Vec::new(),
        },
    }
}

/// 验证 ISGOMOCK/EXPECT 标记，以及 GetEligibleInstances/GetNextStep/
/// IsRetryableErr 的期望匹配与返回值。
#[test]
fn constructor_marker_and_scalar_results_follow_gomock_contract() {
    let mut extension = NewMockExtension::<TestHandle, _>(&());
    assert_eq!(extension.ISGOMOCK(), ());
    assert!(std::ptr::eq(extension.EXPECT(), &extension));

    extension
        .expect_GetEligibleInstances()
        .withf(|_, task| task.ID == 41)
        .times(1)
        .returning(|_, _| Ok(vec!["node-1".to_owned(), "node-2".to_owned()]));
    extension
        .expect_GetNextStep()
        .withf(|task| task.Step == proto_crate::step::StepInit)
        .times(1)
        .return_const(proto_crate::step::StepOne);
    extension
        .expect_IsRetryableErr()
        .withf(|error| error.to_string() == "temporary")
        .times(1)
        .return_const(true);

    let task = task(41, proto_crate::step::StepInit);
    assert_eq!(
        extension.GetEligibleInstances((), &task).unwrap(),
        ["node-1", "node-2"]
    );
    assert_eq!(
        extension.GetNextStep(&task.TaskBase),
        proto_crate::step::StepOne
    );
    assert!(extension.IsRetryableErr(anyhow!("temporary")));
    extension.checkpoint();
}

/// 验证 ModifyMeta、OnNextSubtasksBatch 参数转发，以及 OnDone 错误传播。
#[test]
fn metadata_and_batch_callbacks_forward_arguments_and_errors() {
    let mut extension = NewMockExtension::<TestHandle, _>(&());
    extension
        .expect_ModifyMeta()
        .withf(|meta, modifications| {
            meta == b"old"
                && modifications.len() == 1
                && modifications[0].Type == proto_crate::modify::ModifyRequiredSlots
                && modifications[0].To == 8
        })
        .times(1)
        .returning(|mut meta, _| {
            meta.extend_from_slice(b"-new");
            Ok(meta)
        });
    extension
        .expect_OnNextSubtasksBatch()
        .withf(|_, _, task, nodes, step| {
            task.ID == 42
                && nodes.as_slice() == ["node-a", "node-b"]
                && *step == proto_crate::step::StepOne
        })
        .times(1)
        .returning(|_, _, _, _, _| Ok(vec![b"meta-a".to_vec(), b"meta-b".to_vec()]));
    extension
        .expect_OnDone()
        .withf(|_, _, task| task.ID == 42)
        .times(1)
        .returning(|_, _, _| Err(anyhow!("cleanup failed")));

    let modified = extension
        .ModifyMeta(
            b"old".to_vec(),
            vec![Modification {
                Type: proto_crate::modify::ModifyRequiredSlots,
                To: 8,
            }],
        )
        .unwrap();
    assert_eq!(modified, b"old-new");

    let handle = TestHandle;
    let task = task(42, proto_crate::step::StepInit);
    let metas = extension
        .OnNextSubtasksBatch(
            (),
            &handle,
            &task,
            vec!["node-a".to_owned(), "node-b".to_owned()],
            proto_crate::step::StepOne,
        )
        .unwrap();
    assert_eq!(metas, [b"meta-a".to_vec(), b"meta-b".to_vec()]);
    assert_eq!(
        extension
            .OnDone((), &handle, &task)
            .unwrap_err()
            .to_string(),
        "cleanup failed"
    );
    extension.checkpoint();
}

/// 验证 OnPrepare 可就地修改 task，且 OnTick 按 times 期望被调用。
#[test]
fn prepare_can_mutate_task_and_tick_is_dispatched() {
    let mut extension = NewMockExtension::<TestHandle, _>(&());
    extension
        .expect_OnPrepare()
        .withf(|_, _, task| task.ID == 43 && task.Meta == b"before")
        .times(1)
        .returning(|_, _, task| {
            task.Meta = b"prepared".to_vec();
            task.RequiredSlots = 6;
            task.MaxNodeCount = 3;
            Ok(())
        });

    let ticks = Arc::new(AtomicUsize::new(0));
    let tick_count = Arc::clone(&ticks);
    extension
        .expect_OnTick()
        .withf(|_, task| task.ID == 43)
        .times(2)
        .returning(move |_, _| {
            tick_count.fetch_add(1, Ordering::SeqCst);
        });

    let handle = TestHandle;
    let mut task = task(43, proto_crate::step::StepInit);
    extension.OnPrepare((), &handle, &mut task).unwrap();
    assert_eq!(task.Meta, b"prepared");
    assert_eq!(task.RequiredSlots, 6);
    assert_eq!(task.MaxNodeCount, 3);

    extension.OnTick((), &task);
    extension.OnTick((), &task);
    assert_eq!(ticks.load(Ordering::SeqCst), 2);
    extension.checkpoint();
}
