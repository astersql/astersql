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

use astersql_dxf_framework_scheduler::filter_nodes_with_enough_slots;
use std::collections::{HashMap, HashSet};

const NODE_CAPACITY: i32 = 16;

#[derive(Clone, Copy)]
struct TaskDemand {
    slots: i32,
    remaining: usize,
    step: usize,
}

/// Deterministic view of the resource-controller contract exercised by the Go
/// integration test. Tasks are ranked by their monotonically increasing ID.
/// A task can place at most one subtask on a node, while different tasks may
/// share a node when its remaining slots permit it.
#[derive(Default)]
struct ResourceControlCase {
    nodes: Vec<String>,
    tasks: HashMap<i64, TaskDemand>,
    active: HashMap<i64, HashMap<String, i64>>,
    next_subtask_id: i64,
}

impl ResourceControlCase {
    fn new(node_count: usize) -> Self {
        let mut case = Self::default();
        case.scale_out(node_count);
        case
    }

    fn scale_out(&mut self, count: usize) {
        let first = self.nodes.len();
        self.nodes
            .extend((first..first + count).map(|i| format!("node-{i}")));
        self.schedule();
    }

    fn scale_in(&mut self, count: usize) {
        self.nodes.truncate(self.nodes.len().saturating_sub(count));
        self.schedule();
    }

    fn add_task(&mut self, id: i64, slots: i32, subtasks: usize) {
        self.tasks.insert(
            id,
            TaskDemand {
                slots,
                remaining: subtasks,
                step: 1,
            },
        );
        self.schedule();
    }

    fn enter_next_step(&mut self, id: i64, subtasks: usize) {
        let task = self.tasks.get_mut(&id).unwrap();
        task.step += 1;
        task.remaining = subtasks;
        self.active.remove(&id);
        self.schedule();
    }

    fn complete_task_subtasks(&mut self, id: i64) {
        let completed = self.active.remove(&id).map_or(0, |items| items.len());
        let task = self.tasks.get_mut(&id).unwrap();
        task.remaining = task.remaining.saturating_sub(completed);
        self.schedule();
    }

    fn complete_all_subtasks(&mut self) {
        for id in self.active.keys().copied().collect::<Vec<_>>() {
            let completed = self.active.remove(&id).unwrap().len();
            let task = self.tasks.get_mut(&id).unwrap();
            task.remaining = task.remaining.saturating_sub(completed);
        }
        self.schedule();
    }

    fn schedule(&mut self) {
        let old_active = std::mem::take(&mut self.active);
        let mut used = self
            .nodes
            .iter()
            .cloned()
            .map(|node| (node, 0))
            .collect::<HashMap<_, _>>();
        let mut ids = self.tasks.keys().copied().collect::<Vec<_>>();
        ids.sort_unstable();
        for id in ids {
            let demand = self.tasks[&id];
            let eligible =
                filter_nodes_with_enough_slots(&used, NODE_CAPACITY, &self.nodes, demand.slots);
            let mut assignments = HashMap::new();
            for node in eligible.into_iter().take(demand.remaining) {
                let subtask_id = old_active
                    .get(&id)
                    .and_then(|m| m.get(&node))
                    .copied()
                    .unwrap_or_else(|| {
                        self.next_subtask_id += 1;
                        self.next_subtask_id
                    });
                *used.get_mut(&node).unwrap() += demand.slots;
                assignments.insert(node, subtask_id);
            }
            if !assignments.is_empty() {
                self.active.insert(id, assignments);
            }
        }
    }

    fn count(&self, id: i64) -> usize {
        self.active.get(&id).map_or(0, HashMap::len)
    }

    fn total_count(&self) -> usize {
        self.active.values().map(HashMap::len).sum()
    }

    fn subtask_ids(&self, id: i64) -> HashSet<i64> {
        self.active
            .get(&id)
            .into_iter()
            .flat_map(HashMap::values)
            .copied()
            .collect()
    }

    fn new_subtask_count(&self, id: i64, old: &HashSet<i64>) -> usize {
        self.subtask_ids(id).difference(old).count()
    }
}

#[test]
fn slot_filter_keeps_go_order_missing_node_and_capacity_boundaries() {
    let used = HashMap::from([
        ("n1".to_owned(), 8),
        ("n2".to_owned(), 1),
        ("n3".to_owned(), 0),
    ]);
    let nodes = vec![
        "n3".to_owned(),
        "n2".to_owned(),
        "n1".to_owned(),
        "missing".to_owned(),
    ];
    assert_eq!(
        filter_nodes_with_enough_slots(&used, 8, &nodes, 1),
        vec!["n3".to_owned(), "n2".to_owned()]
    );
}

#[test]
fn fully_utilized_runs_all_sixteen_subtasks() {
    let mut case = ResourceControlCase::new(4);
    for id in 1..=4 {
        case.add_task(id, 4, 4);
    }
    assert_eq!(case.total_count(), 16);
    assert_eq!((1..=4).map(|id| case.count(id)).collect::<Vec<_>>(), [4; 4]);
}

#[test]
fn scale_out_reaches_full_utilization() {
    let mut case = ResourceControlCase::new(2);
    for id in 1..=4 {
        case.add_task(id, 4, 4);
    }
    assert_eq!(case.total_count(), 8);
    assert_eq!((1..=4).map(|id| case.count(id)).collect::<Vec<_>>(), [2; 4]);
    case.scale_out(2);
    assert_eq!(case.total_count(), 16);
}

#[test]
fn scale_in_cancels_excess_and_reschedules_new_subtasks() {
    let mut case = ResourceControlCase::new(4);
    for id in 1..=4 {
        case.add_task(id, 4, 4);
    }
    case.scale_in(1);
    assert_eq!(case.total_count(), 12);
    let old = (1..=4)
        .map(|id| (id, case.subtask_ids(id)))
        .collect::<HashMap<_, _>>();
    case.complete_all_subtasks();
    for id in 1..=4 {
        assert_eq!(case.new_subtask_count(id, &old[&id]), 1);
    }
    case.scale_in(2);
    assert_eq!(case.total_count(), 4);
    assert_eq!((1..=4).map(|id| case.count(id)).collect::<Vec<_>>(), [1; 4]);
}

#[test]
fn high_rank_task_releases_nodes_for_the_waiting_task() {
    let mut case = ResourceControlCase::new(4);
    case.add_task(1, 8, 4);
    case.add_task(2, 16, 3);
    assert_eq!((case.count(1), case.count(2)), (4, 0));
    case.complete_task_subtasks(1);
    case.enter_next_step(1, 1);
    assert_eq!((case.count(1), case.count(2)), (1, 3));
}

#[test]
fn high_rank_next_step_preempts_lower_rank_task() {
    let mut case = ResourceControlCase::new(4);
    case.add_task(1, 8, 1);
    case.add_task(2, 16, 3);
    let old_subtask_ids = case.subtask_ids(2);
    assert_eq!((case.count(1), case.count(2)), (1, 3));
    case.complete_task_subtasks(1);
    case.enter_next_step(1, 4);
    assert_eq!((case.count(1), case.count(2)), (4, 0));
    case.complete_task_subtasks(1);
    assert_eq!(case.new_subtask_count(2, &old_subtask_ids), 3);
}

#[test]
fn blocked_middle_rank_does_not_block_lower_rank_task() {
    let mut case = ResourceControlCase::new(4);
    case.add_task(1, 8, 4);
    case.add_task(2, 16, 2);
    case.add_task(3, 8, 4);
    assert_eq!((case.count(1), case.count(2), case.count(3)), (4, 0, 4));
    case.complete_task_subtasks(1);
    assert_eq!((case.count(2), case.count(3)), (2, 2));
    case.complete_task_subtasks(2);
    assert_eq!(case.count(3), 4);
}
