// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 执行计划（physical / logical plan）树的字符串化工具。
//
// 将 `PlanNode` 树渲染为可读的算子链（如 `Table(t)->Sel(..)->Projection`），
// 以及函数依赖（Functional Dependency, FD：列之间的决定关系）摘要。
// 主要用于 EXPLAIN、调试与单测断言。

use crate::{JoinType, PlanKind, PlanNode};

/// 将计划树自底向上串联为 `子算子->父算子` 形式的字符串。
pub fn ToString(plan: &PlanNode) -> String {
    let mut parts = Vec::new();
    toString(plan, &mut parts);
    parts.join("->")
}

/// 收集计划树上关键算子的函数依赖（FD），自底向上用 ` >>> ` 连接。
pub fn FDToString(plan: &PlanNode) -> String {
    let mut fds = Vec::new();
    fdToString(plan, &mut fds);
    // 深度优先先压入叶节点，再 reverse 得到自底向上顺序。
    fds.reverse();
    fds.join(" >>> ")
}

/// 多子节点或 UnionAll 时，子树描述需内嵌进当前算子，而非平铺链式拼接。
pub fn needIncludeChildrenString(plan: &PlanNode) -> bool {
    matches!(plan.kind, PlanKind::UnionAll { .. }) || plan.children.len() > 1
}

/// 递归收集 Projection/Agg/Join 等会维护 FD 信息的算子描述。
fn fdToString(plan: &PlanNode, output: &mut Vec<String>) {
    match plan.kind {
        PlanKind::Projection | PlanKind::Aggregation { .. } => {
            output.push(format!("{{{}}}", plan.fd));
            for child in &plan.children {
                fdToString(child, output);
            }
        }
        PlanKind::DataSource { .. }
        | PlanKind::Apply
        | PlanKind::Join { .. }
        | PlanKind::UnionAll { .. } => output.push(format!("{{{}}}", plan.fd)),
        _ => {}
    }
}

/// 将所有子计划分别 `ToString` 后用分隔符拼接。
fn child_string(plan: &PlanNode, separator: &str) -> String {
    plan.children
        .iter()
        .map(ToString)
        .collect::<Vec<_>>()
        .join(separator)
}

/// 格式化等值连接键对列表，如 `(a,b)(c,d)`。
fn keys_string(keys: &[(String, String)]) -> String {
    keys.iter()
        .map(|(left, right)| format!("({left},{right})"))
        .collect::<String>()
}

/// 按 Join 语义映射 MergeJoin 在 EXPLAIN 中的算子名。
fn merge_join_name(join_type: JoinType) -> &'static str {
    match join_type {
        JoinType::SemiJoin => "MergeSemiJoin",
        JoinType::AntiSemiJoin => "MergeAntiSemiJoin",
        JoinType::LeftOuterSemiJoin => "MergeLeftOuterSemiJoin",
        JoinType::AntiLeftOuterSemiJoin => "MergeAntiLeftOuterSemiJoin",
        JoinType::LeftOuterJoin => "MergeLeftOuterJoin",
        JoinType::RightOuterJoin => "MergeRightOuterJoin",
        JoinType::InnerJoin => "MergeInnerJoin",
    }
}

/// 按 `PlanKind` 生成当前节点的短描述（不含祖先链）。
fn describe(plan: &PlanNode) -> String {
    match &plan.kind {
        PlanKind::CheckTable => "CheckTable".into(),
        PlanKind::IndexScan {
            table,
            index,
            ranges,
        } => format!("Index({table}.{index})[{}]", ranges.join(" ")),
        PlanKind::TableScan { table } => format!("Table({table})"),
        PlanKind::HashJoin {
            inner_child,
            equal_conditions,
        } => format!(
            "{}HashJoin{{{}}}{}",
            if *inner_child == 0 { "Right" } else { "Left" },
            child_string(plan, "->"),
            keys_string(equal_conditions)
        ),
        PlanKind::MergeJoin { join_type, keys } => format!(
            "{}{{{}}}{}",
            merge_join_name(*join_type),
            child_string(plan, "->"),
            keys_string(keys)
        ),
        PlanKind::Apply => format!("Apply{{{}}}", child_string(plan, "->")),
        PlanKind::MaxOneRow => "MaxOneRow".into(),
        PlanKind::Limit { .. } => "Limit".into(),
        PlanKind::Lock => "Lock".into(),
        PlanKind::ShowDDL => "ShowDDL".into(),
        PlanKind::Show { extractor } => extractor
            .as_ref()
            .map_or_else(|| "Show".into(), |value| format!("Show({value})")),
        PlanKind::ShowDDLJobs => "ShowDDLJobs".into(),
        PlanKind::Sort => "Sort".into(),
        PlanKind::Join { equal_conditions } => {
            format!(
                "Join{{{}}}{}",
                child_string(plan, "->"),
                keys_string(equal_conditions)
            )
        }
        PlanKind::UnionAll { partition } => format!(
            "{}{{{}}}",
            if *partition {
                "PartitionUnionAll"
            } else {
                "UnionAll"
            },
            child_string(plan, "->")
        ),
        PlanKind::Sequence => format!("Sequence{{{}}}", child_string(plan, ",")),
        PlanKind::DataSource {
            table,
            alias,
            partition_id,
        } => partition_id.map_or_else(
            || {
                format!(
                    "DataScan({})",
                    alias
                        .as_deref()
                        .filter(|alias| !alias.is_empty())
                        .unwrap_or(table)
                )
            },
            |id| format!("Partition({id})"),
        ),
        PlanKind::Selection { conditions } => format!("Sel({})", conditions.join(", ")),
        PlanKind::Projection => "Projection".into(),
        PlanKind::TopN {
            by_items,
            offset,
            count,
        } => {
            format!("TopN([{}],{offset},{count})", by_items.join(" "))
        }
        PlanKind::Dual => "Dual".into(),
        PlanKind::HashAgg => "HashAgg".into(),
        PlanKind::StreamAgg => "StreamAgg".into(),
        PlanKind::Aggregation { functions } => format!("Aggr({})", functions.join(",")),
        PlanKind::TableReader => format!("TableReader({})", child_string(plan, "->")),
        PlanKind::IndexReader => format!("IndexReader({})", child_string(plan, "->")),
        PlanKind::IndexLookUpReader => format!("IndexLookUp({})", child_string(plan, ", ")),
        PlanKind::IndexMergeReader {
            partial_plans,
            table_plan,
        } => format!(
            "IndexMergeReader(PartialPlans->[{}], TablePlan->{})",
            partial_plans
                .iter()
                .map(ToString)
                .collect::<Vec<_>>()
                .join(", "),
            ToString(table_plan)
        ),
        PlanKind::UnionScan { conditions } => format!("UnionScan({})", conditions.join(", ")),
        PlanKind::IndexJoin { keys } => {
            format!(
                "IndexJoin{{{}}}{}",
                child_string(plan, "->"),
                keys_string(keys)
            )
        }
        PlanKind::IndexMergeJoin { keys } => {
            format!(
                "IndexMergeJoin{{{}}}{}",
                child_string(plan, "->"),
                keys_string(keys)
            )
        }
        PlanKind::IndexHashJoin { keys } => {
            format!(
                "IndexHashJoin{{{}}}{}",
                child_string(plan, "->"),
                keys_string(keys)
            )
        }
        PlanKind::Analyze { indexes, columns } => {
            let mut children = indexes
                .iter()
                .map(|index| format!("Index({index})"))
                .collect::<Vec<_>>();
            children.extend(
                columns
                    .iter()
                    .map(|cols| format!("Table({})", cols.join(", "))),
            );
            format!("Analyze{{{}}}", children.join(","))
        }
        PlanKind::Update => format!(
            "{}Update",
            if plan.children.is_empty() {
                String::new()
            } else {
                format!("{}->", child_string(plan, "->"))
            }
        ),
        PlanKind::Delete => format!(
            "{}Delete",
            if plan.children.is_empty() {
                String::new()
            } else {
                format!("{}->", child_string(plan, "->"))
            }
        ),
        PlanKind::Insert => format!(
            "{}Insert",
            if plan.children.is_empty() {
                String::new()
            } else {
                format!("{}->", child_string(plan, "->"))
            }
        ),
        PlanKind::Window { functions } => format!("Window({})", functions.join(",")),
        PlanKind::Shuffle { info } => format!("Partition({info})"),
        PlanKind::ShuffleReceiver { info } => format!("PartitionReceiverStub({info})"),
        PlanKind::ExchangeReceiver { task_ids } => format!(
            "Recv({}, )",
            task_ids
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        PlanKind::ExchangeSender { task_ids } => format!(
            "Send({}, )",
            task_ids
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        PlanKind::CTE { storage_id } => format!("CTEReader({storage_id})"),
        PlanKind::Generic(name) => name.clone(),
        other => other.name().to_owned(),
    }
}

/// 深度优先遍历：先收集子树片段，再追加本节点；分支算子则整段嵌入。
fn toString(plan: &PlanNode, output: &mut Vec<String>) {
    // Go 的 PhysicalExchangeReceiver 分支在遍历阶段明确不访问 Children。
    if matches!(plan.kind, PlanKind::ExchangeReceiver { .. }) {
        output.push(describe(plan));
        return;
    }
    // UnionAll / 多子节点：子树已在 describe 内嵌，直接输出本节点描述。
    if needIncludeChildrenString(plan) {
        output.push(describe(plan));
        return;
    }
    for child in &plan.children {
        toString(child, output);
    }
    output.push(describe(plan));
}
