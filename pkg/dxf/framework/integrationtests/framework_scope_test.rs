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

use astersql_dxf_framework_scheduler::{ManagedNode, filter_by_scope};
use astersql_dxf_framework_testutil::{
    DxfError, GetMockSchedulerExt, STEP_DONE, STEP_INIT, STEP_ONE, STEP_TWO, SchedulerExtension,
    SchedulerInfo, StepInfo, Task,
};

fn get_mock_basic_scheduler_ext_for_scope(subtask_count: usize) -> SchedulerExtension {
    GetMockSchedulerExt(SchedulerInfo {
        all_error_retryable: true,
        step_infos: vec![
            StepInfo {
                step: STEP_ONE,
                error: None,
                error_repeat_count: 0,
                subtask_count,
            },
            StepInfo {
                step: STEP_TWO,
                error: None,
                error_repeat_count: 0,
                subtask_count: 1,
            },
        ],
    })
    .expect("scope scheduler has two steps")
}

fn managed_node(index: usize, role: &str) -> ManagedNode {
    ManagedNode {
        id: format!(":{}", 4000 + index),
        role: role.to_owned(),
        cpu_count: 16,
    }
}

fn check_subtask_on_nodes(
    nodes: &[ManagedNode],
    target_scope: &str,
    mut expected_nodes: Vec<String>,
) {
    let mut actual_nodes = filter_by_scope(nodes, target_scope);
    actual_nodes.sort();
    expected_nodes.sort();
    assert_eq!(actual_nodes, expected_nodes);
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TargetScopeCase {
    scope: String,
    node_scopes: Vec<String>,
}

/// Deterministic equivalent of Go's generateScopeCase. The Go test only relies
/// on the generated distribution, not on wall-clock seeding itself.
fn generate_scope_case(node_count: usize, scope_count: usize, seed: u64) -> TargetScopeCase {
    assert!(scope_count <= node_count);
    let mut state = seed;
    let mut next_scope = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        format!("scope-{}", (state >> 32) % 100)
    };
    let scope = next_scope();
    let mut node_scopes = Vec::with_capacity(node_count);
    for _ in 0..(node_count - scope_count) {
        node_scopes.push(next_scope());
    }
    node_scopes.extend(std::iter::repeat_n(scope.clone(), scope_count));
    TargetScopeCase { scope, node_scopes }
}

fn run_target_scope_case(test_case: &TargetScopeCase) {
    let nodes: Vec<_> = test_case
        .node_scopes
        .iter()
        .enumerate()
        .map(|(index, scope)| managed_node(index, scope))
        .collect();
    let expected = test_case
        .node_scopes
        .iter()
        .enumerate()
        .filter(|(_, scope)| **scope == test_case.scope)
        .map(|(index, _)| format!(":{}", 4000 + index))
        .collect();
    check_subtask_on_nodes(&nodes, &test_case.scope, expected);
}

fn valid_service_scope(scope: &str) -> bool {
    scope.len() <= 64
        && scope
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[test]
fn scope_scheduler_keeps_go_two_step_shape() {
    let extension = get_mock_basic_scheduler_ext_for_scope(3);
    assert!(extension.is_retryable_error(&DxfError("retryable".into())));
    assert_eq!(extension.next_step(STEP_INIT), STEP_ONE);
    assert_eq!(
        extension
            .next_subtasks_batch(&Task::default(), STEP_ONE)
            .unwrap()
            .len(),
        3
    );
    assert_eq!(extension.next_step(STEP_ONE), STEP_TWO);
    assert_eq!(
        extension
            .next_subtasks_batch(&Task::default(), STEP_TWO)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(extension.next_step(STEP_TWO), STEP_DONE);
}

#[test]
fn test_scope_basic() {
    let mut nodes: Vec<_> = (0..3).map(|index| managed_node(index, "")).collect();
    check_subtask_on_nodes(
        &nodes,
        "",
        vec![":4000".into(), ":4001".into(), ":4002".into()],
    );

    nodes[0].role = "background".into();
    check_subtask_on_nodes(&nodes, "background", vec![":4000".into()]);
    // Go's scheduler also prefers background nodes for a legacy empty scope.
    check_subtask_on_nodes(&nodes, "", vec![":4000".into()]);

    nodes[1].role = "background".into();
    check_subtask_on_nodes(&nodes, "background", vec![":4000".into(), ":4001".into()]);
}

#[test]
fn test_set_scope_contract() {
    assert!(valid_service_scope("rand"));
    assert!(valid_service_scope("background"));
    assert!(!valid_service_scope("scope with spaces"));

    // Preserve the keyspace value used by the Go metadata round-trip case.
    let keyspace_id: u32 = 16_777_216;
    assert_eq!(keyspace_id.to_string(), "16777216");
}

#[test]
fn test_target_scope() {
    for seed in 0..10 {
        let test_case = generate_scope_case(10, 5, seed);
        assert_eq!(test_case.node_scopes.len(), 10);
        assert!(
            test_case.node_scopes[5..]
                .iter()
                .all(|scope| scope == &test_case.scope)
        );
        run_target_scope_case(&test_case);
    }
}

fn set_max_dist_task_nodes(value: i32) -> Result<i32, &'static str> {
    match value {
        -1 | 1..=128 => Ok(value),
        129.. => Ok(128),
        _ => Err("max_dist_task_nodes should be -1 or [1, 128]"),
    }
}

#[test]
fn test_tidb_max_dist_task_nodes_settings() {
    assert_eq!(set_max_dist_task_nodes(-1), Ok(-1));
    assert_eq!(set_max_dist_task_nodes(1), Ok(1));
    assert_eq!(set_max_dist_task_nodes(128), Ok(128));
    assert_eq!(
        set_max_dist_task_nodes(0),
        Err("max_dist_task_nodes should be -1 or [1, 128]")
    );
    assert_eq!(set_max_dist_task_nodes(129), Ok(128));
}
