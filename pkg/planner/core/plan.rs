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

// 物理计划任务包装与 Shuffle 并行改写辅助。
//
// 提供 `PlanTask`（可标记无效的计划载体）、基于 TiFlash Shuffle 的
// Window / StreamAgg / MergeJoin 并行化，以及从 Probe 父节点估算/统计
// 探测（probe）次数的工具函数。

use crate::{PlanKind, PlanNode, PlannerContext, StoreType};

/// 从可选会话上下文取出 `PlannerContext`，缺失时返回错误。
pub fn AsSctx(ctx: Option<&PlannerContext>) -> Result<&PlannerContext, String> {
    ctx.ok_or_else(|| {
        "the current PlanContext cannot be converted to sessionctx.Context".to_owned()
    })
}

/// 持有可选物理计划的任务包装；`invalid` 为真表示该任务不可用。
#[derive(Clone, Debug, PartialEq)]
pub struct PlanTask {
    /// 当前物理计划；无效任务时为 `None`。
    pub plan: Option<PlanNode>,
    /// 是否标记为无效任务。
    pub invalid: bool,
}

impl PlanTask {
    /// 用给定计划构造有效任务。
    pub fn New(plan: PlanNode) -> Self {
        Self {
            plan: Some(plan),
            invalid: false,
        }
    }
    /// 构造无计划的无效任务。
    pub fn Invalid() -> Self {
        Self {
            plan: None,
            invalid: true,
        }
    }
}

/// Shuffle 并行配置：是否启用及并行流（stream）数量。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShuffleConfig {
    /// 是否启用 Shuffle 改写。
    pub enabled: bool,
    /// 并行流数量；小于等于 1 时不改写。
    pub stream_count: usize,
}

/// 在启用且流数大于 1 时，按算子类型对任务做 Shuffle 并行改写。
pub fn optimizeByShuffle(mut task: PlanTask, config: &ShuffleConfig) -> PlanTask {
    if task.invalid || task.plan.is_none() || !config.enabled || config.stream_count <= 1 {
        return task;
    }
    let plan = task.plan.take().expect("plan presence checked above");
    // Window / StreamAgg / MergeJoin 可按分区键拆成多路 TiFlash Shuffle。
    task.plan = Some(match plan.kind {
        PlanKind::Window { .. } => optimizeByShuffle4Window(plan, config.stream_count),
        PlanKind::StreamAgg => optimizeByShuffle4StreamAgg(plan, config.stream_count),
        PlanKind::MergeJoin { .. } => optimizeByShuffle4MergeJoin(plan, config.stream_count),
        _ => plan,
    });
    task
}

/// 在子计划与父算子之间插入 Shuffle Sender/Receiver（落点 TiFlash）。
fn wrap_shuffle(mut plan: PlanNode, keys: Vec<Vec<String>>, streams: usize) -> PlanNode {
    if plan.children.is_empty()
        || plan.children.len() != keys.len()
        || plan
            .children
            .iter()
            .any(|child| !matches!(child.kind, PlanKind::Sort) || child.children.is_empty())
    {
        return plan;
    }
    plan.children = plan
        .children
        .into_iter()
        .zip(keys)
        .map(|(tail, keys)| {
            let mut sender = PlanNode::New(
                -1,
                PlanKind::Shuffle {
                    info: format!("streams:{streams}, keys:{}", keys.join(",")),
                },
                vec![tail],
            );
            sender.store_type = StoreType::TiFlash;
            let mut receiver = PlanNode::New(
                -1,
                PlanKind::ShuffleReceiver {
                    info: format!("streams:{streams}"),
                },
                vec![sender],
            );
            receiver.store_type = StoreType::TiFlash;
            receiver
        })
        .collect();
    plan
}

fn ndv_limited_streams(plan: &PlanNode, configured: usize) -> Option<usize> {
    let sort = plan.children.first()?;
    if !matches!(sort.kind, PlanKind::Sort) {
        return None;
    }
    let ndv = sort.children.first()?.estimated_rows;
    if ndv <= 1.0 {
        return None;
    }
    Some(configured.min(ndv as usize))
}

fn probe_outer_child(parent: &PlanNode) -> Option<&PlanNode> {
    if !matches!(
        parent.kind,
        PlanKind::Apply
            | PlanKind::IndexJoin { .. }
            | PlanKind::IndexHashJoin { .. }
            | PlanKind::IndexMergeJoin { .. }
    ) {
        return None;
    }
    let outer = match parent.build_side? {
        0 => 1,
        1 => 0,
        _ => return None,
    };
    parent.children.get(outer)
}

/// 按窗口函数列表作为分区键，为 Window 计划插入 Shuffle。
pub fn optimizeByShuffle4Window(plan: PlanNode, streams: usize) -> PlanNode {
    let Some(streams) = ndv_limited_streams(&plan, streams) else {
        return plan;
    };
    let keys = match &plan.kind {
        PlanKind::Window { functions } => functions.clone(),
        _ => Vec::new(),
    };
    wrap_shuffle(plan, vec![keys], streams)
}

/// 为流式聚合（StreamAgg）按 group-by 键插入 Shuffle。
pub fn optimizeByShuffle4StreamAgg(plan: PlanNode, streams: usize) -> PlanNode {
    let Some(streams) = ndv_limited_streams(&plan, streams) else {
        return plan;
    };
    wrap_shuffle(plan, vec![vec!["group-by".to_owned()]], streams)
}

/// 为归并连接（MergeJoin）按左连接键插入 Shuffle。
pub fn optimizeByShuffle4MergeJoin(plan: PlanNode, streams: usize) -> PlanNode {
    let keys = match &plan.kind {
        PlanKind::MergeJoin { keys, .. } => vec![
            keys.iter().map(|(left, _)| left.clone()).collect(),
            keys.iter().map(|(_, right)| right.clone()).collect(),
        ],
        _ => return plan,
    };
    wrap_shuffle(plan, keys, streams)
}

/// 用各 Probe 父节点的估计 probe 次数连乘，得到总估计探测次数。
pub fn getEstimatedProbeCntFromProbeParents(probe_parents: &[PlanNode]) -> f64 {
    probe_parents.iter().fold(1.0, |count, parent| {
        probe_outer_child(parent).map_or(count, |outer| count * outer.estimated_rows)
    })
}

/// 用各 Probe 父节点的实际行数连乘，得到总实际探测次数。
pub fn getActualProbeCntFromProbeParents(probe_parents: &[PlanNode]) -> i64 {
    probe_parents.iter().fold(1_i64, |count, parent| {
        probe_outer_child(parent).map_or(count, |outer| {
            count.wrapping_mul(outer.actual_rows.unwrap_or(1) as i64)
        })
    })
}
