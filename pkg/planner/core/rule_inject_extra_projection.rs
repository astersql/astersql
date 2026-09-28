// Copyright 2017 PingCAP, Inc. Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 向算子下方注入额外 Projection 的物理计划改写。
//
// 某些物理算子（聚合、排序、TopN、UnionAll）要求输入侧先物化表达式或统一
// schema。本规则在这些算子下方插入 Projection（投影），把复杂表达式提前计算，
// 或把 NominalSort（仅表达排序需求、不真正排序的名义排序）转为投影。

use crate::task::{Expression, PlanKind, PlanNode};
/// 对整棵计划树注入额外投影的公开入口。
pub fn InjectExtraProjection(plan: PlanNode) -> PlanNode {
    NewProjInjector().inject(plan)
}
/// 投影注入器：按算子种类决定是否在子树下插入 Projection。
#[derive(Default)]
pub struct projInjector;
/// 构造默认投影注入器。
pub fn NewProjInjector() -> projInjector {
    projInjector
}
impl projInjector {
    /// 自底向上递归注入：先处理孩子，再按本节点类型改写。
    pub fn inject(&self, mut p: PlanNode) -> PlanNode {
        p.children = p.children.into_iter().map(|c| self.inject(c)).collect();
        match p.kind {
            PlanKind::UnionAll => injectProjBelowUnion(p),
            PlanKind::HashAgg | PlanKind::StreamAgg => {
                let funcs = p.agg_funcs.clone();
                let groups = p.group_items.clone();
                InjectProjBelowAgg(p, &funcs, &groups)
            }
            PlanKind::Sort | PlanKind::TopN => {
                let items = p.by_items.clone();
                InjectProjBelowSort(p, &items)
            }
            PlanKind::NominalSort => {
                let items = p.by_items.clone();
                TurnNominalSortIntoProj(p, false, &items)
            }
            _ => p,
        }
    }
}
/// 在 UnionAll 各分支下按需插入类型转换投影，统一到父 schema。
pub fn injectProjBelowUnion(mut p: PlanNode) -> PlanNode {
    // Go only normalizes UnionAll inputs for the TiFlash/MPP implementation.
    if p.store != crate::task::StoreType::TiFlash {
        return p;
    }
    let schema = p.schema.clone();
    p.children = p
        .children
        .into_iter()
        .map(|child| {
            if child.schema.len() != schema.len() || child.schema == schema {
                child
            } else {
                let expressions = schema
                    .iter()
                    .zip(&child.schema)
                    .enumerate()
                    .map(|(i, (dst, src))| {
                        if src == dst {
                            column(i, src.clone())
                        } else {
                            Expression {
                                name: format!("cast(col_{i})"),
                                column: Some(i),
                                function_count: 1,
                                return_type: Some(dst.clone()),
                                ..Default::default()
                            }
                        }
                    })
                    .collect();
                let mut projected = projection(child, expressions);
                projected.schema = schema.clone();
                projected
            }
        })
        .collect();
    p
}
/// 若聚合函数或分组项含非列表达式，则在聚合下方插入投影先求值。
pub fn InjectProjBelowAgg(
    mut p: PlanNode,
    funcs: &[Expression],
    groups: &[Expression],
) -> PlanNode {
    if !funcs.iter().chain(groups).any(is_scalar) {
        return p;
    }
    if p.children.len() == 1 {
        let mut exprs = Vec::new();
        let mut materialize = |expression: &Expression| {
            if is_constant(expression) {
                return expression.clone();
            }
            let index = exprs
                .iter()
                .position(|candidate| same_expression(candidate, expression))
                .unwrap_or_else(|| {
                    exprs.push(expression.clone());
                    exprs.len() - 1
                });
            column(
                index,
                expression.return_type.clone().unwrap_or_else(default_type),
            )
        };
        p.agg_funcs = funcs.iter().map(&mut materialize).collect();
        p.group_items = groups.iter().map(&mut materialize).collect();
        let child = p.children.remove(0);
        p.children.push(projection(child, exprs));
    }
    p
}
/// 若排序/TopN 键含非列表达式，则在下方插入投影。
pub fn InjectProjBelowSort(mut p: PlanNode, items: &[Expression]) -> PlanNode {
    if !items.iter().any(is_scalar) {
        return p;
    }
    if p.children.len() != 1 {
        return p;
    }
    let child = p.children.remove(0);
    let original_schema = p.schema.clone();
    let mut bottom_exprs = child
        .schema
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, field_type)| column(index, field_type))
        .collect::<Vec<_>>();
    let mut bottom_schema = child.schema.clone();
    p.by_items = items
        .iter()
        .map(|item| {
            if !is_scalar(item) {
                return item.clone();
            }
            let index = bottom_exprs.len();
            bottom_exprs.push(item.clone());
            let field_type = item.return_type.clone().unwrap_or_else(default_type);
            bottom_schema.push(field_type.clone());
            column(index, field_type)
        })
        .collect();
    let mut bottom = projection(child, bottom_exprs);
    bottom.schema = bottom_schema;
    p.children.push(bottom);

    let top_exprs = original_schema
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, field_type)| column(index, field_type))
        .collect();
    let mut top = projection(p, top_exprs);
    top.schema = original_schema;
    top
}
/// 将 NominalSort 转为 Projection；`onlyColumn` 为真时直接透传子计划。
pub fn TurnNominalSortIntoProj(
    mut p: PlanNode,
    onlyColumn: bool,
    items: &[Expression],
) -> PlanNode {
    if p.children.len() != 1 {
        return p;
    }
    let child = p.children.remove(0);
    if onlyColumn {
        return child;
    }
    let child_schema = child.schema.clone();
    let mut bottom_exprs = child_schema
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, field_type)| column(index, field_type))
        .collect::<Vec<_>>();
    let mut bottom_schema = child_schema.clone();
    for item in items.iter().filter(|item| is_scalar(item)) {
        bottom_exprs.push(item.clone());
        bottom_schema.push(item.return_type.clone().unwrap_or_else(default_type));
    }
    let mut bottom = projection(child, bottom_exprs);
    bottom.schema = bottom_schema;
    let top_exprs = child_schema
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, field_type)| column(index, field_type))
        .collect();
    let mut top = projection(bottom, top_exprs);
    top.schema = child_schema;
    top
}
/// 构造以 `child` 为输入、带给定表达式列表的 Projection 节点。
fn projection(child: PlanNode, expressions: Vec<Expression>) -> PlanNode {
    let mut p = PlanNode::new(PlanKind::Projection);
    p.schema = expressions
        .iter()
        .map(|expression| expression.return_type.clone().unwrap_or_else(default_type))
        .collect();
    p.stats = child.stats.clone();
    p.expected_count = child.expected_count;
    p.expressions = expressions;
    p.children.push(child);
    p
}

fn is_scalar(expression: &Expression) -> bool {
    expression.function_count > 0
}

fn is_constant(expression: &Expression) -> bool {
    expression.column.is_none() && expression.function_count == 0
}

fn same_expression(left: &Expression, right: &Expression) -> bool {
    left.name == right.name
        && left.column == right.column
        && left.function_count == right.function_count
        && left.virtual_column == right.virtual_column
        && left.return_type == right.return_type
}

fn column(index: usize, return_type: crate::task::FieldType) -> Expression {
    Expression {
        name: format!("col_{index}"),
        column: Some(index),
        return_type: Some(return_type),
        ..Default::default()
    }
}

fn default_type() -> crate::task::FieldType {
    crate::task::FieldType {
        code: crate::task::TypeCode::Null,
        flen: 0,
        decimal: 0,
        unsigned: false,
    }
}
