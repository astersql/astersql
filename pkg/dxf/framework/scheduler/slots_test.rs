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

// 调度槽位管理器的资源预留与节点筛选测试。
//
// 覆盖经典模式下按任务排名预留 stripe、容量不足时回退到单节点 slot，
// 以及 next-gen 模式跳过本地预留的差异；同时验证节点用量快照刷新、
// 候选节点过滤和容量更新的边界行为。

use crate::test_support::TestTaskManager;
use crate::*;
use std::collections::HashMap;
use std::time::{Duration, SystemTime};

/// 构造排名可预测的任务：优先级相同时，创建时间随 ID 递增。
fn ranked_task(id: i64, required_slots: i32) -> TaskBase {
    TaskBase {
        id,
        required_slots,
        priority: 512,
        create_time: SystemTime::UNIX_EPOCH + Duration::from_secs(id as u64),
        ..Default::default()
    }
}

#[test]
fn test_slot_manager_reserve_next_gen() {
    // next-gen 由集群控制器按需扩容，调度层无需节点快照即可判定可调度。
    let manager = SlotManager::new();
    manager.set_next_gen(true);
    manager.update_capacity(16);
    assert_eq!(
        manager.can_reserve(&ranked_task(1, 16)),
        (String::new(), true)
    );
    manager.reserve(&ranked_task(1, 16), "n1");
    manager.unreserve(&ranked_task(1, 16), "n1");
}

#[test]
fn test_slot_manager_reserve() {
    let manager = SlotManager::new();
    manager.update_capacity(16);
    // 经典模式缺少节点用量快照时，无法判断剩余容量。
    assert!(!manager.can_reserve(&ranked_task(1, 1)).1);

    // stripe 预留只累计排名更高的任务，因此前两个任务均可进入预留队列。
    manager.set_used_slots(HashMap::from([("n1".to_owned(), 16)]));
    let task10 = ranked_task(10, 4);
    assert_eq!(manager.can_reserve(&task10), (String::new(), true));
    manager.reserve(&task10, "");

    let task20 = ranked_task(20, 8);
    assert_eq!(manager.can_reserve(&task20), (String::new(), true));
    manager.reserve(&task20, "");

    let task30 = ranked_task(30, 8);
    assert!(!manager.can_reserve(&task30).1);
    // 高排名任务不受低排名预留阻挡；介于现有任务之间的任务只累计更高排名预留。
    let task9 = ranked_task(9, 16);
    assert_eq!(manager.can_reserve(&task9), (String::new(), true));
    let mut task11 = ranked_task(11, 16);
    assert!(!manager.can_reserve(&task11).1);
    task11.required_slots = 12;
    assert_eq!(manager.can_reserve(&task11), (String::new(), true));

    // stripe 容量不足时回退到具体节点；n2 恰好还能容纳 8 个 slot。
    manager.set_used_slots(HashMap::from([("n1".to_owned(), 12), ("n2".to_owned(), 8)]));
    let mut task40 = ranked_task(40, 16);
    assert!(!manager.can_reserve(&task40).1);
    task40.required_slots = 8;
    let (exec_id, ok) = manager.can_reserve(&task40);
    assert!(ok);
    assert_eq!(exec_id, "n2");
    manager.reserve(&task40, &exec_id);

    // 释放 task10 会重建排名索引，使 task15 从受阻变为可按 stripe 调度。
    let task15 = ranked_task(15, 16);
    assert!(!manager.can_reserve(&task15).1);
    manager.unreserve(&task10, "");
    assert_eq!(manager.can_reserve(&task15), (String::new(), true));
    manager.reserve(&task15, "");

    let task50 = ranked_task(50, 8);
    assert!(!manager.can_reserve(&task50).1);
    manager.unreserve(&task40, &exec_id);
    let (exec_id50, ok) = manager.can_reserve(&task50);
    assert!(ok);
    assert_eq!(exec_id50, "n2");
    manager.reserve(&task50, &exec_id50);

    // 节点级预留必须累计；task60 只能落到 n1。
    let task60 = ranked_task(60, 4);
    let (exec_id60, ok) = manager.can_reserve(&task60);
    assert!(ok);
    assert_eq!(exec_id60, "n1");
    manager.reserve(&task60, &exec_id60);

    // 与 Go 用例一致，释放全部任务后所有预留状态都不再影响后续任务。
    manager.unreserve(&task15, "");
    manager.unreserve(&task20, "");
    manager.unreserve(&task50, &exec_id50);
    manager.unreserve(&task60, &exec_id60);
    assert_eq!(manager.can_reserve(&task30), (String::new(), true));
}

#[test]
fn test_slot_manager_update() {
    let task_manager = TestTaskManager::default();
    *task_manager.nodes.lock().unwrap() = vec![
        ManagedNode {
            id: "n1".to_owned(),
            role: String::new(),
            cpu_count: 16,
        },
        ManagedNode {
            id: "n2".to_owned(),
            role: String::new(),
            cpu_count: 16,
        },
    ];
    *task_manager.used_slots.lock().unwrap() =
        HashMap::from([("n1".to_owned(), 12), ("stale".to_owned(), 8)]);
    let node_manager = NodeManager::new();
    let slot_manager = SlotManager::new();
    node_manager
        .refresh_nodes(&task_manager, &slot_manager)
        .unwrap();
    slot_manager.update(&node_manager, &task_manager).unwrap();

    // 刷新仅保留当前托管节点；未上报的 n2 按零已用处理，stale 被丢弃。
    assert_eq!(
        slot_manager.adjust_eligible_nodes(vec!["n1".to_owned(), "n2".to_owned()], 8),
        vec!["n2".to_owned()]
    );

    // 节点缩容后快照同步移除已不受管理的节点。
    *task_manager.nodes.lock().unwrap() = vec![ManagedNode {
        id: "n1".to_owned(),
        role: String::new(),
        cpu_count: 16,
    }];
    *task_manager.used_slots.lock().unwrap() =
        HashMap::from([("n1".to_owned(), 12), ("n2".to_owned(), 8)]);
    node_manager
        .refresh_nodes(&task_manager, &slot_manager)
        .unwrap();
    slot_manager.update(&node_manager, &task_manager).unwrap();
    assert_eq!(
        slot_manager.adjust_eligible_nodes(vec!["n1".to_owned(), "n2".to_owned()], 1),
        vec!["n1".to_owned()]
    );
}

#[test]
fn test_scheduler_adjust_eligible_nodes() {
    let manager = SlotManager::new();
    manager.update_capacity(16);
    let all_nodes = vec!["n1".to_owned(), "n2".to_owned(), "n3".to_owned()];
    assert_eq!(
        manager.adjust_eligible_nodes(all_nodes.clone(), 10),
        all_nodes
    );

    // 有容量足够的节点时只返回这些节点，且忽略候选集之外的陈旧记录。
    manager.set_used_slots(HashMap::from([
        ("n1".to_owned(), 12),
        ("n2".to_owned(), 4),
        ("stale".to_owned(), 0),
    ]));
    assert_eq!(
        manager.adjust_eligible_nodes(all_nodes, 10),
        vec!["n2".to_owned()]
    );
}

#[test]
fn test_slot_manager_update_capacity() {
    let manager = SlotManager::new();
    manager.update_capacity(16);
    assert_eq!(manager.capacity(), 16);
    manager.update_capacity(32);
    assert_eq!(manager.capacity(), 32);
    // 非正 CPU 数不是有效容量更新，必须保留上一次有效值。
    manager.update_capacity(0);
    assert_eq!(manager.capacity(), 32);
}
