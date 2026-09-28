// Copyright 2017 PingCAP, Inc. Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 消除多余 Projection 的逻辑/物理优化规则。
//
// Projection（投影算子）用于重排或计算输出列。若投影表达式仅为列恒等映射
//（col_i → col_i），则可删除该层；严格模式还要求 schema 与孩子一致。消除时
// 需用替换表重写上层表达式中的列引用，保持语义正确。

use crate::task::{Expression, PlanKind, PlanNode};
use std::collections::HashMap;
/// 宽松判定：投影是否仅为列恒等映射（可被消除的候选）。
pub fn canProjectionBeEliminatedLoose(p: &PlanNode) -> bool {
    p.kind == PlanKind::Projection
        && p.children.len() == 1
        && p.expressions
            .iter()
            .all(|expression| expression.column.is_some())
}
/// 严格判定：在宽松条件基础上还要求本节点 schema 与孩子相同。
pub fn canProjectionBeEliminatedStrict(p: &PlanNode) -> bool {
    if !canProjectionBeEliminatedLoose(p) {
        return false;
    }
    if p.schema.is_empty() {
        return true;
    }
    p.schema.len() == p.children[0].schema.len()
        && p.expressions
            .iter()
            .enumerate()
            .all(|(offset, expression)| expression.column == Some(offset))
}
/// 自底向上做物理投影消除：严格可消除则返回孩子。
pub fn doPhysicalProjectionElimination(mut p: PlanNode) -> PlanNode {
    p.children = p
        .children
        .into_iter()
        .map(eliminatePhysicalProjection)
        .collect();
    if canProjectionBeEliminatedStrict(&p) {
        p.children.remove(0)
    } else {
        p
    }
}
/// 物理投影消除入口。
pub fn eliminatePhysicalProjection(p: PlanNode) -> PlanNode {
    doPhysicalProjectionElimination(p)
}
/// 逻辑投影消除器：递归消除恒等投影并维护列替换映射。
#[derive(Default)]
pub struct ProjectionEliminator;
impl ProjectionEliminator {
    /// 优化入口：新建空替换表后进入递归消除。
    pub fn Optimize(&self, p: PlanNode) -> (PlanNode, bool) {
        let (p, _) = self.eliminate(p, &mut HashMap::new(), false);
        // 与 Go 规则一致：本规则通过树重写工作，但不设置 planChanged。
        (p, false)
    }
    /// 递归消除：可消除时记录列→表达式替换并返回孩子；否则用替换表改写本节点表达式。
    pub fn eliminate(
        &self,
        mut p: PlanNode,
        replace: &mut HashMap<usize, Expression>,
        canEliminate: bool,
    ) -> (PlanNode, bool) {
        // LogicalCTE 有独立的逻辑优化过程，不遍历也不重写其子树。
        if p.kind == PlanKind::Cte {
            return (p, false);
        }

        let mut changed = false;
        let child_can_eliminate = match p.kind {
            PlanKind::UnionAll => false,
            PlanKind::HashAgg | PlanKind::StreamAgg | PlanKind::Projection | PlanKind::Window => {
                true
            }
            _ => canEliminate,
        };
        p.children = p
            .children
            .into_iter()
            .map(|c| {
                let (c, x) = self.eliminate(c, replace, child_can_eliminate);
                changed |= x;
                c
            })
            .collect();
        // 恒等投影：把各输出列记入替换表后剥离本层。
        if canEliminate && canProjectionBeEliminatedLoose(&p) {
            for (i, e) in p.expressions.iter().enumerate() {
                replace.insert(i, e.clone());
            }
            return (p.children.remove(0), true);
        }
        // 对本节点表达式/条件应用已累积的列替换。
        for e in p.expressions.iter_mut().chain(p.conditions.iter_mut()) {
            if let Some(c) = e.column {
                if let Some(r) = replace.get(&c) {
                    *e = r.clone();
                }
            }
        }
        (p, changed)
    }
    /// 返回规则注册名。
    pub fn Name(&self) -> &'static str {
        "projection_eliminate"
    }
}
