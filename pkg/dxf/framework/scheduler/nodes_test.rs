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

// 节点管理测试：验证失联节点清理、managed 节点快照刷新与调度 scope 过滤。
//
// 重点覆盖重复维护的幂等性、容量只随有效 CPU 数更新，以及空 scope 对
// background 节点的默认选择规则，避免节点发现结果与调度视图发生偏差。

use crate::test_support::TestTaskManager;
use crate::*;

/// 构造测试节点，使各用例能显式组合角色与 CPU 容量。
fn node(id: &str, role: &str, cpu_count: i32) -> ManagedNode {
    ManagedNode {
        id: id.to_owned(),
        role: role.to_owned(),
        cpu_count,
    }
}

#[test]
/// 验证只删除不在存活集合中的节点，并且相同存活快照不会重复删除。
fn test_maintain_live_nodes() {
    let task_manager = TestTaskManager::default();
    *task_manager.nodes.lock().unwrap() = vec![
        node("live", "", 8),
        node("dead-a", "", 8),
        node("dead-b", "", 8),
    ];
    let node_manager = NodeManager::new();

    let deleted = node_manager
        .maintain_live_nodes(&task_manager, &["live".to_owned()])
        .unwrap();
    assert_eq!(deleted, vec!["dead-a", "dead-b"]);
    assert_eq!(
        task_manager.deleted_nodes.lock().unwrap().as_slice(),
        ["dead-a", "dead-b"]
    );

    let deleted_again = node_manager
        .maintain_live_nodes(&task_manager, &["live".to_owned()])
        .unwrap();
    assert!(deleted_again.is_empty());
}

#[test]
/// 验证节点快照刷新，并以首个正 CPU 数更新全局槽位容量。
fn test_maintain_managed_nodes() {
    let task_manager = TestTaskManager::default();
    *task_manager.nodes.lock().unwrap() = vec![
        node("n1", "", 0),
        node("n2", "background", 12),
        node("n3", "", 16),
    ];
    let node_manager = NodeManager::new();
    let slot_manager = SlotManager::new();
    slot_manager.update_capacity(4);

    let refreshed = node_manager
        .refresh_nodes(&task_manager, &slot_manager)
        .unwrap();
    assert_eq!(refreshed, task_manager.nodes.lock().unwrap().clone());
    assert_eq!(node_manager.get_nodes(), refreshed);
    assert_eq!(slot_manager.capacity(), 12);

    // get_nodes 对齐 Go slices.Clone：修改返回值不能污染 manager 内部快照。
    let mut snapshot = node_manager.get_nodes();
    snapshot.clear();
    assert_eq!(node_manager.get_nodes(), refreshed);

    // 空快照应清空可调度节点，但不能抹掉上一轮已确认的有效容量。
    task_manager.nodes.lock().unwrap().clear();
    node_manager
        .refresh_nodes(&task_manager, &slot_manager)
        .unwrap();
    assert!(node_manager.get_nodes().is_empty());
    assert_eq!(slot_manager.capacity(), 12);
}

#[test]
/// 验证显式 scope 与空 scope 的角色匹配规则，包括 background 的默认优先级。
fn test_filter_by_scope() {
    let cases = [
        (
            vec![
                node("1", "background", 8),
                node("2", "background", 8),
                node("3", "", 8),
            ],
            "",
            vec!["1", "2"],
        ),
        (
            vec![node("1", "", 8), node("2", "", 8), node("3", "", 8)],
            "",
            vec!["1", "2", "3"],
        ),
        (
            vec![node("1", "one", 8), node("2", "", 8), node("3", "", 8)],
            "",
            vec!["2", "3"],
        ),
        (
            vec![
                node("1", "one", 8),
                node("2", "two", 8),
                node("3", "background", 8),
            ],
            "two",
            vec!["2"],
        ),
        (
            vec![node("1", "one", 8), node("2", "", 8), node("3", "", 8)],
            "one",
            vec!["1"],
        ),
        (
            vec![
                node("1", "one", 8),
                node("2", "two", 8),
                node("3", "two", 8),
            ],
            "one",
            vec!["1"],
        ),
        (
            vec![
                node("1", "one", 8),
                node("2", "two", 8),
                node("3", "three", 8),
            ],
            "two",
            vec!["2"],
        ),
        (
            vec![
                node("1", "one", 8),
                node("2", "", 8),
                node("3", "background", 8),
            ],
            "",
            vec!["3"],
        ),
        (
            vec![node("1", "", 8), node("2", "", 8)],
            "background",
            vec![],
        ),
    ];
    for (nodes, scope, expected) in cases {
        assert_eq!(
            filter_by_scope(&nodes, scope),
            expected.into_iter().map(str::to_owned).collect::<Vec<_>>()
        );
    }
}
