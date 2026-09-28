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

// DXF 子任务均衡器测试。
//
// 重点验证失效节点上的子任务迁移、同一轮多任务之间的槽位记账，以及最大节点数与
// 剩余容量过滤；这些约束共同保证均衡过程不会把任务分配到已失效或资源不足的节点。

use crate::test_support::{TestExtension, TestTaskManager, task};
use crate::*;
use std::collections::HashMap;
use std::sync::Arc;

// 构造均衡测试所需的最小活跃子任务，固定步骤、并发度和顺序号等非关注字段。
fn subtask(id: i64, exec_id: &str, state: SubtaskState, task_id: i64) -> SubtaskBase {
    SubtaskBase {
        id,
        exec_id: exec_id.to_owned(),
        state,
        task_id,
        step: 1,
        concurrency: 1,
        ordinal: id as i32,
    }
}

// 将任务同时登记到测试管理器并创建调度器，使均衡器能通过共享管理器读取其子任务。
fn scheduler_for(
    task: Task,
    manager: Arc<TestTaskManager>,
    node_manager: Arc<NodeManager>,
    slot_manager: Arc<SlotManager>,
) -> Arc<dyn Scheduler> {
    manager.insert_task(task.clone());
    Arc::new(BaseScheduler::new(
        task,
        Param {
            task_manager: manager,
            node_manager,
            slot_manager,
            server_id: "test".to_owned(),
            allocated_slots: true,
            node_resource: None,
        },
        Arc::new(TestExtension::default()),
    ))
}

#[test]
fn test_balance_one_task() {
    let manager = Arc::new(TestTaskManager::default());
    let mut scheduler_task = task(1, TASK_STATE_RUNNING);
    scheduler_task.base.step = 1;
    scheduler_task.base.required_slots = 4;
    // `dead` 不在托管节点中，因此其 running 与 pending 子任务都必须故障转移到 `live`。
    manager.active_subtasks.lock().unwrap().insert(
        1,
        vec![
            subtask(1, "dead", SUBTASK_STATE_RUNNING, 1),
            subtask(2, "dead", SUBTASK_STATE_PENDING, 1),
        ],
    );

    let node_manager = Arc::new(NodeManager::new());
    node_manager.set_nodes(vec![ManagedNode {
        id: "live".to_owned(),
        role: String::new(),
        cpu_count: 8,
    }]);
    let slot_manager = Arc::new(SlotManager::new());
    slot_manager.update_capacity(8);
    let scheduler = scheduler_for(
        scheduler_task,
        Arc::clone(&manager),
        Arc::clone(&node_manager),
        Arc::clone(&slot_manager),
    );
    let mut balancer = Balancer::new(Param {
        task_manager: manager.clone(),
        node_manager,
        slot_manager,
        server_id: "test".to_owned(),
        allocated_slots: true,
        node_resource: None,
    });

    balancer.balance(&[scheduler]).unwrap();
    let updates = manager.updated_subtasks.lock().unwrap().clone();
    assert_eq!(updates.len(), 2);
    assert!(updates.iter().all(|subtask| subtask.exec_id == "live"));
}

#[test]
fn test_balance_multiple_tasks() {
    let manager = Arc::new(TestTaskManager::default());
    let node_manager = Arc::new(NodeManager::new());
    node_manager.set_nodes(vec![
        ManagedNode {
            id: "n1".to_owned(),
            role: String::new(),
            cpu_count: 8,
        },
        ManagedNode {
            id: "n2".to_owned(),
            role: String::new(),
            cpu_count: 8,
        },
    ]);
    let slot_manager = Arc::new(SlotManager::new());
    slot_manager.update_capacity(8);

    let mut first = task(1, TASK_STATE_RUNNING);
    first.base.step = 1;
    first.base.required_slots = 8;
    let mut second = task(2, TASK_STATE_RUNNING);
    second.base.step = 1;
    second.base.required_slots = 8;
    // 首个任务占满两个节点；第二个任务本轮无剩余槽位，应保持原分配且不产生更新。
    manager.active_subtasks.lock().unwrap().extend([
        (
            1,
            vec![
                subtask(1, "n1", SUBTASK_STATE_RUNNING, 1),
                subtask(2, "n2", SUBTASK_STATE_RUNNING, 1),
            ],
        ),
        (2, vec![subtask(3, "n1", SUBTASK_STATE_PENDING, 2)]),
    ]);
    let schedulers = vec![
        scheduler_for(
            first,
            Arc::clone(&manager),
            Arc::clone(&node_manager),
            Arc::clone(&slot_manager),
        ),
        scheduler_for(
            second,
            Arc::clone(&manager),
            Arc::clone(&node_manager),
            Arc::clone(&slot_manager),
        ),
    ];
    let mut balancer = Balancer::new(Param {
        task_manager: manager.clone(),
        node_manager,
        slot_manager,
        server_id: "test".to_owned(),
        allocated_slots: true,
        node_resource: None,
    });

    balancer.balance(&schedulers).unwrap();
    assert!(manager.updated_subtasks.lock().unwrap().is_empty());
}

#[test]
fn test_balancer_update_used_nodes() {
    let subtasks = vec![
        subtask(1, "n2", SUBTASK_STATE_RUNNING, 1),
        subtask(2, "n2", SUBTASK_STATE_PENDING, 1),
        subtask(3, "n1", SUBTASK_STATE_PENDING, 1),
    ];
    // 达到节点数上限时，优先保留已有子任务较多的节点，并维持原始顺序处理并列项。
    assert_eq!(
        filter_nodes_by_max_node_count(
            vec!["n1".to_owned(), "n2".to_owned(), "n3".to_owned()],
            &subtasks,
            2,
        ),
        vec!["n2".to_owned(), "n1".to_owned()]
    );

    let slot_manager = SlotManager::new();
    slot_manager.update_capacity(16);
    slot_manager.set_used_slots(HashMap::from([("n1".to_owned(), 16), ("n2".to_owned(), 8)]));
    // 单个子任务需要 8 个槽位，已满载的 n1 被剔除，仅保留仍有容量的 n2。
    assert_eq!(
        slot_manager.adjust_eligible_nodes(vec!["n1".to_owned(), "n2".to_owned()], 8),
        vec!["n2".to_owned()]
    );
}

#[test]
fn test_balance_matches_go_table_driven_edge_cases() {
    struct Case {
        name: &'static str,
        subtasks: Vec<SubtaskBase>,
        max_node_count: i32,
        expected_counts: Vec<usize>,
        expected_updates: usize,
    }

    let cases = [
        Case {
            name: "no subtasks",
            subtasks: vec![],
            max_node_count: 0,
            expected_counts: vec![0, 0, 0],
            expected_updates: 0,
        },
        Case {
            name: "scale out keeps the remainder spread",
            subtasks: vec![
                subtask(1, "n2", SUBTASK_STATE_RUNNING, 1),
                subtask(2, "n2", SUBTASK_STATE_PENDING, 1),
                subtask(3, "n2", SUBTASK_STATE_PENDING, 1),
                subtask(4, "n2", SUBTASK_STATE_PENDING, 1),
            ],
            max_node_count: 0,
            expected_counts: vec![1, 2, 1],
            expected_updates: 2,
        },
        Case {
            name: "running subtasks on an eligible node never move",
            subtasks: vec![
                subtask(1, "n1", SUBTASK_STATE_RUNNING, 1),
                subtask(2, "n1", SUBTASK_STATE_RUNNING, 1),
                subtask(3, "n1", SUBTASK_STATE_RUNNING, 1),
            ],
            max_node_count: 0,
            expected_counts: vec![3, 0, 0],
            expected_updates: 0,
        },
        Case {
            name: "max node count retains the busiest executor",
            subtasks: vec![
                subtask(1, "n1", SUBTASK_STATE_RUNNING, 1),
                subtask(2, "n2", SUBTASK_STATE_RUNNING, 1),
                subtask(3, "n2", SUBTASK_STATE_PENDING, 1),
            ],
            max_node_count: 1,
            expected_counts: vec![0, 3, 0],
            expected_updates: 1,
        },
    ];

    for case in cases {
        let manager = Arc::new(TestTaskManager::default());
        let mut scheduler_task = task(1, TASK_STATE_RUNNING);
        scheduler_task.base.step = 1;
        scheduler_task.base.required_slots = 16;
        scheduler_task.base.max_node_count = case.max_node_count;
        manager
            .active_subtasks
            .lock()
            .unwrap()
            .insert(1, case.subtasks);

        let node_manager = Arc::new(NodeManager::new());
        node_manager.set_nodes(
            ["n1", "n2", "n3"]
                .into_iter()
                .map(|id| ManagedNode {
                    id: id.to_owned(),
                    role: String::new(),
                    cpu_count: 16,
                })
                .collect(),
        );
        let slot_manager = Arc::new(SlotManager::new());
        slot_manager.update_capacity(16);
        let scheduler = scheduler_for(
            scheduler_task,
            Arc::clone(&manager),
            Arc::clone(&node_manager),
            Arc::clone(&slot_manager),
        );
        let mut balancer = Balancer::new(Param {
            task_manager: manager.clone(),
            node_manager,
            slot_manager,
            server_id: "test".to_owned(),
            allocated_slots: true,
            node_resource: None,
        });

        balancer.balance(&[scheduler]).unwrap_or_else(|error| {
            panic!("{}: balance failed: {error}", case.name);
        });
        let active = manager.active_subtasks.lock().unwrap();
        let active = active.get(&1).cloned().unwrap_or_default();
        let counts = ["n1", "n2", "n3"].map(|node| {
            active
                .iter()
                .filter(|subtask| subtask.exec_id == node)
                .count()
        });
        assert_eq!(counts.as_slice(), case.expected_counts, "{}", case.name);
        assert_eq!(
            manager.updated_subtasks.lock().unwrap().len(),
            case.expected_updates,
            "{}",
            case.name
        );
    }
}
