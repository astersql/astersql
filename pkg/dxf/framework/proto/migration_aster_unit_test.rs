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

// Proto 迁移对齐单元测试。
//
// 对照 Go 侧常量与行为，校验 step/type 字符串、任务状态 JSON、节点资源限额、
// Allocatable 并发分配，以及 Subtask / Modification 的构造与展示格式。

use super::modify::*;
use super::node::*;
use super::step::*;
use super::subtask::*;
use super::task::*;
use super::r#type::*;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// 构造默认 ExtraParams（关闭手动恢复与 prepare 模式）。
fn extra_params() -> ExtraParams {
    ExtraParams {
        ManualRecovery: false,
        PauseOnKVDiskFull: false,
        MaxRuntimeSlots: 0,
        TargetSteps: Vec::new(),
        PrepareMode: PrepareModeDisabled,
    }
}

/// 构造最小 TaskBase，供排序与 String 格式断言使用。
fn task_base(id: i64, priority: i32, create_time: SystemTime) -> TaskBase {
    TaskBase {
        ID: id,
        Key: "task-key".to_string(),
        Type: Backfill,
        State: TaskStatePending,
        Step: BackfillStepReadIndex,
        Priority: priority,
        RequiredSlots: 4,
        TargetScope: "background".to_string(),
        CreateTime: create_time,
        MaxNodeCount: 0,
        ExtraParams: extra_params(),
        Keyspace: String::new(),
    }
}

#[test]
/// Step2Str / IsValidStep / Type↔Int 与 Go 常量、未知值格式一致。
fn migration_step_and_type_match_go() {
    assert_eq!(Step2Str(Backfill, StepInit), "init");
    assert_eq!(
        Step2Str(Backfill, BackfillStepMergeTempIndex),
        "merge-temp-index"
    );
    assert_eq!(
        Step2Str(ImportInto, ImportStepConflictResolution),
        "conflict-resolution"
    );
    assert_eq!(Step2Str(TaskTypeExample, 333), "unknown step 333");
    assert_eq!(Step2Str("123", 123), "unknown type 123");
    assert!(IsValidStep(Backfill, StepPrepared));
    assert!(!IsValidBusinessStep(Backfill, StepPrepared));
    assert_eq!(Int2Type(Type2Int(ImportInto)), ImportInto);
    assert_eq!(Int2Type(0), "");
}

#[test]
/// PrepareMode 字符串、ExtraParams JSON omitempty、TaskBase 比较与 runtime slots。
fn migration_task_state_json_and_ranking_match_go() {
    assert_eq!(PrepareModeDisabled.String(), "disabled");
    assert_eq!(123_i32.String(), "unknown(123)");
    assert!(TaskStatePending.CanMoveToModifying());
    assert!(!TaskStateFailed.CanMoveToModifying());

    assert_eq!(serde_json::to_string(&extra_params()).unwrap(), "{}");
    let mut params = extra_params();
    params.ManualRecovery = true;
    assert_eq!(
        serde_json::to_string(&params).unwrap(),
        r#"{"manual_recovery":true}"#
    );
    let decoded: ExtraParams = serde_json::from_str(r#"{"prepare_mode":1}"#).unwrap();
    assert_eq!(decoded.PrepareMode, PrepareModeRequired);

    let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_701_791_610);
    let base = task_base(100, NormalPriority, when);
    let older = task_base(100, NormalPriority, when - Duration::from_secs(20));
    let lower_priority = task_base(100, NormalPriority + 100, when);
    assert!(base.Compare(&older) > 0);
    assert!(base.Compare(&lower_priority) < 0);
    assert!(base.String().contains("create time: 2023-12-05T15:53:30Z"));

    let mut limited = task_base(1, NormalPriority, when);
    limited.ExtraParams.MaxRuntimeSlots = 2;
    assert_eq!(limited.GetRuntimeSlots(), 2);
    limited.ExtraParams.TargetSteps = vec![StepTwo];
    assert_eq!(limited.GetRuntimeSlots(), 4);
}

#[test]
/// LimitDXFResource 按比例限额；Allocatable 并发 Alloc/Free 后 Used==0。
fn migration_node_and_allocatable_match_go() {
    let resource = NewNodeResource(16, 1600, 100);
    let limited = resource.LimitDXFResource(30);
    assert_eq!(
        (limited.TotalCPU, limited.TotalMem, limited.TotalDisk),
        (5, 500, 100)
    );

    let allocatable = Arc::new(NewAllocatable(10_000));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let shared = Arc::clone(&allocatable);
        workers.push(std::thread::spawn(move || {
            for _ in 0..1_000 {
                if shared.Alloc(7) {
                    shared.Free(7);
                }
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(allocatable.Used(), 0);

    let step_resource = StepResource {
        CPU: NewAllocatable(2),
        Mem: NewAllocatable(2 * 1024 * 1024),
    };
    assert_eq!(step_resource.MemoryPerCore(), 1024 * 1024);
    assert_eq!(step_resource.String(), "[CPU=2, Mem=2MiB]");
}

#[test]
/// NewSubtask 初始状态、Modification / ModifyParam 的 String 展示格式。
fn migration_subtask_and_modification_match_go() {
    let subtask = NewSubtask(
        StepOne,
        42,
        TaskTypeExample,
        "node-1".to_string(),
        3,
        vec![1, 2],
        7,
    );
    assert_eq!(subtask.State, "");
    assert_eq!(subtask.TaskID, 42);
    assert_eq!(subtask.Meta, vec![1, 2]);
    assert!(!subtask.IsDone());

    let modification = Modification {
        Type: ModifyRequiredSlots,
        To: 8,
    };
    assert_eq!(modification.String(), "{type: modify_concurrency, to: 8}");
    let param = ModifyParam {
        PrevState: TaskStateRunning,
        Modifications: vec![modification],
    };
    assert_eq!(
        param.String(),
        "{prev_state: running, modifications: [{type: modify_concurrency, to: 8}]}"
    );
}
