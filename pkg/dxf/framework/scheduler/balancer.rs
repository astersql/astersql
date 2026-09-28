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

// DXF 子任务负载均衡：按调度器排名轮询，把 pending subtask 摊到合格节点。
//
// 每个 tick 累加各节点已用槽位，供后续任务过滤“槽位足够”的节点；
// 若某任务无可调度节点则故意不记账，让低优先级任务仍有机会。

use crate::interface::*;
use crate::nodes::filter_by_scope;
use crate::slots::filter_nodes_with_enough_slots;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// 子任务均衡器：持有调度参数与本轮已用槽位账本。
pub struct Balancer {
    /// 调度共享参数（节点管理、任务管理、槽位管理等）。
    pub param: Param,
    /// 本轮 balance 中各节点累计占用的槽位数。
    current_used_slots: HashMap<String, i32>,
}

impl Balancer {
    /// 创建均衡器。
    pub fn new(param: Param) -> Self {
        Self {
            param,
            current_used_slots: HashMap::new(),
        }
    }

    /// Balances in scheduler rank order and carries slot usage from each task
    /// into the next task, exactly as one Go balance tick does.
    /// 按 scheduler 排名依次均衡；节点用法在任务间传递。
    pub fn balance(&mut self, schedulers: &[Arc<dyn Scheduler>]) -> Result<()> {
        // 重置本轮账本：所有托管节点 used=0。
        let managed_nodes = self.param.node_manager.get_nodes();
        self.current_used_slots = managed_nodes
            .iter()
            .map(|node| (node.id.clone(), 0))
            .collect();
        for scheduler in schedulers {
            let task = scheduler.task();
            // 先按 TargetScope 过滤，再与扩展给出的 eligible 取交。
            let scoped = filter_by_scope(&managed_nodes, &task.base.target_scope);
            let application_nodes = scheduler.extension().eligible_instances(&task)?;
            let eligible = if application_nodes.is_empty() {
                scoped
            } else {
                let application_nodes: HashSet<_> = application_nodes.into_iter().collect();
                scoped
                    .into_iter()
                    .filter(|node| application_nodes.contains(node))
                    .collect()
            };
            // 无可用节点则整轮失败，与 Go 一致。
            if eligible.is_empty() {
                return Err(SchedulerError::new("no eligible nodes to balance subtasks"));
            }
            self.balance_subtasks(&task, eligible)?;
        }
        Ok(())
    }

    /// 对单个任务的活跃子任务做重分配并更新 used slots。
    pub fn balance_subtasks(&mut self, task: &Task, eligible_nodes: Vec<String>) -> Result<()> {
        let mut subtasks = self.param.task_manager.active_subtasks(task.base.id)?;
        if subtasks.is_empty() {
            return Ok(());
        }
        // 槽位足够且不超过 MaxNodeCount；无可跑节点则跳过记账。
        let enough = filter_nodes_with_enough_slots(
            &self.current_used_slots,
            self.param.slot_manager.capacity(),
            &eligible_nodes,
            task.base.required_slots,
        );
        let adjusted = filter_nodes_by_max_node_count(enough, &subtasks, task.base.max_node_count);
        if adjusted.is_empty() {
            // No node can run this task. Go deliberately does not account its
            // used slots so lower-ranked tasks still get a chance this tick.
            return Ok(());
        }
        // 仅改写 pending 的 ExecID，写回存储后再累计 used。
        let changed = rebalance_pending_subtasks(&adjusted, &subtasks);
        if !changed.is_empty() {
            let changed_ids: HashMap<i64, String> = changed
                .iter()
                .map(|subtask| (subtask.id, subtask.exec_id.clone()))
                .collect();
            for subtask in &mut subtasks {
                if let Some(exec_id) = changed_ids.get(&subtask.id) {
                    subtask.exec_id.clone_from(exec_id);
                }
            }
            self.param.task_manager.update_subtask_exec_ids(&changed)?;
        }
        self.update_used_nodes(&task.base, &subtasks);
        Ok(())
    }

    /// 每个出现过的 exec_id 累加 RequiredSlots。
    fn update_used_nodes(&mut self, task: &TaskBase, subtasks: &[SubtaskBase]) {
        let nodes: HashSet<_> = subtasks
            .iter()
            .map(|subtask| subtask.exec_id.clone())
            .collect();
        for node in nodes {
            *self.current_used_slots.entry(node).or_default() += task.required_slots;
        }
    }
}

/// 按已有 subtask 分布优先保留忙碌节点，再截断到 max_node_count。
pub fn filter_nodes_by_max_node_count(
    mut nodes: Vec<String>,
    subtasks: &[SubtaskBase],
    max_node_count: i32,
) -> Vec<String> {
    if max_node_count == 0 || nodes.len() <= max_node_count as usize {
        return nodes;
    }
    let mut counts = HashMap::<&str, usize>::new();
    for subtask in subtasks {
        *counts.entry(&subtask.exec_id).or_default() += 1;
    }
    // 按该节点上已有 subtask 数降序，稳定排序对齐 Go SliceStable。
    // slice::sort_by is stable, matching sort.SliceStable in Go.
    nodes.sort_by(|left, right| {
        counts
            .get(right.as_str())
            .unwrap_or(&0)
            .cmp(counts.get(left.as_str()).unwrap_or(&0))
    });
    nodes.truncate(max_node_count as usize);
    nodes
}

// 把 pending 子任务在合格节点间均分；running 不搬迁；不合格节点上的可移动任务抽出重排。
fn rebalance_pending_subtasks(
    adjusted_nodes: &[String],
    subtasks: &[SubtaskBase],
) -> Vec<SubtaskBase> {
    // 目标：每节点 average 个，余数节点多 1 个。
    let average = subtasks.len() / adjusted_nodes.len();
    let mut remainder = subtasks.len() % adjusted_nodes.len();
    let adjusted_set: HashSet<&str> = adjusted_nodes.iter().map(String::as_str).collect();
    let mut groups = HashMap::<String, Vec<SubtaskBase>>::new();
    // running 放组首，避免后续 drain pending 时误动。
    let mut pending_counts = HashMap::<String, usize>::new();
    for subtask in subtasks {
        let group = groups.entry(subtask.exec_id.clone()).or_default();
        if subtask.state == SUBTASK_STATE_RUNNING {
            group.insert(0, subtask.clone());
        } else {
            group.push(subtask.clone());
            *pending_counts.entry(subtask.exec_id.clone()).or_default() += 1;
        }
    }
    for node in adjusted_nodes {
        groups.entry(node.clone()).or_default();
    }

    let mut need_schedule = Vec::new();
    let mut one_more = HashSet::<String>::new();
    let group_nodes = groups.keys().cloned().collect::<Vec<_>>();
    for node in group_nodes {
        let group = groups.get_mut(&node).expect("group was just indexed");
        // 不在合格集：Go 会把该节点上的全部 active subtask 迁走。节点已经
        // 失效或容量不足时，原先的 running 标记不能阻止 failover。
        if !adjusted_set.contains(node.as_str()) {
            need_schedule.append(group);
            continue;
        }

        // 超配额时只挪 pending，保留 running。
        let pending = pending_counts.get(&node).copied().unwrap_or_default();
        if remainder > 0 && group.len() >= average + 1 {
            let move_count = pending.min(group.len() - (average + 1));
            need_schedule.extend(group.drain(group.len() - move_count..));
            one_more.insert(node);
            remainder -= 1;
        } else if remainder == 0 && group.len() > average {
            let move_count = pending.min(group.len() - average);
            need_schedule.extend(group.drain(group.len() - move_count..));
        }
    }
    // 先确定哪些节点拿到 average+1（消耗 remainder）。
    if need_schedule.is_empty() {
        return Vec::new();
    }

    for node in adjusted_nodes {
        if remainder == 0 {
            break;
        }
        if one_more.insert(node.clone()) {
            remainder -= 1;
        }
    }

    // 把 need_schedule 填回未达标节点，并改写 exec_id。
    let mut fill_index = 0;
    for node in adjusted_nodes {
        let current = groups.get(node).map(Vec::len).unwrap_or_default();
        let target = average + usize::from(one_more.contains(node));
        for _ in current..target {
            if let Some(subtask) = need_schedule.get_mut(fill_index) {
                subtask.exec_id.clone_from(node);
                fill_index += 1;
            }
        }
    }
    need_schedule.truncate(fill_index);
    need_schedule
}

/// Go 风格导出名。
pub fn filterNodesByMaxNodeCnt(
    nodes: Vec<String>,
    subtasks: &[SubtaskBase],
    max_node_count: i32,
) -> Vec<String> {
    filter_nodes_by_max_node_count(nodes, subtasks, max_node_count)
}
