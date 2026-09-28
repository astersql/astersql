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

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use astersql_dxf_framework_testutil::{
    CheckIntervals, DxfError, DxfRuntime, GetMockHATestSchedulerExt, NewDXFContextWithRandomNodes,
    NewTestDXFContext, NodeResource, STEP_INIT, STEP_ONE, STEP_TWO, Subtask, Task, TestContext,
};

#[derive(Default)]
struct HaRuntime {
    cancelled_executors: Mutex<HashSet<String>>,
    cancelled_schedulers: Mutex<HashSet<String>>,
    started_schedulers: Mutex<HashSet<String>>,
    live_executor_ids: Mutex<Vec<String>>,
}

impl DxfRuntime for HaRuntime {
    fn set_node_resource(&self, resource: NodeResource) -> Result<NodeResource, DxfError> {
        Ok(resource)
    }

    fn start_executor(&self, _node_id: &str, _resource: NodeResource) -> Result<(), DxfError> {
        Ok(())
    }

    fn stop_executor(&self, _node_id: &str) -> Result<(), DxfError> {
        Ok(())
    }

    fn cancel_executor(&self, node_id: &str) -> Result<(), DxfError> {
        self.cancelled_executors
            .lock()
            .unwrap()
            .insert(node_id.to_owned());
        Ok(())
    }

    fn start_scheduler(&self, node_id: &str, _resource: NodeResource) -> Result<(), DxfError> {
        self.started_schedulers
            .lock()
            .unwrap()
            .insert(node_id.to_owned());
        Ok(())
    }

    fn stop_scheduler(&self, _node_id: &str) -> Result<(), DxfError> {
        Ok(())
    }

    fn cancel_scheduler(&self, node_id: &str) -> Result<(), DxfError> {
        self.cancelled_schedulers
            .lock()
            .unwrap()
            .insert(node_id.to_owned());
        Ok(())
    }

    fn update_live_executor_ids(&self, node_ids: &[String]) -> Result<(), DxfError> {
        *self.live_executor_ids.lock().unwrap() = node_ids.to_vec();
        Ok(())
    }

    fn set_check_intervals(&self, intervals: CheckIntervals) -> Result<CheckIntervals, DxfError> {
        Ok(intervals)
    }
}

fn submit_task_and_check_success_for_ha(task_id: i64, test_context: &TestContext) {
    let extension = GetMockHATestSchedulerExt();
    let task = Task::default();
    assert_eq!(extension.next_step(STEP_INIT), STEP_ONE);

    for (step, expected_count) in [(STEP_ONE, 10), (STEP_TWO, 5)] {
        let batch = extension.next_subtasks_batch(&task, step).unwrap();
        assert_eq!(batch.len(), expected_count);
        for (index, _) in batch.into_iter().enumerate() {
            test_context.CollectSubtask(&Subtask {
                id: index as i64,
                task_id,
                step,
                ..Default::default()
            });
        }
        assert_eq!(
            test_context.CollectedSubtaskCnt(task_id, step),
            expected_count
        );
    }
    assert_eq!(extension.next_step(STEP_ONE), STEP_TWO);
}

#[test]
fn test_ha_node_random_shutdown() {
    let runtime = Arc::new(HaRuntime::default());
    let context = NewDXFContextWithRandomNodes(runtime.clone(), 4, 15).unwrap();
    let initial_count = context.NodeCount();

    // Go keeps a random count in [1, min(nodeCount-1, 10)]. Choosing the upper
    // boundary exercises the same calculation while keeping the test deterministic.
    let keep_count = (initial_count - 1).min(10);
    let nodes_to_shutdown = context.GetRandNodeIDs(initial_count - keep_count);
    for node_id in &nodes_to_shutdown {
        context.AsyncShutdown(node_id.clone()).unwrap();
    }
    context.WaitAsyncOperations().unwrap();

    assert_eq!(context.NodeCount(), keep_count);
    assert_eq!(
        *runtime.cancelled_executors.lock().unwrap(),
        nodes_to_shutdown
    );
    assert_eq!(runtime.live_executor_ids.lock().unwrap().len(), keep_count);
    submit_task_and_check_success_for_ha(1, &context.test_context());
}

#[test]
fn test_ha_random_shutdown_in_different_step() {
    let runtime = Arc::new(HaRuntime::default());
    let context = NewTestDXFContext(runtime.clone(), 6, 16, true).unwrap();
    let nodes_at_step_one = context.GetRandNodeIDs(context.NodeCount() / 2 - 1);
    let nodes_at_step_two = context.GetRandNodeIDs(context.NodeCount() / 2 - 1);

    for node_id in &nodes_at_step_one {
        context.AsyncShutdown(node_id.clone()).unwrap();
    }
    context.WaitAsyncOperations().unwrap();
    let extension = GetMockHATestSchedulerExt();
    assert_eq!(
        extension
            .next_subtasks_batch(&Task::default(), STEP_ONE)
            .unwrap()
            .len(),
        10
    );

    for node_id in nodes_at_step_two.difference(&nodes_at_step_one) {
        context.AsyncShutdown(node_id.clone()).unwrap();
    }
    context.WaitAsyncOperations().unwrap();
    assert_eq!(
        extension
            .next_subtasks_batch(&Task::default(), STEP_TWO)
            .unwrap()
            .len(),
        5
    );
    assert!(context.NodeCount() >= 2);
    assert_eq!(
        runtime.cancelled_executors.lock().unwrap().len(),
        nodes_at_step_one.union(&nodes_at_step_two).count()
    );
    submit_task_and_check_success_for_ha(2, &context.test_context());
}

#[test]
fn test_ha_multiple_owner() {
    let runtime = Arc::new(HaRuntime::default());
    let context = NewDXFContextWithRandomNodes(runtime.clone(), 4, 8).unwrap();
    let previous_count = context.NodeCount();
    let additional_owner_count = 2;
    for index in 0..additional_owner_count {
        context.ScaleOutBy(format!("tidb-{index}"), true).unwrap();
    }
    assert_eq!(context.NodeCount(), previous_count + additional_owner_count);
    assert_eq!(
        runtime.started_schedulers.lock().unwrap().len(),
        additional_owner_count + 1
    );

    let test_context = context.test_context();
    std::thread::scope(|scope| {
        for task_id in 0..10 {
            let test_context = test_context.clone();
            scope.spawn(move || submit_task_and_check_success_for_ha(task_id, &test_context));
        }
    });
    for task_id in 0..10 {
        assert_eq!(test_context.CollectedSubtaskCnt(task_id, STEP_ONE), 10);
        assert_eq!(test_context.CollectedSubtaskCnt(task_id, STEP_TWO), 5);
    }
}

#[test]
fn owner_failover_state_edges_remain_valid() {
    use astersql_dxf_framework_scheduler::*;

    assert!(VerifyTaskStateTransform(
        TASK_STATE_PENDING,
        TASK_STATE_RUNNING
    ));
    assert!(VerifyTaskStateTransform(
        TASK_STATE_RUNNING,
        TASK_STATE_REVERTING
    ));
    assert!(VerifyTaskStateTransform(
        TASK_STATE_REVERTING,
        TASK_STATE_REVERTED
    ));
    assert!(!VerifyTaskStateTransform(
        TASK_STATE_REVERTED,
        TASK_STATE_RUNNING
    ));
}
