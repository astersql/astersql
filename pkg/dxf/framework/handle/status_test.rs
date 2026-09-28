// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 调度状态节点需求估算的回归测试。
//
// 重点覆盖任务按 CPU 槽位共享节点的装箱规则，以及 ImportInto 特定步骤固定单节点的约束。

use super::*;

// 构造节点估算所需的最小任务，其余字段保持中性默认值，避免干扰槽位计算。
fn task(required_slots: i32, max_node_count: i32) -> proto::TaskBase {
    proto::TaskBase {
        ID: 0,
        Key: String::new(),
        Type: "",
        State: proto::TaskStatePending,
        Step: proto::StepInit,
        Priority: proto::NormalPriority,
        RequiredSlots: required_slots,
        TargetScope: String::new(),
        CreateTime: std::time::SystemTime::UNIX_EPOCH,
        MaxNodeCount: max_node_count,
        ExtraParams: proto::ExtraParams::default(),
        Keyspace: String::new(),
    }
}

#[test]
fn test_calculate_required_nodes() {
    // 每个参数对依次表示单任务所需槽位和最大节点数；所有用例均按单节点 8 个槽位计算。
    let cases: &[(&[(i32, i32)], i32)] = &[
        // 无任务及单任务边界。
        (&[], 1),
        (&[(1, 1)], 1),
        (&[(1, 3)], 3),
        (&[(3, 1)], 1),
        (&[(8, 1)], 1),
        (&[(8, 4)], 4),
        // 多个轻量任务可复用同一批节点的剩余槽位。
        (&[(1, 1), (2, 1), (2, 1), (3, 1)], 1),
        (&[(1, 3), (2, 4)], 4),
        (&[(1, 3), (2, 4), (6, 1)], 4),
        (&[(1, 3), (2, 4), (3, 2)], 4),
        (&[(1, 3), (2, 4), (3, 2), (2, 2)], 4),
        (&[(1, 3), (2, 4), (3, 2), (5, 2)], 4),
        (&[(1, 3), (2, 4), (3, 2), (2, 3)], 4),
        (&[(1, 3), (2, 4), (3, 2), (2, 3), (6, 1)], 4),
        (&[(3, 2), (5, 2)], 2),
        (&[(3, 2), (6, 2)], 4),
        // 占满全部槽位的任务无法共享节点，因此各任务的节点数直接累加。
        (&[(8, 4), (8, 6), (8, 5)], 15),
        (&[(8, 4), (8, 6), (8, 5), (8, 20)], 35),
        // 混合场景同时验证任务顺序、槽位复用和独占节点的组合结果。
        (&[(8, 5), (1, 3), (2, 4)], 9),
        (&[(8, 5), (1, 3), (2, 4), (8, 2)], 11),
        (&[(8, 5), (1, 3), (8, 2), (2, 4)], 11),
        (&[(3, 3), (8, 5), (6, 4)], 12),
        (&[(3, 3), (8, 5), (5, 4)], 9),
        (&[(3, 3), (5, 4), (8, 5)], 9),
    ];

    for (index, (params, expected)) in cases.iter().enumerate() {
        let tasks: Vec<_> = params
            .iter()
            .map(|&(slots, nodes)| task(slots, nodes))
            .collect();
        assert_eq!(CalculateRequiredNodes(&tasks, 8), *expected, "case-{index}");
    }
}

#[test]
fn test_get_needed_nodes_for_import_steps() {
    // ImportInto 的冲突处理与后处理步骤固定为单节点，普通导入步骤及其他任务沿用上限。
    let cases = [
        (proto::ImportInto, proto::ImportStepCollectConflicts, 8, 1),
        (proto::ImportInto, proto::ImportStepConflictResolution, 8, 1),
        (proto::ImportInto, proto::ImportStepPostProcess, 8, 1),
        (proto::ImportInto, proto::ImportStepEncodeAndSort, 6, 6),
        (proto::TaskTypeExample, proto::StepOne, 5, 5),
    ];
    for (task_type, step, max_nodes, expected) in cases {
        let mut item = task(1, max_nodes);
        item.Type = task_type;
        item.Step = step;
        assert_eq!(crate::status::getNeededNodes(&item), expected);
    }
}
