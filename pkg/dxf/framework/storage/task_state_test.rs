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

// 任务状态流转写路径的单元测试。
//
// 覆盖取消/失败/回滚/成功、PauseTaskOnError 的 AffectedRows 竞争、
// ExtraParams 更新、ModifyTask 状态校验，以及 pause/resume 状态序列。

use crate::*;

#[test]
/// 串联 Cancel/Fail/Revert/Reverted/Succeed，校验目标 state 与 SQL 参数。
fn TestTaskState() {
    let manager = TaskManager::new();
    manager.CancelTask((), 9).unwrap();
    manager
        .FailTask((), 9, proto::TaskStateRunning, Error::new("boom"))
        .unwrap();
    manager
        .RevertTask((), 9, proto::TaskStateFailed, Error::new("retry"))
        .unwrap();
    manager.RevertedTask((), 9).unwrap();
    manager.SucceedTask((), 10).unwrap();
    let calls = manager.calls();
    assert_eq!(calls.len(), 5);
    assert_eq!(
        calls[0].args[0],
        Value::String(proto::TaskStateCancelling.into())
    );
    assert_eq!(
        calls[1].args[0],
        Value::String(proto::TaskStateFailed.into())
    );
    assert_eq!(
        calls[2].args[0],
        Value::String(proto::TaskStateReverting.into())
    );
    assert!(calls[4].sql.contains("step = %?"));
    assert_eq!(calls[4].args[1], Value::Int(proto::StepDone));
}

#[test]
/// Go 的 error 接口允许 nil；回滚/失败决议状态写路径应保留这一公开契约。
fn task_state_error_transitions_accept_nil_error() {
    let manager = TaskManager::new();

    manager
        .RevertTask((), 9, proto::TaskStatePending, None)
        .unwrap();
    manager
        .AwaitingResolveTask((), 10, proto::TaskStateRunning, None)
        .unwrap();
    manager
        .FailTask((), 11, proto::TaskStatePending, None)
        .unwrap();
    manager.set_affected_rows(1);
    manager
        .PauseTaskOnError((), 12, proto::TaskStateRunning, proto::StepOne, None)
        .unwrap();

    for call in manager
        .calls()
        .into_iter()
        .filter(|call| call.sql.contains("error = %?"))
    {
        assert_eq!(call.args[1], Value::Bytes(Vec::new()));
    }
}

#[test]
/// AffectedRows=0 应返回 ErrTaskChanged；成功路径应清 subtask end_time。
fn TestPauseTaskOnError() {
    let manager = TaskManager::new();
    manager.set_affected_rows(0);
    assert_eq!(
        manager.PauseTaskOnError(
            (),
            7,
            proto::TaskStateRunning,
            proto::StepOne,
            Error::new("boom"),
        ),
        Err(ErrTaskChanged.into())
    );
    assert_eq!(manager.calls().len(), 1);

    let manager = TaskManager::new();
    manager.set_affected_rows(1);
    manager
        .PauseTaskOnError(
            (),
            7,
            proto::TaskStateRunning,
            proto::StepOne,
            Error::new("boom"),
        )
        .unwrap();
    let calls = manager.calls();
    assert_eq!(calls.len(), 2);
    assert!(calls[1].sql.contains("end_time = null"));
    assert_eq!(
        calls[1].args[3],
        Value::String(proto::SubtaskStateFailed.into())
    );
}

#[test]
/// 校验 ExtraParams（如 ManualRecovery、MaxRuntimeSlots）序列化为 JSON 写入。
fn TestUpdateTaskExtraParams() {
    let manager = TaskManager::new();
    manager
        .UpdateTaskExtraParams(
            (),
            12,
            proto::ExtraParams {
                ManualRecovery: true,
                PauseOnKVDiskFull: false,
                MaxRuntimeSlots: 5,
                TargetSteps: vec![2, 3],
                PrepareMode: 0,
            },
        )
        .unwrap();
    let calls = manager.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].args[1], Value::Int(12));
    match &calls[0].args[0] {
        Value::Json(value) => {
            assert!(value.contains("manual_recovery"));
            assert!(value.contains("max_runtime_slots"));
        }
        other => panic!("expected JSON, got {other:?}"),
    }
}

#[test]
/// 非法 PrevState 拒绝；合法路径写入 modify_params。
fn TestModifyTask() {
    let manager = TaskManager::new();
    let invalid = proto::ModifyParam {
        PrevState: proto::TaskStateFailed,
        Modifications: Vec::new(),
    };
    assert_eq!(
        manager.ModifyTaskByID((), 1, invalid),
        Err(ErrTaskStateNotAllow.into())
    );
    assert!(manager.calls().is_empty());

    manager.set_task_state(proto::TaskStateRunning);
    manager.set_affected_rows(1);
    manager
        .ModifyTaskByID(
            (),
            1,
            proto::ModifyParam {
                PrevState: proto::TaskStateRunning,
                Modifications: vec![proto::Modification {
                    Type: proto::ModifyRequiredSlots,
                    To: 6,
                }],
            },
        )
        .unwrap();
    let calls = manager.calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].sql.contains("state = %?, modify_params = %?"));
}

#[test]
/// 校验 Pause→Paused→Resume→Resumed 四步状态参数顺序。
fn TestPauseAndResume() {
    let manager = TaskManager::new();
    manager.set_affected_rows(1);
    assert!(manager.PauseTask((), "task-1".into()).unwrap());
    manager.PausedTask((), 1).unwrap();
    assert!(manager.ResumeTask((), "task-1".into()).unwrap());
    manager.ResumedTask((), 1).unwrap();
    let calls = manager.calls();
    assert_eq!(calls.len(), 4);
    assert_eq!(
        calls[0].args[0],
        Value::String(proto::TaskStatePausing.into())
    );
    assert_eq!(
        calls[2].args[0],
        Value::String(proto::TaskStateResuming.into())
    );
    assert_eq!(
        calls[3].args[0],
        Value::String(proto::TaskStateRunning.into())
    );
}

#[test]
fn cancellation_error_recognizes_only_user_marker() {
    assert!(!IsCancelledErr(None));
    for (message, expected) in [
        ("some err", false),
        ("context canceled", false),
        ("cancelled by user", true),
        ("wrapped: cancelled by user: details", true),
    ] {
        assert_eq!(IsCancelledErr(Some(&Error::new(message))), expected);
    }
}
