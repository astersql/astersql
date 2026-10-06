// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 调度状态查询：汇总运行中任务、节点 CPU、忙碌节点与调度标志。
//
// `CalculateRequiredNodes` 按任务 RequiredSlots/MaxNodeCount 做装箱式节点估算；
// ImportInto 的冲突收集/解决/后处理步骤固定只需 1 个节点。

use crate::handle::{Context, Result, runtime};
use crate::{proto, schstatus, storage};
use std::collections::HashMap;
use std::time::SystemTime;

/// 组装完整调度状态：任务队列长度、TiDB/TiKV Worker 节点组与 Flags。
pub fn GetScheduleStatus(ctx: &Context) -> Result<schstatus::Status> {
    let runtime = runtime()?;
    let tasks = runtime
        .get_task_bases_in_states(ctx, &[proto::TaskStateRunning, proto::TaskStateModifying])?;
    let (node_count, node_cpu) = GetNodesInfo(ctx)?;
    let busy_nodes = GetBusyNodes(ctx)?;
    let flags = GetScheduleFlags(ctx)?;
    let required_nodes = CalculateRequiredNodes(&tasks, node_cpu);
    Ok(schstatus::Status {
        Version: schstatus::Version1,
        TaskQueue: schstatus::TaskQueue {
            ScheduledCount: i32::try_from(tasks.len()).unwrap_or(i32::MAX),
        },
        TiDBWorker: schstatus::NodeGroup {
            CPUCount: node_cpu,
            RequiredCount: required_nodes,
            CurrentCount: node_count,
            BusyNodes: busy_nodes,
        },
        TiKVWorker: schstatus::NodeGroup {
            RequiredCount: required_nodes,
            ..Default::default()
        },
        Flags: flags,
    })
}

/// 返回活跃任务摘要。
pub fn GetActiveTaskSummary(ctx: &Context) -> Result<storage::ActiveTaskSummary> {
    runtime()?.get_active_task_summary(ctx)
}

/// 返回注册在 `mysql.dist_framework_meta` 中的全部受管节点。
pub fn ListManagedNodes(ctx: &Context) -> Result<Vec<proto::ManagedNode>> {
    runtime()?.get_all_nodes(ctx)
}

/// 分页列出历史任务（可按 keyspace 过滤）。
pub fn ListHistoryTasks(
    ctx: &Context,
    page_size: i32,
    page_token: i64,
    keyspace: &str,
) -> Result<storage::HistoryTaskPage> {
    runtime()?.list_history_tasks(ctx, page_size, page_token, keyspace)
}

/// 返回 (节点数, 单节点 CPU)；无节点时 CPU 回退到 local_cpu_count。
pub fn GetNodesInfo(ctx: &Context) -> Result<(i32, i32)> {
    let runtime = runtime()?;
    let nodes = runtime.get_all_nodes(ctx)?;
    let cpu_count = nodes
        .first()
        .map(|node| node.CPUCount)
        .unwrap_or_else(|| runtime.local_cpu_count());
    Ok((i32::try_from(nodes.len()).unwrap_or(i32::MAX), cpu_count))
}

/// 忙碌节点列表，并标记 Owner 执行节点（若不在列表则追加）。
pub fn GetBusyNodes(ctx: &Context) -> Result<Vec<schstatus::Node>> {
    let runtime = runtime()?;
    let owner_exec_id = runtime.owner_exec_id(ctx)?;
    let mut busy_nodes = runtime.get_busy_nodes(ctx)?;
    if let Some(owner) = busy_nodes.iter_mut().find(|node| node.ID == owner_exec_id) {
        owner.IsOwner = true;
    } else {
        busy_nodes.push(schstatus::Node {
            ID: owner_exec_id,
            IsOwner: true,
        });
    }
    Ok(busy_nodes)
}

/// 按任务槽位需求估算所需节点数；至少返回 1。
pub fn CalculateRequiredNodes(tasks: &[proto::TaskBase], cpu_count: i32) -> i32 {
    let mut available_resources: Vec<i32> = Vec::with_capacity(tasks.len());
    // 先尝试填入已有节点剩余槽位，不够再新开节点。
    for task in tasks {
        let mut needed = getNeededNodes(task);
        for available in &mut available_resources {
            if needed <= 0 {
                break;
            }
            if *available >= task.RequiredSlots {
                *available -= task.RequiredSlots;
                needed -= 1;
            }
        }
        for _ in 0..needed {
            available_resources.push(cpu_count - task.RequiredSlots);
        }
    }
    i32::try_from(available_resources.len())
        .unwrap_or(i32::MAX)
        .max(1)
}

/// ImportInto 特定后置步骤只需 1 节点，其余用 MaxNodeCount。
pub(crate) fn getNeededNodes(task: &proto::TaskBase) -> i32 {
    if task.Type == proto::ImportInto
        && matches!(
            task.Step,
            proto::ImportStepCollectConflicts
                | proto::ImportStepConflictResolution
                | proto::ImportStepPostProcess
        )
    {
        1
    } else {
        task.MaxNodeCount
    }
}

/// 收集已启用的调度标志（当前仅 PauseScaleIn）。
pub fn GetScheduleFlags(ctx: &Context) -> Result<HashMap<schstatus::Flag, schstatus::TTLFlag>> {
    let mut flags = HashMap::new();
    let pause_scale_in = getPauseScaleInFlag(ctx)?;
    if pause_scale_in.Enabled {
        flags.insert(schstatus::PauseScaleInFlag.to_owned(), pause_scale_in);
    }
    Ok(flags)
}

/// 读取暂停缩容标志；已过期则视为默认（关闭）。
fn getPauseScaleInFlag(ctx: &Context) -> Result<schstatus::TTLFlag> {
    let mut flag = runtime()?.get_pause_scale_in_flag(ctx)?.unwrap_or_default();
    if flag.Enabled && flag.TTLInfo.ExpireTime < SystemTime::now() {
        flag = schstatus::TTLFlag::default();
    }
    Ok(flag)
}
