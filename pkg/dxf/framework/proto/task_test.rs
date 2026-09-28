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
// 任务协议相关单元测试：PrepareMode、终态、并发旋钮、排名与运行槽位。

// limitations under the License.

use super::*;
use serde_json::{Value, json};
use std::time::{Duration, SystemTime};

/// 构造最小 TaskBase 测试夹具。
fn task_base(state: TaskState) -> TaskBase {
    TaskBase {
        ID: 0,
        Key: String::new(),
        Type: TaskTypeExample,
        State: state,
        Step: StepInit,
        Priority: NormalPriority,
        RequiredSlots: 0,
        TargetScope: String::new(),
        CreateTime: SystemTime::UNIX_EPOCH,
        MaxNodeCount: 0,
        ExtraParams: ExtraParams::default(),
        Keyspace: String::new(),
    }
}

/// 由 TaskBase 填充空 Meta/ModifyParam 得到完整 Task。
fn task_from_base(base: TaskBase) -> Task {
    Task {
        TaskBase: base,
        SchedulerID: String::new(),
        StartTime: SystemTime::UNIX_EPOCH,
        StateUpdateTime: SystemTime::UNIX_EPOCH,
        Meta: vec![],
        Error: None,
        ModifyParam: ModifyParam {
            PrevState: "",
            Modifications: vec![],
        },
    }
}

/// 校验框架 step 常量、PrepareMode 字符串与 ExtraParams JSON 序列化。
#[test]
fn test_task_step() {
    assert_eq!(StepInit, -1);
    assert_eq!(StepDone, -2);
    assert_eq!(PrepareModeDisabled, 0);
    assert_eq!(PrepareModeRequired, 1);
    assert_eq!(PrepareModeDisabled.String(), "disabled");
    assert_eq!(PrepareModeRequired.String(), "required");
    assert_eq!((123 as PrepareMode).String(), "unknown(123)");

    // 默认 ExtraParams 序列化为空对象（零值字段均 skip）。
    let data = serde_json::to_value(ExtraParams::default()).unwrap();
    assert_eq!(data, json!({}));

    let data = serde_json::to_value(ExtraParams {
        ManualRecovery: true,
        ..ExtraParams::default()
    })
    .unwrap();
    assert_eq!(data, json!({"manual_recovery": true}));

    let data = serde_json::to_value(ExtraParams {
        PrepareMode: PrepareModeRequired,
        ..ExtraParams::default()
    })
    .unwrap();
    assert_eq!(data, json!({"prepare_mode": 1}));

    let extra_params: ExtraParams =
        serde_json::from_value(Value::Object(Default::default())).unwrap();
    assert_eq!(extra_params.PrepareMode, PrepareModeDisabled);
    let extra_params: ExtraParams = serde_json::from_value(json!({"prepare_mode": 1})).unwrap();
    assert_eq!(extra_params.PrepareMode, PrepareModeRequired);
}

/// 仅 succeed/failed/reverted 为终态。
#[test]
fn test_task_is_done() {
    let cases = [
        (TaskStatePending, false),
        (TaskStateRunning, false),
        (TaskStateSucceed, true),
        (TaskStateReverting, false),
        (TaskStateFailed, true),
        (TaskStateCancelling, false),
        (TaskStatePausing, false),
        (TaskStatePaused, false),
        (TaskStateReverted, true),
    ];
    for (state, done) in cases {
        assert_eq!(task_from_base(task_base(state)).IsDone(), done);
    }
}

/// 越界拒绝、合法更新与 ForTest 恢复。
#[test]
fn test_max_concurrent_task() {
    let restore = SetMaxConcurrentTaskForTest(DefaultMaxConcurrentTask);
    assert_eq!(GetMaxConcurrentTask(), DefaultMaxConcurrentTask);
    assert_eq!(MaxConcurrentTaskUpperBound, 1000);
    for value in [
        maxConcurrentTaskLowerBound - 1,
        MaxConcurrentTaskUpperBound + 1,
    ] {
        assert!(SetMaxConcurrentTask(value).is_err());
        assert_eq!(GetMaxConcurrentTask(), DefaultMaxConcurrentTask);
    }
    SetMaxConcurrentTask(128).unwrap();
    assert_eq!(GetMaxConcurrentTask(), 128);
    SetMaxConcurrentTask(MaxConcurrentTaskUpperBound).unwrap();
    assert_eq!(GetMaxConcurrentTask(), MaxConcurrentTaskUpperBound);
    restore();
}

/// 排名：优先级 > 创建时间 > ID。
#[test]
fn test_task_compare() {
    let mut base_a = task_base(TaskStatePending);
    base_a.ID = 100;
    base_a.CreateTime = SystemTime::UNIX_EPOCH + Duration::from_secs(1_701_792_810);
    let mut task_b = task_from_base(task_base(TaskStatePending));
    task_b.ID = base_a.ID;
    task_b.Priority = base_a.Priority;
    task_b.CreateTime = base_a.CreateTime;
    assert_eq!(base_a.CompareTask(&task_b), 0);

    // 数值更小的 Priority 排名更高（Compare 返回负值）。
    task_b.Priority = 100;
    assert!(base_a.CompareTask(&task_b) > 0);
    task_b.Priority = base_a.Priority + 100;
    assert!(base_a.CompareTask(&task_b) < 0);

    task_b.Priority = base_a.Priority;
    task_b.CreateTime = base_a.CreateTime - Duration::from_secs(20);
    assert!(base_a.CompareTask(&task_b) > 0);
    task_b.CreateTime = base_a.CreateTime + Duration::from_secs(10);
    assert!(base_a.CompareTask(&task_b) < 0);

    task_b.CreateTime = base_a.CreateTime;
    task_b.ID = base_a.ID - 10;
    assert!(base_a.CompareTask(&task_b) > 0);
    task_b.ID = base_a.ID + 10;
    assert!(base_a.CompareTask(&task_b) < 0);
}

/// MaxRuntimeSlots/TargetSteps 压低运行槽位，以及 LimitDXFResource 比例裁剪。
#[test]
fn test_task_base_get_runtime_slots() {
    let mut task = task_base(TaskStatePending);
    task.RequiredSlots = 4;
    task.Step = StepOne;
    assert_eq!(task.GetRuntimeSlots(), 4);

    // 无 TargetSteps 时所有 step 都受 MaxRuntimeSlots 限制。
    task.ExtraParams.MaxRuntimeSlots = 2;
    for step in [StepOne, StepTwo] {
        task.Step = step;
        assert_eq!(task.GetRuntimeSlots(), 2);
    }
    // 仅 StepOne 受限制；StepTwo 仍用 RequiredSlots。
    task.ExtraParams.TargetSteps = vec![StepOne];
    task.Step = StepOne;
    assert_eq!(task.GetRuntimeSlots(), 2);
    task.Step = StepTwo;
    assert_eq!(task.GetRuntimeSlots(), 4);

    let resource = NewNodeResource(16, 1600, 100);
    let limited = resource.LimitDXFResource(30);
    assert_eq!(limited.TotalCPU, 5);
    assert_eq!(limited.TotalMem, 500);
    assert_eq!(limited.TotalDisk, resource.TotalDisk);

    let full = resource.LimitDXFResource(100);
    assert_eq!(full.TotalCPU, 16);
    assert_eq!(full.TotalMem, 1600);
    assert_eq!(full.TotalDisk, resource.TotalDisk);

    let small = NewNodeResource(2, 200, 100).LimitDXFResource(10);
    assert_eq!(small.TotalCPU, 1);
    assert_eq!(small.TotalMem, 100);
}
