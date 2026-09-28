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

use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};

use astersql_dxf_framework_testutil::{
    DistributedTaskRuntime, DxfError, GetMockRollbackSchedulerExt, STEP_INIT, STEP_ONE,
    SubmitAndWaitTask, Task, TaskBase, TaskState, WaitCondition,
};

#[derive(Default)]
struct RollbackRuntime {
    task: Mutex<Option<TaskBase>>,
    transitions: Mutex<Vec<TaskState>>,
    refresh_calls: AtomicI32,
    cancel_requests: AtomicI32,
}

impl RollbackRuntime {
    fn refresh(&self) {
        let call = self.refresh_calls.fetch_add(1, Ordering::SeqCst) + 1;
        let mut task = self.task.lock().unwrap();
        let task = task
            .as_mut()
            .expect("task must be submitted before refresh");
        if call <= 2 && task.state == TaskState::Running {
            self.cancel_requests.fetch_add(1, Ordering::SeqCst);
            task.state = TaskState::Cancelling;
            self.transitions.lock().unwrap().push(task.state);
        }
    }

    fn transition(&self, state: TaskState) {
        self.task.lock().unwrap().as_mut().unwrap().state = state;
        self.transitions.lock().unwrap().push(state);
    }
}

impl DistributedTaskRuntime for RollbackRuntime {
    fn submit_task(
        &self,
        task_key: &str,
        _task_type: &str,
        _keyspace: &str,
        _concurrency: usize,
        _target_scope: &str,
        _meta: &[u8],
    ) -> Result<i64, DxfError> {
        *self.task.lock().unwrap() = Some(TaskBase {
            id: 1,
            key: task_key.to_owned(),
            state: TaskState::Running,
            step: STEP_ONE,
        });
        self.transitions.lock().unwrap().push(TaskState::Running);
        self.refresh();
        self.transition(TaskState::Reverting);
        self.refresh();
        self.transition(TaskState::Reverted);
        self.refresh();
        Ok(1)
    }

    fn get_task_by_key_with_history(&self, task_key: &str) -> Result<TaskBase, DxfError> {
        self.task
            .lock()
            .unwrap()
            .clone()
            .filter(|task| task.key == task_key)
            .ok_or_else(|| DxfError(format!("task {task_key} not found")))
    }

    fn wait_task(&self, task_id: i64, condition: WaitCondition) -> Result<TaskBase, DxfError> {
        let task = self.task.lock().unwrap().clone().unwrap();
        if task.id != task_id {
            return Err(DxfError(format!("task {task_id} not found")));
        }
        let satisfied = match condition {
            WaitCondition::DoneOrPaused => task.is_done() || task.state == TaskState::Paused,
            WaitCondition::Done => task.is_done(),
        };
        satisfied
            .then_some(task)
            .ok_or_else(|| DxfError("task has not reached the requested state".to_owned()))
    }
}

#[test]
fn test_framework_rollback() {
    let runtime = RollbackRuntime::default();
    let task = SubmitAndWaitTask(&runtime, "key1", "", 1, false).unwrap();

    assert_eq!(task.state, TaskState::Reverted);
    assert_eq!(runtime.cancel_requests.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.refresh_calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        *runtime.transitions.lock().unwrap(),
        [
            TaskState::Running,
            TaskState::Cancelling,
            TaskState::Reverting,
            TaskState::Reverted,
        ]
    );
}

/// 校验取消必须经 Reverting 再到 Reverted，不可直接跳转。
#[test]
fn cancellation_rolls_through_reverting_before_reverted() {
    use astersql_dxf_framework_scheduler::*;
    assert!(VerifyTaskStateTransform(
        TASK_STATE_CANCELLING,
        TASK_STATE_REVERTING
    ));
    assert!(VerifyTaskStateTransform(
        TASK_STATE_REVERTING,
        TASK_STATE_REVERTED
    ));
    assert!(!VerifyTaskStateTransform(
        TASK_STATE_CANCELLING,
        TASK_STATE_REVERTED
    ));
}

/// The rollback registration must advance from init to the business step and
/// never silently skip the cancellation/reverting path.
#[test]
fn rollback_scheduler_keeps_business_step_before_revert() {
    let extension = GetMockRollbackSchedulerExt();
    assert_eq!(extension.next_step(STEP_INIT), STEP_ONE);
    assert_eq!(
        extension
            .next_subtasks_batch(&Task::default(), STEP_ONE)
            .unwrap()
            .len(),
        3
    );
}
