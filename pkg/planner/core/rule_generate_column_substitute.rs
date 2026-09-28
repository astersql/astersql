// Copyright 2018 PingCAP, Inc. Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 生成列（Generated Column）表达式替换规则。
//
// 生成列是由表达式定义的虚拟/存储列。若计划中再次出现与生成列定义相同的
// 表达式，可直接引用该列下标，避免重复计算。匹配时对表达式名做大小写不敏感
// 比较，并在整棵计划树上收集 `virtual_column` 映射后统一替换。

use crate::task::{Expression, PlanNode};
use std::collections::HashMap;
/// 表达式名（小写）→ 列下标 的映射表。
pub type ExprColumnMap = HashMap<String, usize>;
/// 生成列替换器：收集虚拟列映射并在计划树中做表达式替换。
#[derive(Default)]
pub struct GcSubstituter;
impl GcSubstituter {
    /// 先收集生成列映射，再对整棵计划做替换。
    pub fn Optimize(&self, mut p: PlanNode) -> (PlanNode, bool) {
        let mut map = HashMap::new();
        collectGenerateColumn(&p, &mut map);
        if !map.is_empty() {
            self.substitute(&mut p, &map);
        }
        // Go mutates the logical plan in place and deliberately leaves planChanged false.
        (p, false)
    }
    /// 递归遍历：对本节点 expressions/conditions/by_items/group_items 尝试替换后下推子节点。
    pub fn substitute(&self, p: &mut PlanNode, map: &ExprColumnMap) -> bool {
        let mut changed = false;
        for e in p
            .expressions
            .iter_mut()
            .chain(p.conditions.iter_mut())
            .chain(p.by_items.iter_mut())
            .chain(p.group_items.iter_mut())
            .chain(p.agg_funcs.iter_mut())
        {
            changed |= substituteExpression(e, map);
        }
        for c in &mut p.children {
            changed |= self.substitute(c, map)
        }
        changed
    }
    /// 返回规则注册名。
    pub fn Name(&self) -> &'static str {
        "generate_column_substitute"
    }
}
/// 递归收集计划中所有 `virtual_column` 表达式到映射表。
pub fn collectGenerateColumn(p: &PlanNode, map: &mut ExprColumnMap) {
    if p.kind == crate::task::PlanKind::Cte {
        return;
    }
    for c in &p.children {
        collectGenerateColumn(c, map)
    }
    if !matches!(&p.kind, crate::task::PlanKind::Other(kind) if kind == "DataSource") {
        return;
    }
    for e in &p.expressions {
        if e.virtual_column {
            if let Some(c) = e.column {
                map.insert(e.name.to_ascii_lowercase(), c);
            }
        }
    }
}
/// 若表达式名与候选生成列定义匹配，则改写为列引用。
pub fn tryToSubstituteExpr(expr: &mut Expression, candidateExpr: &str, column: usize) -> bool {
    if expr.name.eq_ignore_ascii_case(candidateExpr) {
        expr.column = Some(column);
        expr.name = format!("col_{column}");
        true
    } else {
        false
    }
}
/// `substituteExpression` 的公开别名（兼容 Go 侧导出命名）。
pub fn SubstituteExpression(cond: &mut Expression, map: &ExprColumnMap) -> bool {
    substituteExpression(cond, map)
}
/// 按映射表尝试把表达式替换为对应生成列引用。
pub fn substituteExpression(cond: &mut Expression, map: &ExprColumnMap) -> bool {
    let key = cond.name.to_ascii_lowercase();
    map.get(&key)
        .is_some_and(|c| tryToSubstituteExpr(cond, &key, *c))
}
