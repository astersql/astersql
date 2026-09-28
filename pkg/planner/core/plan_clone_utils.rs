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

// 执行计划克隆辅助工具。
//
// 提供访问路径（Access Path）深拷贝、Point Get（点查）计划为计划缓存
// 做的快速克隆，以及支持缓存的逻辑子树克隆。点查指按主键/唯一键
// 精确命中单行的物理计划。

use std::collections::HashSet;

use crate::{PlanKind, PlanNode, PlannerContext};

/// 索引/表访问路径描述，含范围、过滤后行数估计及部分备选路径。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AccessPath {
    pub index_name: String,
    pub ranges: Vec<String>,
    pub count_after_access: u64,
    pub count_after_index: u64,
    pub partial_alternative_index_paths: Vec<AccessPath>,
}

/// 仅复制 AccessPath 的结构身份；分析结果留空，供统计推导重新生成。
pub fn freshAccessPath(source: &AccessPath) -> AccessPath {
    AccessPath {
        index_name: source.index_name.clone(),
        ..AccessPath::default()
    }
}

/// Point Get 计划快照：上下文、计划树、访问路径、分区名与句柄/索引值。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PointGetPlan {
    pub ctx: PlannerContext,
    pub plan: Option<PlanNode>,
    pub access_path: Option<AccessPath>,
    pub partition_names: Vec<String>,
    pub handles: Vec<i64>,
    pub index_values: Vec<Vec<String>>,
}

/// 为计划缓存快速克隆 Point Get 计划，并替换为新的 PlannerContext。
///
/// 访问路径通过 Rust 所有权深拷贝，避免与源计划共享可变状态。
pub fn FastClonePointGetForPlanCache(
    new_ctx: PlannerContext,
    source: &PointGetPlan,
    destination: &mut PointGetPlan,
) -> PointGetPlan {
    *destination = source.clone();
    destination.ctx = new_ctx;
    destination.access_path = source.access_path.clone();
    destination.partition_names = source.partition_names.clone();
    destination.handles = source.handles.clone();
    destination.index_values = source.index_values.clone();
    destination.clone()
}

/// 判断逻辑算子种类是否支持子树克隆（DataSource/Join/Selection 等常见节点）。
fn supported_logical_kind(kind: &PlanKind) -> bool {
    matches!(
        kind,
        PlanKind::DataSource { .. }
            | PlanKind::Join { .. }
            | PlanKind::Selection { .. }
            | PlanKind::Projection
            | PlanKind::Aggregation { .. }
            | PlanKind::Limit { .. }
            | PlanKind::Sort
            | PlanKind::TopN { .. }
    )
}

/// 克隆支持列表内的逻辑子树；遇到不支持节点则返回 `(None, false)`。
pub fn cloneLogicalSubtree(plan: &PlanNode) -> (Option<PlanNode>, bool) {
    let mut used_ids = HashSet::new();
    collect_plan_ids(plan, &mut used_ids);
    let mut next_id = used_ids
        .iter()
        .copied()
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .unwrap_or(i32::MIN);
    clone_logical_subtree_with_ids(plan, &mut used_ids, &mut next_id)
}

fn clone_logical_subtree_with_ids(
    plan: &PlanNode,
    used_ids: &mut HashSet<i32>,
    next_id: &mut i32,
) -> (Option<PlanNode>, bool) {
    if !supported_logical_kind(&plan.kind) {
        return (None, false);
    }
    let (children, ok) = cloneWithChildren(&plan.children, used_ids, next_id);
    if !ok {
        return (None, false);
    }
    let mut cloned = plan.clone();
    cloned.id = allocate_fresh_id(used_ids, next_id);
    cloned.children = children;
    (Some(cloned), true)
}

/// 逐子节点递归克隆；任一失败则整体失败并返回空向量。
fn cloneWithChildren(
    children: &[PlanNode],
    used_ids: &mut HashSet<i32>,
    next_id: &mut i32,
) -> (Vec<PlanNode>, bool) {
    let mut cloned = Vec::with_capacity(children.len());
    for child in children {
        let (next, ok) = clone_logical_subtree_with_ids(child, used_ids, next_id);
        if !ok {
            return (Vec::new(), false);
        }
        cloned.push(next.expect("successful clone returns a plan"));
    }
    (cloned, true)
}

fn collect_plan_ids(plan: &PlanNode, ids: &mut HashSet<i32>) {
    ids.insert(plan.id);
    for child in &plan.children {
        collect_plan_ids(child, ids);
    }
}

fn allocate_fresh_id(used_ids: &mut HashSet<i32>, next_id: &mut i32) -> i32 {
    while used_ids.contains(next_id) {
        *next_id = next_id.wrapping_add(1);
    }
    let allocated = *next_id;
    used_ids.insert(allocated);
    *next_id = next_id.wrapping_add(1);
    allocated
}
