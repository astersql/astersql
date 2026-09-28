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

// 从物理执行计划反推优化器 Hint 的工具。
//
// Hint（优化器提示）用于指定连接算法、聚合方式或存储引擎等。本模块遍历
// 扁平或树形物理计划，生成可回写到 SQL 的 Hint 列表（如 `HASH_JOIN`、`LEADING`）。

use crate::{FlatPhysicalPlan, PlanKind, PlanNode, StoreType};
use std::collections::BTreeSet;
/// 单个优化器 Hint：名称、涉及表名与可选存储引擎。
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct OptimizerHint {
    pub name: String,
    pub tables: Vec<String>,
    pub store: Option<String>,
}
/// 从扁平物理计划（FlatPhysicalPlan）主路径算子生成 Hint 集合。
pub fn GenHintsFromFlatPlan(flat: &FlatPhysicalPlan) -> Vec<OptimizerHint> {
    let mut hints = BTreeSet::new();
    for operator in &flat.Main {
        genHintsFromSingle(&operator.Origin, operator.StoreType, &mut hints);
    }
    hints.into_iter().collect()
}
/// 从树形物理计划递归收集 Hint；根为 Hash/Merge Join 时额外生成 `LEADING`。
pub fn GenHintsFromPhysicalPlan(plan: &PlanNode) -> Vec<OptimizerHint> {
    let mut hints = BTreeSet::new();
    collect(plan, &mut hints);
    // 根节点为 Hash/Merge Join 且多表时补充 LEADING 提示连接顺序。
    if matches!(
        plan.kind,
        PlanKind::HashJoin { .. } | PlanKind::MergeJoin { .. }
    ) {
        let tables = extractOrderedPhysicalJoinGroup(plan);
        if tables.len() > 2 {
            hints.insert(OptimizerHint {
                name: "LEADING".into(),
                tables,
                store: None,
            });
        }
    }
    hints.into_iter().collect()
}
/// 深度优先遍历计划树，对每个节点调用 `genHintsFromSingle`。
fn collect(plan: &PlanNode, hints: &mut BTreeSet<OptimizerHint>) {
    genHintsFromSingle(plan, plan.store_type, hints);
    for child in &plan.children {
        collect(child, hints);
    }
}
/// 按算子种类映射为对应 Hint（连接/聚合/读存储）。
fn genHintsFromSingle(plan: &PlanNode, store: StoreType, hints: &mut BTreeSet<OptimizerHint>) {
    match &plan.kind {
        PlanKind::HashJoin { .. } => {
            hints.insert(OptimizerHint {
                name: "HASH_JOIN".into(),
                tables: join_tables(plan),
                store: None,
            });
        }
        PlanKind::MergeJoin { .. } => {
            hints.insert(OptimizerHint {
                name: "MERGE_JOIN".into(),
                tables: join_tables(plan),
                store: None,
            });
        }
        PlanKind::IndexJoin { .. } => {
            hints.insert(OptimizerHint {
                name: "INL_JOIN".into(),
                tables: join_tables(plan),
                store: None,
            });
        }
        PlanKind::IndexMergeJoin { .. } => {
            hints.insert(OptimizerHint {
                name: "INL_MERGE_JOIN".into(),
                tables: join_tables(plan),
                store: None,
            });
        }
        PlanKind::IndexHashJoin { .. } => {
            hints.insert(OptimizerHint {
                name: "INL_HASH_JOIN".into(),
                tables: join_tables(plan),
                store: None,
            });
        }
        PlanKind::HashAgg => {
            hints.insert(OptimizerHint {
                name: "HASH_AGG".into(),
                tables: Vec::new(),
                store: None,
            });
        }
        PlanKind::StreamAgg => {
            hints.insert(OptimizerHint {
                name: "STREAM_AGG".into(),
                tables: Vec::new(),
                store: None,
            });
        }
        PlanKind::TableScan { table } => {
            hints.insert(OptimizerHint {
                name: "READ_FROM_STORAGE".into(),
                tables: vec![table.clone()],
                store: Some(format!("{:?}", store).to_lowercase()),
            });
        }
        _ => {}
    }
}
/// 从连接子节点提取表（别）名列表。
fn join_tables(plan: &PlanNode) -> Vec<String> {
    plan.children
        .iter()
        .filter_map(extractTableAsName)
        .collect()
}
/// 在 TableScan/DataSource 上取表名，否则向下递归查找。
fn extractTableAsName(plan: &PlanNode) -> Option<String> {
    if plan.children.len() > 1 {
        return None;
    }
    match &plan.kind {
        PlanKind::TableScan { table } | PlanKind::DataSource { table, .. } => Some(table.clone()),
        _ => plan.children.iter().find_map(extractTableAsName),
    }
}
/// 按物理 Join 树中序收集表名，供 `LEADING` Hint 使用。
fn extractOrderedPhysicalJoinGroup(plan: &PlanNode) -> Vec<String> {
    fn visit(plan: &PlanNode) -> Option<Vec<String>> {
        match &plan.kind {
            PlanKind::HashJoin {
                equal_conditions, ..
            } if equal_conditions.is_empty() => return None,
            PlanKind::MergeJoin { join_type, .. }
                if !matches!(
                    join_type,
                    crate::JoinType::InnerJoin
                        | crate::JoinType::LeftOuterJoin
                        | crate::JoinType::RightOuterJoin
                ) =>
            {
                return None;
            }
            _ => {}
        }

        if matches!(
            plan.kind,
            PlanKind::HashJoin { .. } | PlanKind::MergeJoin { .. }
        ) {
            if plan.children.len() != 2 {
                return None;
            }
            let left_is_join = matches!(
                plan.children[0].kind,
                PlanKind::HashJoin { .. } | PlanKind::MergeJoin { .. }
            );
            let right_is_join = matches!(
                plan.children[1].kind,
                PlanKind::HashJoin { .. } | PlanKind::MergeJoin { .. }
            );
            if left_is_join && right_is_join {
                return None;
            }
            if left_is_join || right_is_join {
                let join_child = if left_is_join { 0 } else { 1 };
                let leaf_child = 1 - join_child;
                let mut tables = visit(&plan.children[join_child])?;
                if tables.len() < 2 {
                    return None;
                }
                tables.push(extractTableAsName(&plan.children[leaf_child])?);
                return Some(tables);
            }
            return Some(vec![
                extractTableAsName(&plan.children[0])?,
                extractTableAsName(&plan.children[1])?,
            ]);
        }
        None
    }
    visit(plan).unwrap_or_default()
}
