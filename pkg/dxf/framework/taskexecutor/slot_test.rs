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

// `slotManager` 与 `TaskBase::Compare` 单元测试。
//
// 覆盖 slot 分配/释放、高优先级抢占评估、`exchange` 扩缩容以及优先级比较顺序。

// Ported from pkg/dxf/framework/taskexecutor/slot_test.go, driving the real
// `slotManager`/`TaskBase` production types instead of local arithmetic.

// Compare 用 CreateTime 作为次级排序键。
use std::time::SystemTime;

use crate::{TaskBase, newSlotManager};

/// 构造仅填充 ID/Priority/RequiredSlots 的测试任务。
fn task(id: i64, priority: i32, required_slots: i32) -> TaskBase {
    TaskBase {
        ID: id,
        Priority: priority,
        RequiredSlots: required_slots,
        ..Default::default()
    }
}

#[test]
/// 覆盖分配、不足拒绝、优先级抢占评估、释放与排序。
fn test_slot_manager() {
    let sm = newSlotManager(10);

    // rank(required slots in parenthesis): task1(1), task2(10)
    let task1 = task(1, 1, 1);
    let mut task2 = task(2, 2, 10);

    let (can_alloc, tasks_need_free) = sm.canAlloc(&task1);
    assert!(can_alloc);
    assert!(tasks_need_free.is_empty());
    assert!(sm.alloc(&task1));
    assert_eq!(1, sm.TasksForTest().len());
    assert!(sm.TasksForTest().contains(&task1));
    assert_eq!(9, sm.availableSlots());

    // the available slots is not enough for task2
    let (can_alloc, tasks_need_free) = sm.canAlloc(&task2);
    assert!(!can_alloc);
    assert!(tasks_need_free.is_empty());
    // call alloc will fail
    assert!(!sm.alloc(&task2));

    // rank: task2(10), task1(1)
    // increase the priority of task2, task2 is waiting for allocation
    task2.Priority = 0;
    let (can_alloc, tasks_need_free) = sm.canAlloc(&task2);
    assert!(can_alloc);
    assert_eq!(vec![task1.clone()], tasks_need_free);
    // call alloc still fail as task is still holding the resources
    assert!(!sm.alloc(&task2));

    // rank: task3(1), task2(10), task1(1)
    // task with higher priority can alloc
    let task3 = task(3, -1, 1);
    let (can_alloc, tasks_need_free) = sm.canAlloc(&task3);
    assert!(can_alloc);
    assert!(tasks_need_free.is_empty());
    assert!(sm.alloc(&task3));
    assert_eq!(2, sm.TasksForTest().len());
    assert!(sm.TasksForTest().contains(&task3));
    assert_eq!(8, sm.availableSlots());

    // rank: task3(1), task2(10), task1(1), task4(1)
    let task4 = task(4, 1, 1);
    let (can_alloc, tasks_need_free) = sm.canAlloc(&task4);
    assert!(can_alloc);
    assert!(tasks_need_free.is_empty());
    assert!(sm.alloc(&task4));
    assert_eq!(7, sm.availableSlots());
    // rank: task3(1), task2(10), task5(1), task1(1), task4(1)
    let task5 = task(5, 0, 1);
    let (can_alloc, tasks_need_free) = sm.canAlloc(&task5);
    assert!(can_alloc);
    assert!(tasks_need_free.is_empty());
    assert!(sm.alloc(&task5));
    assert_eq!(4, sm.TasksForTest().len());
    // test the order of the priority-sorted task list.
    assert_eq!(
        vec![task4.clone(), task1.clone(), task5.clone(), task3.clone()],
        sm.TasksForTest()
    );
    assert_eq!(6, sm.availableSlots());

    // rank: task3(1), task2(10), task5(1), task6(8), task1(1), task4(1)
    let mut task6 = task(6, 0, 8);
    let (can_alloc, tasks_need_free) = sm.canAlloc(&task6);
    assert!(can_alloc);
    assert_eq!(vec![task4.clone(), task1.clone()], tasks_need_free);
    // rank: task3(1), task2(10), task5(1), task6(9), task1(1), task4(1)
    task6.RequiredSlots = 9;
    let (can_alloc, tasks_need_free) = sm.canAlloc(&task6);
    assert!(!can_alloc);
    assert!(tasks_need_free.is_empty());

    sm.free(task4.ID);
    sm.free(task5.ID);
    sm.free(task3.ID);
    assert_eq!(1, sm.TasksForTest().len());
    assert!(sm.TasksForTest().contains(&task1));
    assert_eq!(9, sm.availableSlots());

    // task2 is waiting for allocation again
    let (can_alloc, tasks_need_free) = sm.canAlloc(&task2);
    assert!(can_alloc);
    assert_eq!(vec![task1.clone()], tasks_need_free);

    sm.free(task1.ID);
    assert!(sm.TasksForTest().is_empty());
    assert_eq!(10, sm.availableSlots());

    assert!(sm.alloc(&task2));
    assert_eq!(1, sm.TasksForTest().len());
    assert!(sm.TasksForTest().contains(&task2));
    assert_eq!(0, sm.availableSlots());
    sm.free(task2.ID);
    assert!(sm.TasksForTest().is_empty());
    assert_eq!(10, sm.availableSlots());
}

#[test]
/// 覆盖 exchange：未知任务、扩容不足、扩容成功、缩容与幂等。
fn test_slot_manager_exchange_slots() {
    let task1 = task(1, 0, 4);
    let task2 = task(2, 0, 8);

    {
        let sm = newSlotManager(16);
        assert!(!sm.exchange(&task(1, 0, 8)));
    }

    {
        let sm = newSlotManager(16);
        assert!(sm.alloc(&task1));
        assert_eq!(4, sm.TasksForTest()[0].RequiredSlots);
        assert_eq!(12, sm.availableSlots());
        assert!(!sm.exchange(&task(1, 0, 32)));
        assert_eq!(4, sm.TasksForTest()[0].RequiredSlots);
        assert_eq!(12, sm.availableSlots());
    }

    {
        let sm = newSlotManager(16);
        assert!(sm.alloc(&task1));
        assert_eq!(12, sm.availableSlots());
        assert!(sm.exchange(&task(1, 0, 8)));
        assert_eq!(1, sm.TasksForTest().len());
        assert_eq!(8, sm.TasksForTest()[0].RequiredSlots);
        assert_eq!(8, sm.availableSlots());
    }

    {
        let sm = newSlotManager(16);
        assert!(sm.alloc(&task1));
        assert_eq!(12, sm.availableSlots());
        assert!(sm.exchange(&task(1, 0, 2)));
        assert_eq!(1, sm.TasksForTest().len());
        assert_eq!(2, sm.TasksForTest()[0].RequiredSlots);
        assert_eq!(14, sm.availableSlots());
        // exchange again, no change
        assert!(sm.exchange(&task(1, 0, 2)));
        assert_eq!(1, sm.TasksForTest().len());
        assert_eq!(2, sm.TasksForTest()[0].RequiredSlots);
        assert_eq!(14, sm.availableSlots());
    }

    {
        let sm = newSlotManager(16);
        assert!(sm.alloc(&task1));
        assert_eq!(12, sm.availableSlots());
        let (can_alloc, tasks_need_free) = sm.canAlloc(&task2);
        assert!(can_alloc);
        assert!(tasks_need_free.is_empty());
        assert!(sm.exchange(&task(1, 0, 10)));
        assert_eq!(1, sm.TasksForTest().len());
        assert_eq!(10, sm.TasksForTest()[0].RequiredSlots);
        assert_eq!(6, sm.availableSlots());
        assert!(!sm.alloc(&task2));
        assert_eq!(6, sm.availableSlots());
    }
}

#[test]
/// 验证 usedSlots = capacity − available。
fn test_slot_manager_used_slots() {
    let sm = newSlotManager(10);
    assert_eq!(0, sm.usedSlots());
    assert!(sm.alloc(&task(1, 0, 3)));
    assert_eq!(3, sm.usedSlots());
    assert_eq!(7, sm.availableSlots());
    sm.free(1);
    assert_eq!(0, sm.usedSlots());
}

#[test]
/// 验证 Compare：Priority → CreateTime → ID。
fn test_task_base_compare_uses_priority_then_create_time_then_id() {
    let now = SystemTime::now();
    let higher_priority = TaskBase {
        Priority: -1,
        ..Default::default()
    };
    let lower_priority = TaskBase {
        Priority: 1,
        ..Default::default()
    };
    assert_eq!(-1, higher_priority.Compare(&lower_priority));
    assert_eq!(1, lower_priority.Compare(&higher_priority));

    let earlier = TaskBase {
        CreateTime: now,
        ..Default::default()
    };
    let later = TaskBase {
        CreateTime: now + std::time::Duration::from_secs(1),
        ..Default::default()
    };
    assert_eq!(-1, earlier.Compare(&later));

    let id1 = TaskBase {
        ID: 1,
        ..Default::default()
    };
    let id2 = TaskBase {
        ID: 2,
        ..Default::default()
    };
    assert_eq!(-1, id1.Compare(&id2));
    assert_eq!(0, id1.Compare(&id1.clone()));
}
