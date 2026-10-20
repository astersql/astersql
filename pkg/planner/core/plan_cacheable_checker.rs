// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// 计划缓存可缓存性检查。
//
// 判断 AST（抽象语法树）或物理执行计划（execution plan）是否适合放入
// Plan Cache（计划缓存）。Prepared 与 Non-Prepared 路径分别检查子查询、
// 全局 SET、常量参数数量、计划内存占用及特定不可缓存算子。

use crate::{AstNode, PlanKind, PlanNode, StoreType, ast};

/// Column metadata needed to validate the narrowly supported COALESCE UPDATE shape.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoalesceColumnType {
    pub name: String,
    pub field_type: types_dependency::metadata::FieldType,
}

impl CoalesceColumnType {
    pub fn new(name: impl Into<String>, field_type: types_dependency::metadata::FieldType) -> Self {
        Self {
            name: name.into().to_ascii_lowercase(),
            field_type,
        }
    }
}

fn coalesce_column_type<'a>(
    columns: &'a [CoalesceColumnType],
    name: &ast::ColumnName,
) -> Option<&'a types_dependency::metadata::FieldType> {
    columns
        .iter()
        .find(|column| column.name == name.Name.L)
        .map(|column| &column.field_type)
}

fn same_coalesce_column_type(
    expected: &types_dependency::metadata::FieldType,
    actual: &types_dependency::metadata::FieldType,
) -> bool {
    use types_dependency::metadata::mysql;

    const VALUE_FLAGS: usize = mysql::NotNullFlag | mysql::UnsignedFlag | mysql::ZerofillFlag;
    expected.GetType() == actual.GetType()
        && expected.GetFlen() == actual.GetFlen()
        && expected.GetDecimal() == actual.GetDecimal()
        && expected.GetCharset() == actual.GetCharset()
        && expected.GetCollate() == actual.GetCollate()
        && expected.GetFlag() & VALUE_FLAGS == actual.GetFlag() & VALUE_FLAGS
}

/// Reports whether an UPDATE assignment is the exact numeric COALESCE shape admitted by Go.
pub fn SupportedCoalesceAssignment(
    assignment: &ast::Assignment,
    columns: &[CoalesceColumnType],
) -> bool {
    use types_dependency::metadata::mysql;

    let Some(target) = coalesce_column_type(columns, &assignment.Column) else {
        return false;
    };
    if !matches!(
        target.GetType(),
        mysql::TypeTiny
            | mysql::TypeShort
            | mysql::TypeInt24
            | mysql::TypeLong
            | mysql::TypeLonglong
            | mysql::TypeNewDecimal
    ) {
        return false;
    }

    fn supported(
        expression: &ast::ExprNode,
        target: &types_dependency::metadata::FieldType,
        columns: &[CoalesceColumnType],
    ) -> bool {
        let ast::ExprKind::Function { FnName, Args, .. } = &expression.Kind else {
            return false;
        };
        if FnName.L != "coalesce" || Args.len() != 2 {
            return false;
        }
        let first_supported = match &Args[0].Kind {
            ast::ExprKind::Value(value) => matches!(
                value.Datum,
                ast::ValueDatum::Int64(_)
                    | ast::ValueDatum::Uint64(_)
                    | ast::ValueDatum::Decimal(_)
            ),
            _ => supported(&Args[0], target, columns),
        };
        if !first_supported {
            return false;
        }
        match &Args[1].Kind {
            ast::ExprKind::Column(column) => coalesce_column_type(columns, column)
                .is_some_and(|fallback| same_coalesce_column_type(target, fallback)),
            _ => supported(&Args[1], target, columns),
        }
    }

    supported(&assignment.Expr, target, columns)
}

/// Returns source offsets of parameter markers nested inside COALESCE calls.
pub fn CoalesceParamMarkerOffsets(assignment: &ast::Assignment) -> Vec<usize> {
    fn collect(expression: &ast::ExprNode, coalesce_depth: usize, offsets: &mut Vec<usize>) {
        match &expression.Kind {
            ast::ExprKind::ParamMarker { Offset } if coalesce_depth > 0 => offsets.push(*Offset),
            ast::ExprKind::Function { FnName, Args, .. } => {
                let depth = coalesce_depth + usize::from(FnName.L == "coalesce");
                for argument in Args {
                    collect(argument, depth, offsets);
                }
            }
            _ => {}
        }
    }

    let mut offsets = Vec::new();
    collect(&assignment.Expr, 0, &mut offsets);
    offsets.sort_unstable();
    offsets.dedup();
    offsets
}

/// 简化入口：仅返回 AST 是否可缓存（忽略失败原因）。
pub fn Cacheable(node: &AstNode) -> bool {
    CacheableWithCtx(node, false, true).0
}

/// 带上下文的 AST 可缓存性检查。
///
/// `subquery` 为 false 时禁止子查询；`param_limit` 为 false 时直接判定不可缓存。
/// 返回 `(是否可缓存, 不可缓存原因)`。
pub fn CacheableWithCtx(node: &AstNode, subquery: bool, param_limit: bool) -> (bool, String) {
    // 递归遍历 AST，命中不可缓存节点时返回静态原因字符串。
    fn visit(n: &AstNode, s: bool) -> Option<&'static str> {
        match n {
            AstNode::Subquery(_) if !s => Some("query has sub-queries is un-cacheable"),
            AstNode::Set { global: true } => Some("global SET is un-cacheable"),
            AstNode::Other {
                read_only: false, ..
            } => Some("unsupported node"),
            AstNode::Select(v) | AstNode::Do(v) | AstNode::Subquery(v) => {
                v.iter().find_map(|n| visit(n, s))
            }
            AstNode::Explain(n) => visit(n, s),
            AstNode::AggregateFunc { args, .. } | AstNode::WindowFunc { args, .. } => {
                args.iter().find_map(|n| visit(n, s))
            }
            AstNode::Other {
                read_only: true,
                children,
            } => children.iter().find_map(|n| visit(n, s)),
            _ => None,
        }
    }
    if !matches!(
        node,
        AstNode::Select(_)
            | AstNode::Insert { .. }
            | AstNode::Update { .. }
            | AstNode::Delete { .. }
    ) {
        return (
            false,
            "not a SELECT/UPDATE/INSERT/DELETE/SET statement".into(),
        );
    }
    if !param_limit {
        return (false, "parameterized limit disabled".into());
    }
    visit(node, subquery).map_or((true, String::new()), |r| (false, r.into()))
}

/// Non-Prepared Plan Cache 专用检查：先统计常量个数，超限则不可缓存。
///
/// 未超限时再调用 `CacheableWithCtx`（默认禁止子查询）。
pub fn NonPreparedPlanCacheableWithCtx(node: &AstNode, max_params: usize) -> (bool, String) {
    let mut count = 0;
    // 统计 Value 节点数量作为“常量参数”个数。
    fn walk(n: &AstNode, c: &mut usize) {
        if matches!(n, AstNode::Value(_)) {
            *c += 1;
        }
        match n {
            AstNode::Select(v) | AstNode::Do(v) | AstNode::Subquery(v) => {
                for n in v {
                    walk(n, c);
                }
            }
            AstNode::Explain(n) => walk(n, c),
            AstNode::AggregateFunc { args, .. } | AstNode::WindowFunc { args, .. } => {
                for n in args {
                    walk(n, c);
                }
            }
            AstNode::Other { children, .. } => {
                for n in children {
                    walk(n, c);
                }
            }
            _ => {}
        }
    }
    walk(node, &mut count);
    if count > max_params {
        (false, "query has too many constants".into())
    } else {
        CacheableWithCtx(node, false, true)
    }
}

/// 检查物理计划树是否可放入计划缓存。
///
/// 拒绝过大计划、带参数的 TableDual，以及 Apply/Shuffle 等不可缓存物理算子。
pub fn isPlanCacheable(plan: &PlanNode, param_num: usize, max_size: i64) -> (bool, String) {
    if plan.MemoryUsage() > max_size && max_size > 0 {
        return (
            false,
            "plan is too large(decided by the variable @@tidb_plan_cache_max_plan_size)".into(),
        );
    }
    if matches!(plan.kind, PlanKind::Dual) && param_num > 0 {
        return (false, "get a TableDual plan".into());
    }
    if matches!(plan.kind, PlanKind::TableReader) && plan.store_type == StoreType::TiFlash {
        return (false, "TiFlash plan is un-cacheable".into());
    }
    if matches!(plan.kind, PlanKind::Apply) {
        return (false, "PhysicalApply plan is un-cacheable".into());
    }
    if matches!(
        plan.kind,
        PlanKind::Shuffle { .. } | PlanKind::ShuffleReceiver { .. }
    ) {
        return (false, "get a Shuffle plan".into());
    }
    // 递归检查子计划；任一子节点不可缓存则整体失败。
    for child in &plan.children {
        let r = isPlanCacheable(child, param_num, max_size);
        if !r.0 {
            return r;
        }
    }
    (true, String::new())
}
