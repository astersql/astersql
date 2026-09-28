// Copyright 2017 PingCAP, Inc. Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 谓词下推（Predicate Pushdown，PPD）及相关分片前缀条件构造。
//
// 谓词下推把 Selection（过滤）条件下推到更靠近数据源的算子，以尽早减少中间结果行数。
// 另含为分片索引（shard index）在 CNF/DNF 条件下追加前缀谓词的辅助逻辑。

use crate::find_best_task::AccessPath;
use crate::task::{Expression, PlanKind, PlanNode};
/// 谓词下推优化规则：将过滤条件尽量推到计划树下方。
#[derive(Default)]
pub struct PPDSolver;
impl PPDSolver {
    /// 从空的传入谓词集合开始下推。
    pub fn Optimize(&self, p: PlanNode) -> (PlanNode, bool) {
        let (p, _) = push(p, Vec::new());
        // Go's PPDSolver deliberately leaves planChanged false even when
        // PredicatePushDown rewrites the logical plan tree.
        (p, false)
    }
    /// 返回规则注册名。
    pub fn Name(&self) -> &'static str {
        "predicate_push_down"
    }
}
/// 按分片列与位数，为索引访问条件追加 shard 前缀表达式。
#[derive(Clone)]
pub struct exprPrefixAdder {
    /// 分片键所在列。
    pub shardColumn: usize,
    /// 分片位数（决定前缀粒度）。
    pub shardBits: u8,
}
impl exprPrefixAdder {
    /// 对分片索引的访问条件追加前缀（委托 CNF 路径）。
    pub fn addExprPrefix4ShardIndex(&self, conds: &[Expression]) -> Vec<Expression> {
        self.addExprPrefix4CNFCond(conds)
    }
    /// 对 CNF（合取范式）条件中命中分片列的 eq/in 谓词追加 shard 前缀。
    pub fn addExprPrefix4CNFCond(&self, conds: &[Expression]) -> Vec<Expression> {
        let mut out = conds.to_vec();
        for c in conds {
            if c.column == Some(self.shardColumn)
                && (c.name.starts_with("eq:") || c.name.starts_with("in:"))
            {
                out.push(Expression {
                    name: format!("shard({},{})", c.name, self.shardBits),
                    column: Some(self.shardColumn),
                    ..Default::default()
                });
            }
        }
        out
    }
    /// 将 DNF（析取范式）`or:` 条件拆成独立分支表达式列表。
    pub fn addExprPrefix4DNFCond(&self, c: &Expression) -> Vec<Expression> {
        c.name
            .strip_prefix("or:")
            .map(|s| {
                s.split('|')
                    .map(|n| Expression {
                        name: n.into(),
                        column: c.column,
                        ..Default::default()
                    })
                    .collect()
            })
            .unwrap_or_else(|| vec![c.clone()])
    }
}
/// 依次用多个 adder 为条件集合追加分片前缀。
pub fn addPrefix4ShardIndexes(conds: &[Expression], adders: &[exprPrefixAdder]) -> Vec<Expression> {
    adders
        .iter()
        .fold(conds.to_vec(), |v, a| a.addExprPrefix4ShardIndex(&v))
}
/// 就地更新 AccessPath 上的访问条件，追加分片前缀。
pub fn addExprPrefixCond(path: &mut AccessPath, adder: &exprPrefixAdder) {
    path.access_conditions = adder.addExprPrefix4CNFCond(&path.access_conditions)
}
/// 递归下推：折叠 Selection，穿过 Projection/Sort/Limit/TopN，否则挂到当前节点。
fn push(mut p: PlanNode, mut incoming: Vec<Expression>) -> (PlanNode, bool) {
    // Selection 自身不保留：条件并入 incoming 后继续下推到唯一孩子。
    if p.kind == PlanKind::Selection {
        incoming.append(&mut p.conditions);
        if p.children.len() == 1 {
            return push(p.children.remove(0), incoming);
        }
    }
    let mut changed = !incoming.is_empty();
    if p.children.is_empty() {
        p.conditions.extend(incoming);
        return (p, changed);
    }
    // 这些算子不消费过滤语义，谓词可穿透到孩子。
    if matches!(
        p.kind,
        PlanKind::Projection | PlanKind::Sort | PlanKind::Limit | PlanKind::TopN
    ) {
        let (c, x) = push(p.children.remove(0), incoming);
        p.children.push(c);
        changed |= x
    } else if !incoming.is_empty() {
        p.conditions.extend(incoming)
    }
    (p, changed)
}
