// Copyright 2017 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// Copyright 2026 AsterSQL.

// Join 重排序（Join Reorder）核心数据结构与求解器。
//
// 多表 Inner Join 的连接顺序显著影响代价。本模块抽取连续 Inner Join 组，
// 按表数量阈值选择 DP（动态规划）或贪心求解器重排，并在 schema 变化时
// 用 Projection 恢复原始输出列顺序。

use crate::task::{Expression, JoinType};
use std::collections::{HashMap, HashSet};

/// 本模块通用结果类型别名。
pub type Result<T> = std::result::Result<T, String>;
/// 等值连接边：左右列下标及是否允许 NULL 相等。
#[derive(Clone, Debug)]
pub struct JoinEdge {
    pub left_column: usize,
    pub right_column: usize,
    pub null_equal: bool,
}
/// 连接计划树节点种类（叶子、Join、投影、过滤、聚合、Apply、窗口、UnionAll）。
#[derive(Clone, Debug)]
pub enum JoinNode {
    Leaf {
        name: String,
        predicates: Vec<Expression>,
        unique_keys: Vec<Vec<usize>>,
        correlated_columns: Vec<usize>,
    },
    Join {
        join_type: JoinType,
        left: Box<JoinPlan>,
        right: Box<JoinPlan>,
        equal_conditions: Vec<JoinEdge>,
        other_conditions: Vec<Expression>,
        preferred_method: Option<String>,
    },
    Projection {
        expressions: Vec<Expression>,
        child: Box<JoinPlan>,
    },
    Selection {
        conditions: Vec<Expression>,
        child: Box<JoinPlan>,
    },
    Aggregation {
        group_by: Vec<usize>,
        child: Box<JoinPlan>,
        default_values: HashMap<usize, String>,
    },
    Apply {
        join_type: JoinType,
        left: Box<JoinPlan>,
        right: Box<JoinPlan>,
        correlated_columns: Vec<usize>,
        no_decorrelate: bool,
    },
    Window {
        partition_by: Vec<usize>,
        row_number_column: Option<usize>,
        upper_bound: Option<u64>,
        child: Box<JoinPlan>,
    },
    UnionAll(Vec<JoinPlan>),
}
/// 带 id、算子节点、输出 schema 列集与行数估计的连接子计划。
#[derive(Clone, Debug)]
pub struct JoinPlan {
    pub id: usize,
    pub node: JoinNode,
    pub schema: Vec<usize>,
    pub row_count: f64,
}
impl JoinPlan {
    /// 返回 schema 列集合。
    pub fn columns(&self) -> HashSet<usize> {
        self.schema.iter().copied().collect()
    }
    /// 判断 schema 是否包含指定列。
    pub fn contains_column(&self, column: usize) -> bool {
        self.schema.contains(&column)
    }
}

/// 单个 Inner Join 组的基础信息：叶子计划、等值边与其它条件。
#[derive(Clone, Debug, Default)]
pub struct basicJoinGroupInfo {
    pub joinNodePlans: Vec<JoinPlan>,
    pub eqEdges: Vec<JoinEdge>,
    pub otherConds: Vec<Expression>,
}
/// 抽取 Join 组的完整结果：组信息、各层 Join 类型及原始 schema。
#[derive(Clone, Debug, Default)]
pub struct joinGroupResult {
    pub group: basicJoinGroupInfo,
    pub joinTypes: Vec<joinTypeWithExtMsg>,
    pub originalSchema: Vec<usize>,
}
/// Join 类型及其扩展信息（偏好物理方法、原始计划 id）。
#[derive(Clone, Debug)]
pub struct joinTypeWithExtMsg {
    pub joinType: JoinType,
    pub preferredMethod: Option<String>,
    pub originalPlanID: usize,
}
/// 带累计代价的连接顺序节点（用于贪心/链式拼接）。
#[derive(Clone, Debug)]
pub struct jrNode {
    pub p: JoinPlan,
    pub cumCost: f64,
}
/// 单组 Join 排序求解的共享状态：边、条件、类型列表与列分配器。
#[derive(Clone, Debug, Default)]
pub struct baseSingleGroupJoinOrderSolver {
    pub eqEdges: Vec<JoinEdge>,
    pub otherConds: Vec<Expression>,
    pub joinTypes: Vec<joinTypeWithExtMsg>,
    pub columnAllocator: usize,
}
/// Join 重排序总控：表数不超过 `dpThreshold` 用 DP，否则用贪心。
#[derive(Default)]
pub struct JoinReOrderSolver {
    pub dpThreshold: usize,
}

/// 从计划中抽取连续 Inner Join 组（公开入口）。
pub fn extractJoinGroup(plan: &JoinPlan) -> joinGroupResult {
    extractJoinGroupImpl(plan)
}
/// 实现：向下展开 Inner Join，叶子计划与边/条件分别收集。
pub fn extractJoinGroupImpl(plan: &JoinPlan) -> joinGroupResult {
    let mut result = joinGroupResult {
        originalSchema: plan.schema.clone(),
        ..joinGroupResult::default()
    };
    fn walk(plan: &JoinPlan, out: &mut joinGroupResult) {
        match &plan.node {
            JoinNode::Join {
                join_type: JoinType::Inner,
                left,
                right,
                equal_conditions,
                other_conditions,
                preferred_method,
            } => {
                // Inner Join 继续展开；边与类型记入结果。
                walk(left, out);
                walk(right, out);
                out.group.eqEdges.extend(equal_conditions.clone());
                out.group.otherConds.extend(other_conditions.clone());
                out.joinTypes.push(joinTypeWithExtMsg {
                    joinType: JoinType::Inner,
                    preferredMethod: preferred_method.clone(),
                    originalPlanID: plan.id,
                });
            }
            // 非连续 Inner Join 的子树整体作为一组内的「叶子」计划。
            _ => out.group.joinNodePlans.push(plan.clone()),
        }
    }
    walk(plan, &mut result);
    result
}

impl JoinReOrderSolver {
    /// 优化入口。
    pub fn Optimize(&mut self, plan: JoinPlan) -> Result<(JoinPlan, bool)> {
        self.optimizeRecursive(plan)
    }
    /// 先递归优化子树，再对本层可重排的 Inner Join 组调用 DP/贪心。
    pub fn optimizeRecursive(&mut self, plan: JoinPlan) -> Result<(JoinPlan, bool)> {
        let plan = map_children(plan, |child| self.optimizeRecursive(child).map(|v| v.0))?;
        let group = extractJoinGroup(&plan);
        if group.group.joinNodePlans.len() < 2 {
            return Ok((plan, false));
        }
        let mut base = baseSingleGroupJoinOrderSolver {
            eqEdges: group.group.eqEdges.clone(),
            otherConds: group.group.otherConds.clone(),
            joinTypes: group.joinTypes.clone(),
            ..baseSingleGroupJoinOrderSolver::default()
        };
        // 小规模用 DP 求最优；大规模用贪心近似。
        let reordered = if group.group.joinNodePlans.len() <= self.dpThreshold.max(2) {
            crate::rule_join_reorder_dp::joinReorderDPSolver { base: base.clone() }
                .solve(&group.group.joinNodePlans)?
        } else {
            crate::rule_join_reorder_greedy::joinReorderGreedySolver {
                base: base.clone(),
                joinNodePlans: group.group.joinNodePlans.clone(),
            }
            .solve()?
        };
        Ok((
            restoreSchemaIfChanged(reordered, &group.originalSchema, &mut base),
            true,
        ))
    }
    /// 返回规则注册名。
    pub fn Name(&self) -> &'static str {
        "join_reorder"
    }
}
/// 若重排后 schema 列序变化，则包一层 Projection 恢复原始输出顺序。
pub fn restoreSchemaIfChanged(
    plan: JoinPlan,
    originalSchema: &[usize],
    base: &mut baseSingleGroupJoinOrderSolver,
) -> JoinPlan {
    if plan.schema == originalSchema {
        return plan;
    }
    let expressions = originalSchema
        .iter()
        .map(|column| Expression {
            name: format!("col_{column}"),
            column: Some(*column),
            ..Expression::default()
        })
        .collect();
    let id = base.columnAllocator;
    base.columnAllocator += 1;
    JoinPlan {
        id,
        node: JoinNode::Projection {
            expressions,
            child: Box::new(plan),
        },
        schema: originalSchema.to_vec(),
        row_count: 0.0,
    }
}
/// 将 `src` 映射合并进 `dst`。
pub fn mergeMap<K: Eq + std::hash::Hash, V>(dst: &mut HashMap<K, V>, src: HashMap<K, V>) {
    dst.extend(src);
}

impl baseSingleGroupJoinOrderSolver {
    /// 按 leading hint 给定的计划 id 顺序重排组内节点，其余保持原序追加。
    pub fn generateLeadingJoinGroup(
        &mut self,
        plans: &[JoinPlan],
        leading: &[usize],
    ) -> Result<Vec<JoinPlan>> {
        let mut output = Vec::new();
        let mut used = HashSet::new();
        for id in leading {
            let plan = plans
                .iter()
                .find(|p| p.id == *id)
                .ok_or_else(|| format!("leading hint references plan {id}"))?;
            if used.insert(*id) {
                output.push(plan.clone());
            }
        }
        output.extend(plans.iter().filter(|p| !used.contains(&p.id)).cloned());
        Ok(output)
    }
    /// 嵌套 leading 组：按多层 hint 依次调用 `generateLeadingJoinGroup`。
    pub fn generateNestedLeadingJoinGroup(
        &mut self,
        plans: &[JoinPlan],
        groups: &[Vec<usize>],
    ) -> Result<Vec<JoinPlan>> {
        let mut current = plans.to_vec();
        for group in groups {
            current = self.generateLeadingJoinGroup(&current, group)?;
        }
        Ok(current)
    }
    /// 将 jrNode 列表从尾部两两弹出并 `makeJoin`，得到左深连接树。
    pub fn connectJoinNodes(&mut self, mut nodes: Vec<jrNode>) -> Result<JoinPlan> {
        if nodes.is_empty() {
            return Err("empty join group".into());
        }
        while nodes.len() > 1 {
            let right = nodes.pop().unwrap();
            let left = nodes.pop().unwrap();
            let base_cost = left.cumCost + right.cumCost;
            let (plan, _) = self.makeJoin(left.p, right.p, Vec::new(), None, Vec::new());
            nodes.push(jrNode {
                cumCost: base_cost + plan.row_count.max(1.0),
                p: plan,
            });
        }
        Ok(nodes.pop().unwrap().p)
    }
    /// 为每个叶子计划构造带基础累计代价的 jrNode。
    pub fn generateJoinOrderNode(&self, plans: &[JoinPlan]) -> Vec<jrNode> {
        plans
            .iter()
            .cloned()
            .map(|p| jrNode {
                cumCost: self.baseNodeCumCost(&p),
                p,
            })
            .collect()
    }
    /// 节点累计代价为自身与全部后代的行数估计之和。
    pub fn baseNodeCumCost(&self, plan: &JoinPlan) -> f64 {
        let child_cost = match &plan.node {
            JoinNode::Join { left, right, .. } | JoinNode::Apply { left, right, .. } => {
                self.baseNodeCumCost(left) + self.baseNodeCumCost(right)
            }
            JoinNode::Projection { child, .. }
            | JoinNode::Selection { child, .. }
            | JoinNode::Aggregation { child, .. }
            | JoinNode::Window { child, .. } => self.baseNodeCumCost(child),
            JoinNode::UnionAll(children) => children
                .iter()
                .map(|child| self.baseNodeCumCost(child))
                .sum(),
            JoinNode::Leaf { .. } => 0.0,
        };
        plan.row_count + child_cost
    }
    /// 检查左右计划之间是否存在等值边，并返回边列表与默认 Join 类型信息。
    pub fn checkConnection(
        &self,
        left: &JoinPlan,
        right: &JoinPlan,
    ) -> (Vec<JoinEdge>, Option<joinTypeWithExtMsg>) {
        let edges = self
            .eqEdges
            .iter()
            .filter(|e| {
                left.contains_column(e.left_column) && right.contains_column(e.right_column)
                    || left.contains_column(e.right_column) && right.contains_column(e.left_column)
            })
            .cloned()
            .collect();
        (edges, self.joinTypes.first().cloned())
    }
    /// 判断其它条件是否引用左右任一侧的列。
    pub fn hasOtherJoinCondition(&self, left: &JoinPlan, right: &JoinPlan) -> bool {
        self.otherConds.iter().any(|e| {
            e.column
                .is_some_and(|c| left.contains_column(c) || right.contains_column(c))
        })
    }
    /// 构造一条等值连接边。
    pub fn buildJoinEdge(&self, left: usize, right: usize, nullEqual: bool) -> JoinEdge {
        JoinEdge {
            left_column: left,
            right_column: right,
            null_equal: nullEqual,
        }
    }
    /// 若表达式尚无列下标，则注入投影分配新列并返回（新计划, 列号）。
    pub fn injectExpr(&mut self, plan: JoinPlan, expression: Expression) -> (JoinPlan, usize) {
        if let Some(column) = expression.column {
            return (plan, column);
        }
        let column = self.columnAllocator;
        self.columnAllocator += 1;
        let mut schema = plan.schema.clone();
        schema.push(column);
        let id = self.columnAllocator;
        (
            JoinPlan {
                id,
                node: JoinNode::Projection {
                    expressions: vec![expression],
                    child: Box::new(plan),
                },
                schema,
                row_count: 0.0,
            },
            column,
        )
    }
    /// 按旧/新计划 id 查找并传播偏好物理 Join 方法 hint。
    pub fn propagateJoinMethodHint(&self, oldPlanID: usize, newPlanID: usize) -> Option<String> {
        self.joinTypes
            .iter()
            .find(|j| j.originalPlanID == oldPlanID || j.originalPlanID == newPlanID)
            .and_then(|j| j.preferredMethod.clone())
    }
    /// 组装 Join 节点：补齐等值边、吸收相关 other 条件、合并 schema 并估计行数。
    pub fn makeJoin(
        &mut self,
        left: JoinPlan,
        right: JoinPlan,
        mut edges: Vec<JoinEdge>,
        joinType: Option<joinTypeWithExtMsg>,
        inputOtherConds: Vec<Expression>,
    ) -> (JoinPlan, Vec<Expression>) {
        if edges.is_empty() {
            edges = self.checkConnection(&left, &right).0;
        }
        let mut other = inputOtherConds;
        let left_cols = left.columns();
        let right_cols = right.columns();
        let mut remaining = Vec::new();
        // 能下推到本 Join 的 other 条件吸入，其余留在求解器状态中。
        for condition in self.otherConds.drain(..) {
            if condition
                .column
                .is_some_and(|c| left_cols.contains(&c) || right_cols.contains(&c))
            {
                other.push(condition);
            } else {
                remaining.push(condition);
            }
        }
        self.otherConds = remaining;
        let mut schema = left.schema.clone();
        for column in &right.schema {
            if !schema.contains(column) {
                schema.push(*column);
            }
        }
        let rows = estimate_join_rows(&left, &right, !edges.is_empty());
        let id = self.columnAllocator;
        self.columnAllocator += 1;
        (
            JoinPlan {
                id,
                node: JoinNode::Join {
                    join_type: joinType
                        .as_ref()
                        .map(|j| j.joinType)
                        .unwrap_or(JoinType::Inner),
                    left: Box::new(left),
                    right: Box::new(right),
                    equal_conditions: edges,
                    other_conditions: other,
                    preferred_method: joinType.and_then(|j| j.preferredMethod),
                },
                schema,
                row_count: rows,
            },
            self.otherConds.clone(),
        )
    }
    /// 无等值边时按轮次两两合并，构造灌木（bushy）连接树。
    pub fn makeBushyJoin(&mut self, mut plans: Vec<JoinPlan>) -> Result<JoinPlan> {
        if plans.is_empty() {
            return Err("empty cartesian join group".into());
        }
        while plans.len() > 1 {
            let mut next = Vec::with_capacity(plans.len().div_ceil(2));
            let mut iter = plans.into_iter();
            while let Some(left) = iter.next() {
                if let Some(right) = iter.next() {
                    next.push(self.newCartesianJoin(left, right));
                } else {
                    next.push(left);
                }
            }
            plans = next;
        }
        Ok(plans.pop().unwrap())
    }
    /// 创建无等值条件的笛卡尔 Join。
    pub fn newCartesianJoin(&mut self, left: JoinPlan, right: JoinPlan) -> JoinPlan {
        self.makeJoin(left, right, Vec::new(), None, Vec::new()).0
    }
    /// 用给定等值边创建 Join。
    pub fn newJoinWithEdges(
        &mut self,
        left: JoinPlan,
        right: JoinPlan,
        edges: Vec<JoinEdge>,
    ) -> JoinPlan {
        self.makeJoin(left, right, edges, None, Vec::new()).0
    }
    /// 累计代价 = 左右累计代价 + 本 Join 行数估计。
    pub fn calcJoinCumCost(&self, join: &JoinPlan, left: &jrNode, right: &jrNode) -> f64 {
        left.cumCost + right.cumCost + join.row_count.max(1.0)
    }
}
/// 已注入表达式是否可复用：已有列引用，或非不确定函数。
pub fn canReuseInjectedJoinExpr(expression: &Expression) -> bool {
    expression.column.is_some()
        || expression.function_count == 0 && !expression.name.starts_with("nondeterministic:")
}
/// 在 Join 组中查找包含指定列的节点下标。
pub fn findNodeIndexInGroup(group: &[JoinPlan], column: usize) -> Result<usize> {
    group
        .iter()
        .position(|p| p.contains_column(column))
        .ok_or_else(|| format!("column {column} is not in join group"))
}
/// 查找同时覆盖给定列集合的单一节点下标；列跨多个节点则报错。
pub fn findNodeIndexForColumns(group: &[JoinPlan], columns: &[usize]) -> Result<usize> {
    let mut found = None;
    for column in columns {
        let index = findNodeIndexInGroup(group, *column)?;
        if found.is_some_and(|old| old != index) {
            return Err("columns span multiple join nodes".into());
        }
        found = Some(index);
    }
    found.ok_or_else(|| "empty column set".into())
}
/// 估计 Join 输出行数：有连接边时取笛卡尔积开方与较小输入行数的较大者。
fn estimate_join_rows(left: &JoinPlan, right: &JoinPlan, connected: bool) -> f64 {
    let cartesian = left.row_count.max(1.0) * right.row_count.max(1.0);
    if connected {
        cartesian.sqrt().max(left.row_count.min(right.row_count))
    } else {
        cartesian
    }
}
/// 对 JoinPlan 所有子节点应用变换 `f`，保持本节点其它字段不变。
fn map_children<F>(mut plan: JoinPlan, mut f: F) -> Result<JoinPlan>
where
    F: FnMut(JoinPlan) -> Result<JoinPlan>,
{
    plan.node = match plan.node {
        JoinNode::Join {
            join_type,
            left,
            right,
            equal_conditions,
            other_conditions,
            preferred_method,
        } => JoinNode::Join {
            join_type,
            left: Box::new(f(*left)?),
            right: Box::new(f(*right)?),
            equal_conditions,
            other_conditions,
            preferred_method,
        },
        JoinNode::Projection { expressions, child } => JoinNode::Projection {
            expressions,
            child: Box::new(f(*child)?),
        },
        JoinNode::Selection { conditions, child } => JoinNode::Selection {
            conditions,
            child: Box::new(f(*child)?),
        },
        JoinNode::Aggregation {
            group_by,
            child,
            default_values,
        } => JoinNode::Aggregation {
            group_by,
            child: Box::new(f(*child)?),
            default_values,
        },
        JoinNode::Apply {
            join_type,
            left,
            right,
            correlated_columns,
            no_decorrelate,
        } => JoinNode::Apply {
            join_type,
            left: Box::new(f(*left)?),
            right: Box::new(f(*right)?),
            correlated_columns,
            no_decorrelate,
        },
        JoinNode::Window {
            partition_by,
            row_number_column,
            upper_bound,
            child,
        } => JoinNode::Window {
            partition_by,
            row_number_column,
            upper_bound,
            child: Box::new(f(*child)?),
        },
        JoinNode::UnionAll(children) => JoinNode::UnionAll(
            children
                .into_iter()
                .map(&mut f)
                .collect::<Result<Vec<_>>>()?,
        ),
        leaf => leaf,
    };
    Ok(plan)
}
