// Copyright 2026 AsterSQL.

use super::context::{STEP_INIT, STEP_ONE, Task, TaskBase};
use super::scheduler_util::{GetMockSchedulerExt, SchedulerInfo, StepInfo};

#[test]
#[should_panic(expected = "stepInfos should not be empty")]
fn empty_step_infos_panics_like_go() {
    let _ = GetMockSchedulerExt(SchedulerInfo {
        all_error_retryable: true,
        step_infos: Vec::new(),
    });
}

#[test]
fn negative_error_repeat_count_succeeds_like_go() {
    let scheduler = GetMockSchedulerExt(SchedulerInfo {
        all_error_retryable: false,
        step_infos: vec![StepInfo {
            step: STEP_ONE,
            error: Some(super::context::DxfError("unused".into())),
            error_repeat_count: -1,
            subtask_count: 1,
        }],
    })
    .unwrap();
    let task = Task {
        base: TaskBase {
            step: STEP_INIT,
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(
        scheduler.next_subtasks_batch(&task, STEP_ONE).unwrap(),
        vec![b"subtask-0".to_vec()]
    );
}

#[test]
fn unknown_transition_returns_go_step_zero_value() {
    let scheduler = GetMockSchedulerExt(SchedulerInfo {
        all_error_retryable: true,
        step_infos: vec![StepInfo {
            step: STEP_ONE,
            error: None,
            error_repeat_count: 0,
            subtask_count: 0,
        }],
    })
    .unwrap();

    assert_eq!(
        scheduler.next_step(super::context::Step(99)),
        super::context::Step(0)
    );
}
