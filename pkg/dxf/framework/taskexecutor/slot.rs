// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// CPU slot（执行槽位）分配与抢占。
//
// 每个任务声明 `RequiredSlots`；节点容量通常等于 `TotalCPU`。
// 高优先级任务可通过 `canAlloc` 指出需抢占的低优任务，由 Manager 先 Cancel 再等待释放。

use crate::TaskBase;
use std::collections::HashMap;
use std::sync::Mutex;
/// slot 管理器内部状态：任务列表、索引与剩余容量。
struct Slots {
    /// 任务 ID → `tasks` 下标，加速释放/交换。
    index: HashMap<i64, usize>,
    /// 已占用 slot 的任务，按优先级降序（低优在前便于抢占扫描）。
    tasks: Vec<TaskBase>,
    /// 当前剩余可用 slot 数。
    available: i32,
}
/// CPU slot（执行槽位）管理器：分配、释放、抢占评估与配额交换。
pub struct slotManager {
    /// 总容量（通常等于节点 TotalCPU）。
    capacity: i32,
    /// 受互斥保护的内部状态。
    inner: Mutex<Slots>,
}
/// 创建指定容量的 slot 管理器。
pub fn newSlotManager(capacity: i32) -> slotManager {
    slotManager {
        capacity,
        inner: Mutex::new(Slots {
            index: HashMap::new(),
            tasks: vec![],
            available: capacity,
        }),
    }
}
impl slotManager {
    /// 根据 `tasks` 重建 ID→下标索引。
    fn rebuild(inner: &mut Slots) {
        inner.index.clear();
        for (i, t) in inner.tasks.iter().enumerate() {
            inner.index.insert(t.ID, i);
        }
    }
    /// 判断能否分配：空闲足够则直接可分配；否则看能否通过取消更低优先级任务腾出空间。
    fn canAlloc0(inner: &Slots, task: &TaskBase) -> (bool, Vec<TaskBase>) {
        // 空闲 slot 已够，无需抢占。
        if inner.available >= task.RequiredSlots {
            return (true, vec![]);
        }
        let mut free = 0;
        let mut tasks = vec![];
        // tasks 按优先级从低到高排列；Compare<0 表示 running 优先级更低。
        for running in &inner.tasks {
            if running.Compare(task) < 0 {
                break;
            }
            free += running.RequiredSlots;
            tasks.push(running.clone());
            if inner.available + free >= task.RequiredSlots {
                return (true, tasks);
            }
        }
        (false, vec![])
    }
    /// 尝试占用 slot；若需先释放低优任务则返回 false（由上层先 Cancel）。
    pub fn alloc(&self, task: &TaskBase) -> bool {
        let mut i = self.inner.lock().expect("slot lock poisoned");
        let (can, free) = Self::canAlloc0(&i, task);
        if !can || !free.is_empty() {
            return false;
        }
        i.available -= task.RequiredSlots;
        i.tasks.push(task.clone());
        // 保持低优先级在前，便于 canAlloc 扫描可抢占集合。
        i.tasks.sort_by(|a, b| b.Compare(a).cmp(&0));
        Self::rebuild(&mut i);
        true
    }
    /// 释放某任务占用的 slot。
    pub fn free(&self, id: i64) {
        let mut i = self.inner.lock().expect("slot lock poisoned");
        let Some(idx) = i.index.get(&id).copied() else {
            return;
        };
        i.available += i.tasks[idx].RequiredSlots;
        i.tasks.remove(idx);
        Self::rebuild(&mut i)
    }
    /// 只读评估：是否可分配，以及需要先释放的任务列表。
    pub fn canAlloc(&self, task: &TaskBase) -> (bool, Vec<TaskBase>) {
        Self::canAlloc0(&self.inner.lock().expect("slot lock poisoned"), task)
    }
    /// 就地调整已运行任务的 RequiredSlots（扩缩容）；容量不足则失败。
    pub fn exchange(&self, new_task: &TaskBase) -> bool {
        let mut i = self.inner.lock().expect("slot lock poisoned");
        let Some(idx) = i.index.get(&new_task.ID).copied() else {
            return false;
        };
        let delta = new_task.RequiredSlots - i.tasks[idx].RequiredSlots;
        if delta > 0 && i.available < delta {
            return false;
        }
        i.available -= delta;
        i.tasks[idx] = new_task.clone();
        true
    }
    /// 剩余可用 slot。
    pub fn availableSlots(&self) -> i32 {
        self.inner.lock().expect("slot lock poisoned").available
    }
    /// 已使用 slot = 容量 − 剩余。
    pub fn usedSlots(&self) -> i32 {
        self.capacity - self.availableSlots()
    }
}
#[cfg(test)]
impl slotManager {
    /// Exposes the internal, priority-sorted task list, mirroring Go's
    /// white-box `sm.executorTasks` assertions.
    /// 测试用：暴露内部按优先级排序的任务列表。
    pub fn TasksForTest(&self) -> Vec<TaskBase> {
        self.inner.lock().expect("slot lock poisoned").tasks.clone()
    }
}
