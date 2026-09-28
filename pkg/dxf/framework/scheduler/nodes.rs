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

// 节点管理：维护存活执行器视图与框架 managed 节点缓存。
//
// 存活视图用于发现并删除死节点；managed 视图供调度选节点，
// 并用首个非零 CPUCount 刷新 SlotManager 容量。

use crate::interface::{ManagedNode, Result, TaskManager};
use crate::slots::SlotManager;
use std::collections::HashSet;
use std::sync::{Arc, RwLock};

/// 与 Go 一致维护两套视图：服务发现快照（找死节点）与框架 managed 节点快照（调度用）。
/// Keeps the same two views as Go: the service-discovery snapshot used to find
/// dead executors and the framework-managed node snapshot used for scheduling.
#[derive(Default)]
pub struct NodeManager {
    /// 上一轮存活 ExecID 集合。
    previous_live_nodes: RwLock<HashSet<String>>,
    /// 当前 managed 节点列表快照。
    nodes: RwLock<Arc<Vec<ManagedNode>>>,
}

impl NodeManager {
    /// 构造空 NodeManager。
    pub fn new() -> Self {
        Self::default()
    }

    /// 对比当前存活 ExecID 与上次快照；对已不在存活集合中的 managed 节点执行删除。
    /// 仅在删除成功后更新 previous_live_nodes，失败则下轮重试。
    pub fn maintain_live_nodes(
        &self,
        task_manager: &dyn TaskManager,
        live_exec_ids: &[String],
    ) -> Result<Vec<String>> {
        // 与上次集合相同则无需访问 TaskManager。
        let current: HashSet<String> = live_exec_ids.iter().cloned().collect();
        let mut previous = self
            .previous_live_nodes
            .write()
            .expect("live-node lock poisoned");
        if *previous == current {
            return Ok(Vec::new());
        }

        // managed 中不在当前存活集合的节点视为死节点。
        let dead_nodes = task_manager
            .all_nodes()?
            .into_iter()
            .filter(|node| !current.contains(&node.id))
            .map(|node| node.id)
            .collect::<Vec<_>>();
        if !dead_nodes.is_empty() {
            // As in Go, publish the discovery snapshot only after deletion has
            // succeeded, so a failed storage call is retried on the next tick.
            task_manager.delete_dead_nodes(&dead_nodes)?;
        }
        *previous = current;
        Ok(dead_nodes)
    }

    /// 从 TaskManager 拉取全部节点，刷新缓存，并用首个正 CPU 数更新槽位容量。
    pub fn refresh_nodes(
        &self,
        task_manager: &dyn TaskManager,
        slot_manager: &SlotManager,
    ) -> Result<Vec<ManagedNode>> {
        let new_nodes = task_manager.all_nodes()?;
        // 对齐 Go：用第一个非零 CPUCount 作为全局 slot capacity。
        if let Some(cpu_count) = new_nodes
            .iter()
            .map(|node| node.cpu_count)
            .find(|cpu| *cpu > 0)
        {
            slot_manager.update_capacity(cpu_count);
        }
        *self.nodes.write().expect("managed-node lock poisoned") = Arc::new(new_nodes.clone());
        Ok(new_nodes)
    }

    /// 返回节点列表的稳定克隆，对齐 Go slices.Clone。
    /// Returns a stable snapshot, matching Go's slices.Clone contract.
    pub fn get_nodes(&self) -> Vec<ManagedNode> {
        self.nodes
            .read()
            .expect("managed-node lock poisoned")
            .as_ref()
            .clone()
    }

    /// 测试/注入用：直接覆盖 managed 节点缓存。
    pub fn set_nodes(&self, nodes: Vec<ManagedNode>) {
        *self.nodes.write().expect("managed-node lock poisoned") = Arc::new(nodes);
    }
}

/// 按 target_scope 过滤节点 ID。
/// 若 scope 为空且存在 role=background 的节点，则默认只选 background；
/// 否则选 role 与有效 scope 完全相等的节点。
pub fn filter_by_scope(nodes: &[ManagedNode], target_scope: &str) -> Vec<String> {
    // 空 scope + 存在 background 时，有效 scope 设为 background。
    let have_background = nodes.iter().any(|node| node.role == "background");
    let effective_scope = if target_scope.is_empty() && have_background {
        "background"
    } else {
        target_scope
    };
    nodes
        .iter()
        .filter(|node| node.role == effective_scope)
        .map(|node| node.id.clone())
        .collect()
}
