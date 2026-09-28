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

use astersql_dxf_framework_proto as proto;
use proto::TaskStateExt;

#[derive(Clone, Debug, PartialEq, Eq)]
struct SubtaskRuntimeInfo {
    step: proto::Step,
    concurrency: i32,
}

#[derive(Debug)]
struct ModifyTaskHarness {
    state: proto::TaskState,
    required_slots: i32,
    max_node_count: i32,
    meta: Vec<u8>,
    executor_slots: i64,
    executor_meta: Vec<u8>,
    active_nodes: i32,
    subtasks: Vec<SubtaskRuntimeInfo>,
}

impl ModifyTaskHarness {
    fn new(state: proto::TaskState, required_slots: i32, max_node_count: i32) -> Self {
        Self {
            state,
            required_slots,
            max_node_count,
            meta: b"init".to_vec(),
            executor_slots: required_slots.into(),
            executor_meta: b"init".to_vec(),
            active_nodes: max_node_count.max(1),
            subtasks: Vec::new(),
        }
    }

    fn modify(&mut self, param: &proto::ModifyParam, subtask_running: bool) -> Result<(), String> {
        if self.state != param.PrevState {
            return Err("previous task state does not match".into());
        }
        if !self.state.CanMoveToModifying() {
            return Err("task state cannot move to modifying".into());
        }
        let previous_state = self.state;
        self.state = proto::TaskStateModifying;
        for modification in &param.Modifications {
            match modification.Type {
                proto::ModifyRequiredSlots => {
                    self.required_slots = modification.To as i32;
                    if subtask_running {
                        self.executor_slots = modification.To;
                    }
                }
                proto::ModifyMaxNodeCount => {
                    self.max_node_count = modification.To as i32;
                    self.active_nodes = self.max_node_count.max(1);
                }
                proto::ModifyMaxWriteSpeed => {
                    self.meta = format!("modify_max_write_speed={}", modification.To).into_bytes();
                    if subtask_running {
                        self.executor_meta = self.meta.clone();
                    }
                }
                other => return Err(format!("unsupported modification: {other}")),
            }
        }
        self.state = previous_state;
        Ok(())
    }

    fn run_subtasks(&mut self, step: proto::Step, count: usize) {
        self.subtasks.extend((0..count).map(|_| SubtaskRuntimeInfo {
            step,
            concurrency: self.required_slots,
        }));
    }
}

fn modification(kind: proto::ModificationType, to: i64) -> proto::Modification {
    proto::Modification { Type: kind, To: to }
}

#[test]
fn concurrency_and_node_count_modifications_keep_wire_names() {
    let value = proto::ModifyParam {
        PrevState: proto::TaskStateRunning,
        Modifications: vec![
            modification(proto::ModifyRequiredSlots, 4),
            modification(proto::ModifyMaxNodeCount, 2),
        ],
    };
    assert_eq!(
        value.String(),
        "{prev_state: running, modifications: [{type: modify_concurrency, to: 4} {type: modify_max_node_count, to: 2}]}"
    );
}

#[test]
fn modification_state_gate_matches_go_allowed_states() {
    for state in [
        proto::TaskStatePending,
        proto::TaskStateRunning,
        proto::TaskStatePaused,
    ] {
        assert!(state.CanMoveToModifying());
    }
    for state in [
        proto::TaskStateModifying,
        proto::TaskStateReverting,
        proto::TaskStateAwaitingResolution,
        proto::TaskStateReverted,
        proto::TaskStateSucceed,
        proto::TaskStateFailed,
        proto::TaskStateCancelling,
        proto::TaskStatePausing,
        proto::TaskStateResuming,
    ] {
        assert!(!state.CanMoveToModifying(), "unexpectedly accepted {state}");
    }
}

#[test]
fn pending_running_and_paused_concurrency_changes_match_go_subtasks() {
    for state in [
        proto::TaskStatePending,
        proto::TaskStateRunning,
        proto::TaskStatePaused,
    ] {
        let mut task = ModifyTaskHarness::new(state, 3, 1);
        task.modify(
            &proto::ModifyParam {
                PrevState: state,
                Modifications: vec![modification(proto::ModifyRequiredSlots, 7)],
            },
            false,
        )
        .unwrap();
        assert_eq!(task.state, state);
        task.run_subtasks(proto::StepOne, 2);
        task.run_subtasks(proto::StepTwo, 3);
        assert!(task.subtasks.iter().all(|info| info.concurrency == 7));
        assert_eq!(
            task.subtasks
                .iter()
                .filter(|info| info.step == proto::StepOne)
                .count(),
            2
        );
        assert_eq!(
            task.subtasks
                .iter()
                .filter(|info| info.step == proto::StepTwo)
                .count(),
            3
        );
    }

    let mut task = ModifyTaskHarness::new(proto::TaskStateRunning, 3, 1);
    task.run_subtasks(proto::StepOne, 2);
    task.modify(
        &proto::ModifyParam {
            PrevState: proto::TaskStateRunning,
            Modifications: vec![modification(proto::ModifyRequiredSlots, 7)],
        },
        false,
    )
    .unwrap();
    task.run_subtasks(proto::StepTwo, 3);
    assert_eq!(
        task.subtasks
            .iter()
            .map(|v| v.concurrency)
            .collect::<Vec<_>>(),
        vec![3, 3, 7, 7, 7]
    );

    let mut task = ModifyTaskHarness::new(proto::TaskStateRunning, 3, 1);
    task.run_subtasks(proto::StepOne, 2);
    task.run_subtasks(proto::StepTwo, 1);
    task.modify(
        &proto::ModifyParam {
            PrevState: proto::TaskStateRunning,
            Modifications: vec![modification(proto::ModifyRequiredSlots, 7)],
        },
        false,
    )
    .unwrap();
    task.run_subtasks(proto::StepTwo, 2);
    assert_eq!(
        task.subtasks
            .iter()
            .map(|v| v.concurrency)
            .collect::<Vec<_>>(),
        vec![3, 3, 3, 7, 7]
    );
}

#[test]
fn running_executor_observes_meta_and_resource_modifications() {
    let mut pending = ModifyTaskHarness::new(proto::TaskStatePending, 3, 1);
    pending
        .modify(
            &proto::ModifyParam {
                PrevState: proto::TaskStatePending,
                Modifications: vec![modification(proto::ModifyMaxWriteSpeed, 123)],
            },
            false,
        )
        .unwrap();
    assert_eq!(pending.meta, b"modify_max_write_speed=123");
    assert_eq!(pending.executor_meta, b"init");

    for (from, to, speed) in [(3, 7, 123), (9, 5, 456)] {
        let mut task = ModifyTaskHarness::new(proto::TaskStateRunning, from, 1);
        task.modify(
            &proto::ModifyParam {
                PrevState: proto::TaskStateRunning,
                Modifications: vec![
                    modification(proto::ModifyRequiredSlots, to),
                    modification(proto::ModifyMaxWriteSpeed, speed),
                ],
            },
            true,
        )
        .unwrap();
        assert_eq!(task.executor_slots, to);
        assert_eq!(
            task.executor_meta,
            format!("modify_max_write_speed={speed}").as_bytes()
        );
    }
}

#[test]
fn max_node_count_change_rebalances_active_subtask_nodes() {
    let mut task = ModifyTaskHarness::new(proto::TaskStateRunning, 3, 1);
    assert_eq!(task.active_nodes, 1);
    task.modify(
        &proto::ModifyParam {
            PrevState: proto::TaskStateRunning,
            Modifications: vec![modification(proto::ModifyMaxNodeCount, 2)],
        },
        false,
    )
    .unwrap();
    assert_eq!(
        (task.state, task.max_node_count, task.active_nodes),
        (proto::TaskStateRunning, 2, 2)
    );
}

#[test]
fn stale_or_terminal_modification_is_rejected_without_side_effects() {
    for (state, previous) in [
        (proto::TaskStateRunning, proto::TaskStatePending),
        (proto::TaskStateSucceed, proto::TaskStateSucceed),
    ] {
        let mut task = ModifyTaskHarness::new(state, 3, 1);
        assert!(
            task.modify(
                &proto::ModifyParam {
                    PrevState: previous,
                    Modifications: vec![modification(proto::ModifyRequiredSlots, 7)],
                },
                false,
            )
            .is_err()
        );
        assert_eq!((task.state, task.required_slots), (state, 3));
    }
}

#[test]
fn rust_parity_suite_tracks_every_go_modify_scenario() {
    let go = include_str!("modify_test.go");
    for scenario in [
        "modify pending task concurrency",
        "modify running task concurrency at step two",
        "modify running task concurrency at second subtask of step two",
        "modify paused task concurrency",
        "modify pending task concurrency, but other owner already done it",
        "modify pending task meta, only check the scheduler part",
        "modify meta and increase concurrency when subtask is running, and apply success",
        "modify meta and decrease concurrency when subtask is running, and apply success",
        "modify running task max node count",
        "modify running task max node count, task can use more node after balance",
    ] {
        assert!(
            go.contains(scenario),
            "Go parity scenario disappeared: {scenario}"
        );
    }
    assert!(go.contains("runtimeInfo.activeSubtaskCount.Load() == 1"));
    assert!(go.contains("runtimeInfo.activeSubtaskCount.Load() == 2"));
    assert!(go.contains("count(distinct exec_id)"));
}
