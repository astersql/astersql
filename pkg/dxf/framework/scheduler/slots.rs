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

// 调度槽位（slot）与条带（stripe）资源管理。
//
// DXF（Distributed eXecution Framework）将每个节点的 CPU 核抽象为 slot：
// 一核一槽。stripe 表示在每个托管节点上各占一个 slot 的资源组。
// `SlotManager` 负责跟踪已用 slot、按任务排名预留 stripe / 单节点 slot，
// 以及过滤容量不足的候选执行节点。
//
// `next_gen` 模式下不在调度层预留 slot（由集群控制器按需扩容）。

use crate::interface::{Result, TaskBase, TaskManager};
use crate::nodes::NodeManager;
use std::collections::HashMap;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

/// 某任务占用的 stripe 预留：任务元数据 + 所需 stripe 数。
#[derive(Clone)]
struct TaskStripes {
    /// 被预留资源的任务。
    task: TaskBase,
    /// 该任务占用的 stripe 数量（通常等于 `required_slots`）。
    stripes: i32,
}

/// 预留视图：按排名排序的 stripe 列表、任务到下标映射、按执行节点累计的 slot。
#[derive(Default)]
struct Reservations {
    /// 已预留 stripe，按任务排名（rank）升序排列。
    stripes: Vec<TaskStripes>,
    /// 任务 ID → `stripes` 向量下标，便于快速 unreserve。
    task_to_index: HashMap<i64, usize>,
    /// 按执行节点累计的单节点 slot 预留。
    slots: HashMap<String, i32>,
}

/// Slot is one core on one node; stripe is one slot on every managed node.
/// Both reservation maps share one lock so callers never observe half an
/// update, while used slots are replaced as one independently refreshed view.
///
/// 槽位管理器：维护容量、预留与已用 slot。
/// Slot = 单节点上一核；stripe = 每个托管节点上各一槽。预留表共享一把锁，
/// 避免调用方观察到半更新；已用 slot 则作为独立快照整表替换。
pub struct SlotManager {
    /// 单节点可用 slot 容量（通常等于 CPU 核数）。
    capacity: AtomicI32,
    /// 是否启用 next-gen：为 true 时跳过本地 slot 预留。
    next_gen: AtomicBool,
    /// stripe / 单节点预留表。
    reservations: RwLock<Reservations>,
    /// 各执行节点当前已用 slot 的快照。
    used_slots: RwLock<HashMap<String, i32>>,
}

impl Default for SlotManager {
    fn default() -> Self {
        // 默认容量取本机可用并行度（核数），获取失败则退回 1。
        let cpu = std::thread::available_parallelism()
            .map(|count| count.get() as i32)
            .unwrap_or(1);
        Self {
            capacity: AtomicI32::new(cpu),
            next_gen: AtomicBool::new(false),
            reservations: RwLock::new(Reservations::default()),
            used_slots: RwLock::new(HashMap::new()),
        }
    }
}

impl SlotManager {
    /// 构造默认 `SlotManager`（容量为本机核数）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置是否启用 next-gen 调度（跳过本地预留）。
    pub fn set_next_gen(&self, enabled: bool) {
        self.next_gen.store(enabled, Ordering::Release);
    }

    /// 根据节点管理器与任务管理器刷新各节点已用 slot 快照。
    pub fn update(&self, node_manager: &NodeManager, task_manager: &dyn TaskManager) -> Result<()> {
        let reported = task_manager.used_slots_on_nodes()?;
        // 仅保留当前仍被托管的节点；缺失上报的节点按 0 已用处理。
        let snapshot = node_manager
            .get_nodes()
            .into_iter()
            .map(|node| {
                let used = reported.get(&node.id).copied().unwrap_or_default();
                (node.id, used)
            })
            .collect();
        *self.used_slots.write().expect("used-slot lock poisoned") = snapshot;
        Ok(())
    }

    /// 直接覆盖已用 slot 快照（测试或外部注入用）。
    pub fn set_used_slots(&self, slots: HashMap<String, i32>) {
        *self.used_slots.write().expect("used-slot lock poisoned") = slots;
    }

    /// First tries a stripe reservation after accounting for all higher-ranked
    /// tasks. If it does not fit, Go falls back to a minimum-resource slot on a
    /// single executor and returns that executor ID.
    ///
    /// 先按 stripe 判断（扣除更高排名任务的预留）；若放不下，再回退到单执行节点
    /// 的最小资源 slot，并返回该执行节点 ID。返回 `(exec_id, ok)`：
    /// `ok=false` 表示无法预留；stripe 成功时 `exec_id` 为空串。
    pub fn can_reserve(&self, task: &TaskBase) -> (String, bool) {
        // next-gen：调度层不预留，直接视为可调度。
        if self.next_gen.load(Ordering::Acquire) {
            return (String::new(), true);
        }
        let used = self.used_slots.read().expect("used-slot lock poisoned");
        // 尚无节点用量信息时无法判定，拒绝预留。
        if used.is_empty() {
            return (String::new(), false);
        }
        let reservations = self.reservations.read().expect("reservation lock poisoned");
        let capacity = self.capacity();
        // 累加排名更高（更优先）任务已预留的 stripe。
        let reserved_for_higher_rank = reservations
            .stripes
            .iter()
            .take_while(|entry| entry.task.compare(task).is_lt())
            .map(|entry| entry.stripes)
            .sum::<i32>();
        if task.required_slots + reserved_for_higher_rank <= capacity {
            return (String::new(), true);
        }
        // stripe 放不下：尝试绑定到仍有空闲容量的具体执行节点。
        for (exec_id, used_slots) in used.iter() {
            let reserved = reservations.slots.get(exec_id).copied().unwrap_or_default();
            if used_slots + reserved + task.required_slots <= capacity {
                return (exec_id.clone(), true);
            }
        }
        (String::new(), false)
    }

    /// 为任务登记预留；`exec_id` 非空时同时累加该节点的 slot 预留。
    pub fn reserve(&self, task: &TaskBase, exec_id: &str) {
        if self.next_gen.load(Ordering::Acquire) {
            return;
        }
        let mut reservations = self
            .reservations
            .write()
            .expect("reservation lock poisoned");
        reservations.stripes.push(TaskStripes {
            task: task.clone(),
            stripes: task.required_slots,
        });
        // 按任务排名重排，保证 can_reserve 的 take_while 语义正确。
        reservations
            .stripes
            .sort_by(|left, right| left.task.compare(&right.task));
        rebuild_task_indexes(&mut reservations);
        if !exec_id.is_empty() {
            *reservations.slots.entry(exec_id.to_owned()).or_default() += task.required_slots;
        }
    }

    /// 释放任务预留；若节点预留归零则移除该节点条目。
    pub fn unreserve(&self, task: &TaskBase, exec_id: &str) {
        if self.next_gen.load(Ordering::Acquire) {
            return;
        }
        let mut reservations = self
            .reservations
            .write()
            .expect("reservation lock poisoned");
        let Some(index) = reservations.task_to_index.get(&task.id).copied() else {
            return;
        };
        reservations.stripes.remove(index);
        rebuild_task_indexes(&mut reservations);
        if !exec_id.is_empty() {
            let remove = if let Some(slots) = reservations.slots.get_mut(exec_id) {
                *slots -= task.required_slots;
                *slots == 0
            } else {
                false
            };
            if remove {
                reservations.slots.remove(exec_id);
            }
        }
    }

    /// 返回当前单节点 slot 容量。
    pub fn capacity(&self) -> i32 {
        self.capacity.load(Ordering::Acquire)
    }

    /// 更新容量；`cpu_count <= 0` 时忽略，避免把有效容量清零。
    pub fn update_capacity(&self, cpu_count: i32) {
        if cpu_count > 0 {
            self.capacity.store(cpu_count, Ordering::Release);
        }
    }

    /// 在候选节点中优先留下容量足够的节点；若无一足够则原样返回候选列表。
    pub fn adjust_eligible_nodes(
        &self,
        eligible_nodes: Vec<String>,
        required_slots: i32,
    ) -> Vec<String> {
        let used = self.used_slots.read().expect("used-slot lock poisoned");
        let enough =
            filter_nodes_with_enough_slots(&used, self.capacity(), &eligible_nodes, required_slots);
        if enough.is_empty() {
            eligible_nodes
        } else {
            enough
        }
    }
}

/// 根据当前 `stripes` 向量重建 `task_to_index` 映射。
fn rebuild_task_indexes(reservations: &mut Reservations) {
    reservations.task_to_index.clear();
    let indexes = reservations
        .stripes
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.task.id, index))
        .collect::<Vec<_>>();
    reservations.task_to_index.extend(indexes);
}

/// 从候选节点中筛选「已用 + 需求 <= 容量」的节点。
pub fn filter_nodes_with_enough_slots(
    used_slots: &HashMap<String, i32>,
    capacity: i32,
    eligible_nodes: &[String],
    required_slots: i32,
) -> Vec<String> {
    eligible_nodes
        .iter()
        .filter(|node| {
            used_slots
                .get(*node)
                .is_some_and(|used| used + required_slots <= capacity)
        })
        .cloned()
        .collect()
}
