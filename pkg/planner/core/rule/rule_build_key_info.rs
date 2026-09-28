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

// BuildKeySolver 对应 Go 类型，用于为逻辑计划构建 key 信息。
// pub struct BuildKeySolver;
//
// *************************** LogicalOptRule 接口实现 ***************************
// impl BuildKeySolver {
// Name 对应 LogicalOptRule.Name，返回规则注册名。
//     pub fn Name(&self) -> &'static str { "build_keys" }
//
// Optimize 对应 LogicalOptRule.Optimize。context 在 Go 中未使用，故这里省略。
//     pub fn Optimize(&self, plan: base::LogicalPlan) -> (base::LogicalPlan, bool, Result<(), Error>) {
//         let plan_changed = false;
//         ruleutil::build_key_info_portal(&plan);
//         (plan, plan_changed, Ok(()))
//     }
// }
//
// Go imports：context；planner/core/base；planner/core/rule/util。
// 这些名称是迁移参考，未在这里中虚构跨文件模块连接。
// */
// 构建逻辑计划的唯一键 / 候选键（key）信息。
//
// Key 信息描述输出行可由哪些列唯一确定，供 Join 消除、聚合简化等
// 后续优化规则使用。自底向上从 DataSource 索引列推导，并经 Projection 等算子映射。

use crate::rule_init::{LogicalRule, Plan, PlanKind};

/// 构建 key 信息的逻辑优化规则求解器。
pub struct BuildKeySolver;
impl LogicalRule for BuildKeySolver {
    /// 规则注册名。
    fn name(&self) -> &'static str {
        "build_keys"
    }
    /// 遍历计划树填充 `plan.keys`；结构不变，返回 changed=false。
    fn optimize(&self, mut plan: Plan) -> Result<(Plan, bool), String> {
        build_keys(&mut plan);
        Ok((plan, false))
    }
}
/// 后序遍历：先处理子节点，再按算子类型推导本层 keys。
fn build_keys(plan: &mut Plan) {
    for child in &mut plan.children {
        build_keys(child);
    }
    plan.keys = match &plan.kind {
        // DataSource：非空索引列集均可作为候选唯一键。
        PlanKind::DataSource { indexes, .. } => indexes
            .values()
            .filter(|columns| !columns.is_empty())
            .cloned()
            .collect(),
        // Projection：仅当子键每一列都能映射到「单列恒等」投影输出时保留。
        PlanKind::Projection { expressions } => {
            if expressions.len() != plan.schema.len() {
                Vec::new()
            } else {
                plan.children
                    .first()
                    .map(|child| {
                        child
                            .keys
                            .iter()
                            .filter_map(|key| {
                                key.iter()
                                    .map(|column| {
                                        expressions.iter().enumerate().find_map(
                                            |(offset, expression)| {
                                                (expression.columns()
                                                    == std::collections::BTreeSet::from([*column]))
                                                .then_some(plan.schema[offset])
                                            },
                                        )
                                    })
                                    .collect::<Option<Vec<_>>>()
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            }
        }
        // Selection/Sort/Limit 不改变唯一性，直接继承子节点 keys。
        PlanKind::Selection | PlanKind::Sort { .. } | PlanKind::Limit { .. } => plan
            .children
            .first()
            .map(|child| child.keys.clone())
            .unwrap_or_default(),
        // Semi/AntiSemi Join：输出唯一性由左子树决定。
        PlanKind::Join {
            join_type: crate::rule_init::JoinType::Semi | crate::rule_init::JoinType::AntiSemi,
            ..
        } => plan
            .children
            .first()
            .map(|child| child.keys.clone())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
}
