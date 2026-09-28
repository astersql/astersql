// Copyright 2017 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// Copyright 2026 AsterSQL.

// Join 重排序的动态规划（DP）求解器。
//
// 对规模较小的 Inner Join 组，用位图枚举连通子集的最优连接树：先按等值边
// 建邻接图并用 BFS 确定节点处理顺序，再对每个子集掩码枚举二分拆分，保留
// 行数估计更优的方案。等值连通分量分别求解，分量之间再以灌木树完成笛卡尔连接。

use crate::rule_join_reorder::{JoinEdge, JoinPlan, Result, baseSingleGroupJoinOrderSolver};
use crate::task::Expression;
use std::collections::{HashMap, VecDeque};

/// 组内节点间的一条等值边（节点下标 + 原始 JoinEdge）。
#[derive(Clone, Debug)]
pub struct joinGroupEqEdge {
    pub node1: usize,
    pub node2: usize,
    pub edge: JoinEdge,
}
/// 组内涉及多节点的非等值条件。
#[derive(Clone, Debug)]
pub struct joinGroupNonEqEdge {
    pub nodes: Vec<usize>,
    pub condition: Expression,
}
/// DP Join 重排序求解器，复用单组求解的共享状态。
#[derive(Clone, Debug, Default)]
pub struct joinReorderDPSolver {
    pub base: baseSingleGroupJoinOrderSolver,
}
/// 在 Join 组中查找包含指定列的节点下标（转发到 join_reorder 模块）。
pub fn findNodeIndexInGroup(group: &[JoinPlan], column: usize) -> Result<usize> {
    crate::rule_join_reorder::findNodeIndexInGroup(group, column)
}
/// 查找同时覆盖给定列集合的单一节点下标。
pub fn findNodeIndexForColumns(group: &[JoinPlan], columns: &[usize]) -> Result<usize> {
    crate::rule_join_reorder::findNodeIndexForColumns(group, columns)
}
impl joinReorderDPSolver {
    /// 求解入口：建等值边邻接图 → BFS 排序 → DP 枚举最优连接树。
    pub fn solve(&mut self, joinGroup: &[JoinPlan]) -> Result<JoinPlan> {
        if joinGroup.is_empty() {
            return Err("empty join group".into());
        }
        if joinGroup.len() > usize::BITS as usize - 1 {
            return Err("join group is too large for DP bitmap".into());
        }
        // 将列级等值边映射为组内节点下标边。
        let mut eq = Vec::new();
        for edge in &self.base.eqEdges {
            let left = findNodeIndexForColumns(joinGroup, &[edge.left_column])?;
            let right = findNodeIndexForColumns(joinGroup, &[edge.right_column])?;
            if left != right {
                eq.push(joinGroupEqEdge {
                    node1: left,
                    node2: right,
                    edge: edge.clone(),
                });
            }
        }
        let mut adjacency = vec![Vec::new(); joinGroup.len()];
        for edge in &eq {
            adjacency[edge.node1].push(edge.node2);
            adjacency[edge.node2].push(edge.node1);
        }
        // 与 Go 一致，分别对每个等值边连通分量做 DP；分量之间最后再构造
        // 灌木式笛卡尔连接树。把所有节点放进一次 DP 会导致三个以上互不
        // 连通的节点没有可用的中间子计划。
        let mut visited = vec![false; joinGroup.len()];
        let mut joins = Vec::new();
        for start in 0..joinGroup.len() {
            if visited[start] {
                continue;
            }
            let component = self.bfsGraph(start, &adjacency);
            for node in &component {
                visited[*node] = true;
            }
            joins.push(self.dpGraph(&component, joinGroup, &eq)?);
        }
        self.makeBushyJoin(joins, Vec::new())
    }
    /// BFS 遍历单个连通分量得到节点顺序。
    pub fn bfsGraph(&self, startNode: usize, adjacents: &[Vec<usize>]) -> Vec<usize> {
        let mut visited = vec![false; adjacents.len()];
        let mut queue = VecDeque::from([startNode]);
        let mut output = Vec::new();
        while let Some(node) = queue.pop_front() {
            if visited[node] {
                continue;
            }
            visited[node] = true;
            output.push(node);
            for adjacent in &adjacents[node] {
                if !visited[*adjacent] {
                    queue.push_back(*adjacent);
                }
            }
        }
        output
    }
    /// 按位掩码 DP：`best[mask]` 存该子集最优 JoinPlan，最终取全集掩码。
    pub fn dpGraph(
        &mut self,
        order: &[usize],
        group: &[JoinPlan],
        edges: &[joinGroupEqEdge],
    ) -> Result<JoinPlan> {
        let mut best: HashMap<usize, JoinPlan> = HashMap::new();
        // 单节点子集初始化。
        for (bit, node) in order.iter().enumerate() {
            best.insert(1usize << bit, group[*node].clone());
        }
        let full: usize = (1usize << order.len()) - 1;
        for size in 2..=order.len() {
            for mask in 1usize..=full {
                if mask.count_ones() as usize != size {
                    continue;
                }
                // 枚举 mask 的非空真子集作为左半，补集为右半（避免重复：subset < other）。
                let mut subset = (mask - 1) & mask;
                while subset > 0 {
                    let other = mask ^ subset;
                    if subset < other {
                        let (Some(left), Some(right)) =
                            (best.get(&subset).cloned(), best.get(&other).cloned())
                        else {
                            subset = (subset - 1) & mask;
                            continue;
                        };
                        // DP 仅在一个等值连通分量内组合有连接边的子计划。
                        if !self.nodesAreConnected(subset, other, order, edges) {
                            subset = (subset - 1) & mask;
                            continue;
                        }
                        let join = self.newJoinWithEdge(left, right, edges, &[])?;
                        if best
                            .get(&mask)
                            .is_none_or(|old| join.row_count < old.row_count)
                        {
                            best.insert(mask, join);
                        }
                    }
                    subset = (subset - 1) & mask;
                }
            }
        }
        best.remove(&full)
            .ok_or_else(|| "DP could not build join tree".into())
    }
    /// 判断两个子集掩码之间是否存在跨边等值连接。
    pub fn nodesAreConnected(
        &self,
        leftMask: usize,
        rightMask: usize,
        order: &[usize],
        edges: &[joinGroupEqEdge],
    ) -> bool {
        let bit_for = |node: usize| {
            order
                .iter()
                .position(|n| *n == node)
                .map(|b| 1usize << b)
                .unwrap_or(0)
        };
        edges.iter().any(|e| {
            leftMask & bit_for(e.node1) != 0 && rightMask & bit_for(e.node2) != 0
                || leftMask & bit_for(e.node2) != 0 && rightMask & bit_for(e.node1) != 0
        })
    }
    /// 选取左右计划之间适用的等值边并调用 `makeJoin`。
    pub fn newJoinWithEdge(
        &mut self,
        left: JoinPlan,
        right: JoinPlan,
        edges: &[joinGroupEqEdge],
        otherConds: &[Expression],
    ) -> Result<JoinPlan> {
        let selected = edges
            .iter()
            .filter(|e| {
                left.contains_column(e.edge.left_column)
                    && right.contains_column(e.edge.right_column)
                    || left.contains_column(e.edge.right_column)
                        && right.contains_column(e.edge.left_column)
            })
            .map(|e| e.edge.clone())
            .collect();
        Ok(self
            .base
            .makeJoin(left, right, selected, None, otherConds.to_vec())
            .0)
    }
    /// 将剩余节点做灌木式笛卡尔连接，并并入额外 other 条件。
    pub fn makeBushyJoin(
        &mut self,
        group: Vec<JoinPlan>,
        otherConds: Vec<Expression>,
    ) -> Result<JoinPlan> {
        self.base.otherConds.extend(otherConds);
        self.base.makeBushyJoin(group)
    }
}
