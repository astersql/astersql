// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 贪心 Join 重排序求解器。
//
// Join Reorder（连接重排序）在保持语义等价前提下调整多表连接顺序。
// 贪心策略反复挑选当前估计行数最小的一对节点合并，直至只剩一棵连接树；
// 相对 DP 更适合表数较多、可接受近似最优的场景。

use crate::rule_join_reorder::{JoinPlan, Result, baseSingleGroupJoinOrderSolver, jrNode};
use crate::task::Expression;

/// 基于单组求解器基类的贪心连接顺序求解器。
#[derive(Clone, Debug, Default)]
pub struct joinReorderGreedySolver {
    /// 等值边、其它条件与代价计算等共享状态。
    pub base: baseSingleGroupJoinOrderSolver,
    /// 待重排的叶子/子树计划列表。
    pub joinNodePlans: Vec<JoinPlan>,
}
impl joinReorderGreedySolver {
    /// 与 Go 默认配置一致：按累计代价排序，从最小节点开始扩展连通分量，
    /// 最后再把互不连通的分量组成 bushy 笛卡尔连接树。
    pub fn solve(&mut self) -> Result<JoinPlan> {
        let mut nodes = self.base.generateJoinOrderNode(&self.joinNodePlans);
        if nodes.is_empty() {
            return Err("empty join group".into());
        }
        nodes.sort_by(|left, right| left.cumCost.total_cmp(&right.cumCost));

        let mut cartesian_group = Vec::new();
        while !nodes.is_empty() {
            cartesian_group.push(self.construct_connected_join_tree(&mut nodes)?.p);
        }
        self.base.makeBushyJoin(cartesian_group)
    }

    fn construct_connected_join_tree(&mut self, nodes: &mut Vec<jrNode>) -> Result<jrNode> {
        let mut current = nodes.remove(0);
        loop {
            let mut best: Option<(usize, jrNode, baseSingleGroupJoinOrderSolver)> = None;
            for (index, node) in nodes.iter().enumerate() {
                // makeJoin 会移动 otherConds；枚举候选必须隔离状态，只提交最终候选。
                let mut candidate_base = self.base.clone();
                let (edges, join_type) = candidate_base.checkConnection(&current.p, &node.p);
                let is_cartesian =
                    edges.is_empty() && !candidate_base.hasOtherJoinCondition(&current.p, &node.p);
                // Go 默认 CartesianJoinOrderThreshold 为 0，连通树阶段禁用笛卡尔边。
                if is_cartesian {
                    continue;
                }
                let (plan, _) = candidate_base.makeJoin(
                    current.p.clone(),
                    node.p.clone(),
                    edges,
                    join_type,
                    Vec::new(),
                );
                let cost = candidate_base.calcJoinCumCost(&plan, &current, node);
                if best.as_ref().is_none_or(|(_, old, _)| cost < old.cumCost) {
                    best = Some((
                        index,
                        jrNode {
                            p: plan,
                            cumCost: cost,
                        },
                        candidate_base,
                    ));
                }
            }
            let Some((index, joined, selected_base)) = best else {
                break;
            };
            nodes.remove(index);
            self.base = selected_base;
            current = joined;
        }
        Ok(current)
    }
    /// 求解后包装为带累计代价的 jrNode，供上层连接组组装使用。
    pub fn constructConnectedJoinTree(&mut self) -> Result<jrNode> {
        let mut nodes = self.base.generateJoinOrderNode(&self.joinNodePlans);
        if nodes.is_empty() {
            return Err("empty join group".into());
        }
        nodes.sort_by(|left, right| left.cumCost.total_cmp(&right.cumCost));
        self.construct_connected_join_tree(&mut nodes)
    }
    /// 检查左右子计划是否有等值边或其它连接条件，并尝试构造 Join。
    pub fn checkConnectionAndMakeJoin(
        &mut self,
        left: JoinPlan,
        right: JoinPlan,
    ) -> (Option<JoinPlan>, Vec<Expression>, bool) {
        let (edges, join_type) = self.base.checkConnection(&left, &right);
        let is_cartesian = edges.is_empty() && !self.base.hasOtherJoinCondition(&left, &right);
        if is_cartesian {
            return (None, Vec::new(), true);
        }
        let (plan, remaining) = self
            .base
            .makeJoin(left, right, edges, join_type, Vec::new());
        (Some(plan), remaining, false)
    }
}
