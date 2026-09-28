// Copyright 2022 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// IndexMerge（索引合并）访问路径的生成与清理。
//
// 在启用 IndexMerge 或存在相关 Hint 时，为普通索引、多值索引（MV Index，
// 面向 JSON 数组等多值列）以及组合路径生成 AND/OR 合并方案，并按 Hint/全文
// 索引约束裁剪候选 AccessPath。

use crate::find_best_task::{AccessPath, DataSource, Datum, IndexInfo, Range};
use crate::task::{Expression, FieldType, TypeCode};
use std::collections::{HashMap, HashSet};

/// 本模块统一的错误类型别名。
pub type Result<T> = std::result::Result<T, String>;
/// 过滤类型：未识别。
pub const UNSPECIFIED_FILTER_TP: i32 = 0;
/// 过滤类型：非 MV 索引上的 EQ/IN。
pub const EQ_OR_IN_NON_MV_TP: i32 = 1;
/// 过滤类型：MV 索引多值 OR（如 JSON_OVERLAPS）。
pub const MULTI_VALUES_OR_MV_TP: i32 = 2;
/// 过滤类型：MV 索引多值 AND（如 JSON_CONTAINS）。
pub const MULTI_VALUES_AND_MV_TP: i32 = 3;
/// 过滤类型：MV 索引单值成员（如 MEMBER OF）。
pub const SINGLE_VALUE_MV_TP: i32 = 4;

/// IndexMerge Hint：可指定参与合并的索引下标列表。
#[derive(Clone, Debug, Default)]
pub struct IndexMergeHint {
    pub indexes: Vec<usize>,
}
/// 生成 IndexMerge 路径时的数据源包装：开关、Hint、告警与全文索引集合。
#[derive(Clone, Debug, Default)]
pub struct IndexMergeDataSource {
    pub source: DataSource,
    pub index_merge_hints: Vec<IndexMergeHint>,
    pub enable_index_merge: bool,
    pub no_index_merge_hint: bool,
    pub temporary_table: bool,
    pub warnings: Vec<String>,
    pub fts_indexes: HashSet<usize>,
}

/// 入口：按开关与 Hint 生成 OR/AND/MV/组合 IndexMerge 路径并做清理。
pub fn generateIndexMergePath(ds: &mut IndexMergeDataSource) -> Result<()> {
    let mut warning;
    // 未启用且无 Hint，或显式禁用 / 临时表：只记录告警，不生成路径。
    if (!ds.enable_index_merge && ds.index_merge_hints.is_empty()) || ds.no_index_merge_hint {
        warning = "IndexMerge is inapplicable or disabled".into();
    } else if ds.temporary_table {
        warning = "IndexMerge cannot be used on a temporary table".into();
    } else {
        // 依次生成 OR、MV-AND、组合 AND，并在 Hint 场景裁剪非合并路径。
        let regular = ds.source.paths.len();
        let conditions = ds.source.conditions.clone();
        warning = generateOtherIndexMerge(ds, regular, &conditions)?;
        generateANDIndexMerge4MVIndex(ds, regular, &conditions)?;
        let before_composed = ds.source.paths.len();
        generateANDIndexMerge4ComposedIndex(ds, regular, &conditions)?;
        if ds.source.paths.len() > regular
            && ds.source.conditions.len() > ds.source.pushed_down_conditions.len()
        {
            let max_rows = ds.source.paths[regular..]
                .iter()
                .map(|p| p.count_after_access)
                .fold(0.0_f64, f64::max);
            if ds.source.stats.row_count > max_rows {
                ds.source.stats.row_count = max_rows;
            }
        }
        if !ds.index_merge_hints.is_empty() {
            if ds.source.paths.len() == regular {
                if warning.is_empty() {
                    warning = "IndexMerge is inapplicable".into();
                }
            } else {
                let start = if ds.source.paths.len() > before_composed {
                    before_composed
                } else {
                    regular
                };
                ds.source.paths.drain(..start);
            }
        }
        cleanAccessPathForMVIndexHint(ds);
        cleanAccessPathForFTS(ds)?;
    }
    if !ds.index_merge_hints.is_empty() && !warning.is_empty() {
        ds.index_merge_hints.clear();
        ds.warnings.push(warning);
    }
    Ok(())
}

/// 用单条条件在普通（非向量）索引上生成局部路径；返回是否需保留表过滤。
pub fn generateNormalIndexPartialPath(
    ds: &IndexMergeDataSource,
    item: &Expression,
    candidate: &AccessPath,
) -> (Option<AccessPath>, bool) {
    if candidate.index.as_ref().is_some_and(|i| i.vector) {
        return (None, false);
    }
    let cnf = split_cnf(item);
    let (pushed, rejected): (Vec<_>, Vec<_>) = cnf.into_iter().partition(is_pushable);
    (
        accessPathsForConds(ds, &pushed, candidate),
        !rejected.is_empty(),
    )
}
/// 判断索引是否被 IndexMerge Hint 允许（空 Hint 表示全部允许）。
pub fn isInIndexMergeHints(ds: &IndexMergeDataSource, index: usize) -> bool {
    ds.index_merge_hints.is_empty()
        || ds
            .index_merge_hints
            .iter()
            .any(|h| h.indexes.is_empty() || h.indexes.contains(&index))
}
/// 是否存在显式指定了索引列表的 IndexMerge Hint。
pub fn indexMergeHintsHasSpecifiedIdx(ds: &IndexMergeDataSource) -> bool {
    ds.index_merge_hints.iter().any(|h| !h.indexes.is_empty())
}
/// 索引是否出现在某个显式 IndexMerge Hint 的索引列表中。
pub fn isSpecifiedInIndexMergeHints(ds: &IndexMergeDataSource, index: usize) -> bool {
    ds.index_merge_hints
        .iter()
        .any(|h| h.indexes.contains(&index))
}

/// 将条件绑定到候选路径上构造新的 AccessPath（含 range 与行数估算）。
pub fn accessPathsForConds(
    ds: &IndexMergeDataSource,
    conditions: &[Expression],
    path: &AccessPath,
) -> Option<AccessPath> {
    let offset = ds
        .source
        .paths
        .iter()
        .position(|p| std::ptr::eq(p, path))
        .unwrap_or(0);
    if !isInIndexMergeHints(ds, offset) || conditions.is_empty() {
        return None;
    }
    let mut result = path.clone();
    result.access_conditions = conditions.to_vec();
    result.table_filters.clear();
    result.ranges = conditions.iter().filter_map(range_from_filter).collect();
    if result.ranges.is_empty() {
        return None;
    }
    result.count_after_access =
        (path.count_after_access / (conditions.len() as f64 + 1.0)).max(1.0);
    Some(result)
}

/// AND 场景下收集普通索引局部路径列表。
pub fn generateNormalIndexPartialPath4And(
    ds: &IndexMergeDataSource,
    normalPathCnt: usize,
    used: &mut HashMap<String, Expression>,
) -> Vec<AccessPath> {
    generateANDIndexMerge4NormalIndex(ds, normalPathCnt, used)
        .map(|p| p.partial_index_paths)
        .unwrap_or_default()
}
/// 按显式 Hint 将多条普通索引路径合并为一条 AND IndexMerge。
pub fn generateANDIndexMerge4NormalIndex(
    ds: &IndexMergeDataSource,
    normalPathCnt: usize,
    used: &mut HashMap<String, Expression>,
) -> Option<AccessPath> {
    if !indexMergeHintsHasSpecifiedIdx(ds) {
        return None;
    }
    let composed = !used.is_empty();
    let mut partial = Vec::new();
    for (offset, path) in ds.source.paths.iter().take(normalPathCnt).enumerate() {
        let Some(index) = &path.index else { continue };
        if index.multi_valued || !isSpecifiedInIndexMergeHints(ds, offset) || path.full_range() {
            continue;
        }
        if composed
            && path
                .access_conditions
                .iter()
                .all(|c| used.contains_key(&expr_hash(c)))
        {
            continue;
        }
        for condition in &path.access_conditions {
            used.entry(expr_hash(condition))
                .or_insert_with(|| condition.clone());
        }
        partial.push(path.clone());
    }
    if partial.is_empty() || (partial.len() == 1 && !composed) {
        return None;
    }
    Some(merge_path(partial, Vec::new(), false))
}

/// 为 AND 组合收集 MV 索引局部路径，并记录已使用过滤。
pub fn generateMVIndexMergePartialPaths4And(
    ds: &IndexMergeDataSource,
    normalPathCnt: usize,
    filters: &[Expression],
) -> Result<(Vec<AccessPath>, HashMap<String, Expression>)> {
    let mut records = Vec::new();
    let mut used = HashMap::new();
    for (offset, path) in ds.source.paths.iter().take(normalPathCnt).enumerate() {
        if !isMVIndexPath(path) || !isInIndexMergeHints(ds, offset) {
            continue;
        }
        let Some(index) = &path.index else { continue };
        let Some(columns) = PrepareIdxColsAndUnwrapArrayType(index, &ds.source.columns, true)
        else {
            continue;
        };
        let (access, _, kind) = collectFilters4MVIndex(filters, &columns);
        if access.is_empty() || !matches!(kind, MULTI_VALUES_AND_MV_TP | SINGLE_VALUE_MV_TP) {
            continue;
        }
        if let Some(parts) = buildPartialPaths4MVIndexWithPath(ds, path, &access, kind)? {
            for c in access {
                used.insert(expr_hash(&c), c);
            }
            records.extend(parts);
        }
    }
    Ok((records, used))
}

/// 生成 OR 类 IndexMerge（委托 unfinished_path）；无新增路径时返回告警文案。
pub fn generateOtherIndexMerge(
    ds: &mut IndexMergeDataSource,
    regularPathCount: usize,
    filters: &[Expression],
) -> Result<String> {
    crate::indexmerge_unfinished_path::generateORIndexMerge(ds, filters)?;
    if ds.source.paths.len() == regularPathCount {
        Ok("No available filter or index for IndexMerge".into())
    } else {
        Ok(String::new())
    }
}
/// 组合 MV 与普通索引局部路径，生成 AND IndexMerge。
pub fn generateANDIndexMerge4ComposedIndex(
    ds: &mut IndexMergeDataSource,
    normalPathCnt: usize,
    filters: &[Expression],
) -> Result<()> {
    let (mv, mut used) = generateMVIndexMergePartialPaths4And(ds, normalPathCnt, filters)?;
    let normal = generateNormalIndexPartialPath4And(ds, normalPathCnt, &mut used);
    let mut partial = mv;
    partial.extend(normal);
    if partial.len() > 1 {
        ds.source
            .paths
            .push(merge_path(partial, uncovered(filters, &used), false));
    }
    Ok(())
}
/// 为每个 MV 索引路径单独生成 AND IndexMerge。
pub fn generateANDIndexMerge4MVIndex(
    ds: &mut IndexMergeDataSource,
    normalPathCnt: usize,
    filters: &[Expression],
) -> Result<()> {
    let originals = ds
        .source
        .paths
        .iter()
        .take(normalPathCnt)
        .cloned()
        .collect::<Vec<_>>();
    for path in originals {
        if !isMVIndexPath(&path) {
            continue;
        }
        let Some(index) = &path.index else { continue };
        let Some(columns) = PrepareIdxColsAndUnwrapArrayType(index, &ds.source.columns, true)
        else {
            continue;
        };
        let (access, remaining, kind) = collectFilters4MVIndex(filters, &columns);
        if access.is_empty() {
            continue;
        }
        if let Some(partial) = buildPartialPaths4MVIndexWithPath(ds, &path, &access, kind)? {
            ds.source.paths.push(merge_path(
                partial,
                remaining,
                kind == MULTI_VALUES_AND_MV_TP,
            ));
        }
    }
    Ok(())
}

/// 用单个值表达式与过滤构造一条 MV 索引局部路径。
pub fn buildPartialPathUp4MVIndex(
    base: &AccessPath,
    value: &Expression,
    filters: &[Expression],
) -> Option<AccessPath> {
    let mut path = base.clone();
    path.access_conditions = filters.to_vec();
    path.ranges = vec![range_from_filter(value)?];
    path.count_after_access = (base.count_after_access / 2.0).max(1.0);
    Some(path)
}
/// 准备索引列后调用 `buildPartialPaths4MVIndex`。
pub fn buildPartialPaths4MVIndexWithPath(
    ds: &IndexMergeDataSource,
    path: &AccessPath,
    access: &[Expression],
    kind: i32,
) -> Result<Option<Vec<AccessPath>>> {
    let Some(index) = &path.index else {
        return Ok(None);
    };
    let Some(columns) = PrepareIdxColsAndUnwrapArrayType(index, &ds.source.columns, true) else {
        return Ok(None);
    };
    buildPartialPaths4MVIndex(path, access, &columns, kind)
}
/// 按访问过滤中的值展开多条 MV 局部路径（单值类型只取首条过滤）。
pub fn buildPartialPaths4MVIndex(
    path: &AccessPath,
    access: &[Expression],
    columns: &[usize],
    kind: i32,
) -> Result<Option<Vec<AccessPath>>> {
    let mut result = Vec::new();
    for condition in access {
        let values = filter_values(condition);
        if values.is_empty() {
            continue;
        }
        for value in values {
            let expr = Expression {
                name: format!("eq:{value}"),
                column: columns.first().copied(),
                ..Expression::default()
            };
            if let Some(part) = buildPartialPath4MVIndex(path, &expr, access) {
                result.push(part);
            }
        }
        if kind == SINGLE_VALUE_MV_TP {
            break;
        }
    }
    Ok((!result.is_empty()).then_some(result))
}
/// 判断值类型到索引类型的转换对 MV 范围是否安全。
pub fn isSafeTypeConversion4MVIndexRange(value: &FieldType, index: &FieldType) -> bool {
    fn eval_type(code: &TypeCode) -> u8 {
        match code {
            TypeCode::Null => 0,
            TypeCode::Int | TypeCode::UInt => 1,
            TypeCode::Float => 2,
            TypeCode::Decimal => 3,
            TypeCode::String | TypeCode::Bytes => 4,
            TypeCode::Vector => 5,
        }
    }

    eval_type(&value.code) == eval_type(&index.code)
}
/// 单值 MV 局部路径的薄封装。
pub fn buildPartialPath4MVIndex(
    path: &AccessPath,
    value: &Expression,
    access: &[Expression],
) -> Option<AccessPath> {
    buildPartialPathUp4MVIndex(path, value, access)
}
/// 取出索引列（可选要求 MV），并与表列求交。
pub fn PrepareIdxColsAndUnwrapArrayType(
    index: &IndexInfo,
    tableColumns: &[usize],
    require_mv: bool,
) -> Option<Vec<usize>> {
    if require_mv && !index.multi_valued {
        return None;
    }
    let columns: Vec<_> = index
        .columns
        .iter()
        .copied()
        .filter(|c| tableColumns.is_empty() || tableColumns.contains(c))
        .collect();
    (!columns.is_empty()).then_some(columns)
}

/// 将过滤分为可访问与剩余，并判定 MV 过滤种类。
pub fn collectFilters4MVIndex(
    filters: &[Expression],
    indexColumns: &[usize],
) -> (Vec<Expression>, Vec<Expression>, i32) {
    let mut access = Vec::new();
    let mut remained = Vec::new();
    let mut kind = UNSPECIFIED_FILTER_TP;
    for filter in filters {
        let (ok, current) = checkAccessFilter4IdxCol(filter, indexColumns.first().copied());
        if ok {
            if kind == UNSPECIFIED_FILTER_TP {
                kind = current;
            }
            if current == kind || current == SINGLE_VALUE_MV_TP {
                access.push(filter.clone());
            } else {
                remained.push(filter.clone());
            }
        } else {
            remained.push(filter.clone());
        }
    }
    (access, remained, kind)
}
/// 可变版本：把 `collectFilters4MVIndex` 结果追加到输出向量。
pub fn CollectFilters4MVIndexMutations(
    filters: &[Expression],
    indexColumns: &[usize],
    access: &mut Vec<Expression>,
    remaining: &mut Vec<Expression>,
) -> i32 {
    let (a, r, kind) = collectFilters4MVIndex(filters, indexColumns);
    access.extend(a);
    remaining.extend(r);
    kind
}
/// 按 IndexMerge Hint 指定索引裁剪已生成的 IndexMerge 路径。
pub fn cleanAccessPathForMVIndexHint(ds: &mut IndexMergeDataSource) {
    if ds.index_merge_hints.is_empty() {
        return;
    }
    let specified: HashSet<_> = ds
        .index_merge_hints
        .iter()
        .flat_map(|h| h.indexes.iter().copied())
        .collect();
    if specified.is_empty() {
        return;
    }
    let valid = ds
        .source
        .paths
        .iter()
        .filter(|path| indexMergeContainSpecificIndex(path, &specified))
        .cloned()
        .collect::<Vec<_>>();
    if !valid.is_empty() {
        ds.source.paths = valid;
    }
}
/// 按全文检索（FTS）索引集合裁剪 IndexMerge 路径。
pub fn cleanAccessPathForFTS(ds: &mut IndexMergeDataSource) -> Result<()> {
    if ds.fts_indexes.is_empty() {
        return Ok(());
    }
    ds.source
        .paths
        .retain(|path| path.store == Some(crate::task::StoreType::TiFlash));
    if ds.source.paths.is_empty() {
        return Err("Full text search can be only executed in a columnar storage (TiFlash), but it is not available".into());
    }
    Ok(())
}
/// 判断 IndexMerge 路径的局部索引是否命中指定列/索引集合。
pub fn indexMergeContainSpecificIndex(path: &AccessPath, indexes: &HashSet<usize>) -> bool {
    path.partial_index_paths.iter().any(|part| {
        indexMergeContainSpecificIndex(part, indexes)
            || part.index.as_ref().is_some_and(|index| {
                index
                    .columns
                    .first()
                    .is_some_and(|column| indexes.contains(column))
            })
    })
}

/// 检查过滤是否可用于指定索引列，并返回过滤类型常量。
pub fn checkAccessFilter4IdxCol(filter: &Expression, column: Option<usize>) -> (bool, i32) {
    if column.is_some() && filter.column != column {
        return (false, UNSPECIFIED_FILTER_TP);
    }
    // 按过滤函数名映射到 MV/非 MV 过滤类型常量。
    let name = filter.name.to_ascii_lowercase();
    if name.starts_with("member-of:") {
        (true, SINGLE_VALUE_MV_TP)
    } else if name.starts_with("json-contains:") {
        classify_mv_values(filter, MULTI_VALUES_AND_MV_TP)
    } else if name.starts_with("json-overlaps:") {
        classify_mv_values(filter, MULTI_VALUES_OR_MV_TP)
    } else if name.starts_with("eq:") || name.starts_with("in:") {
        (true, EQ_OR_IN_NON_MV_TP)
    } else {
        (false, UNSPECIFIED_FILTER_TP)
    }
}

fn classify_mv_values(filter: &Expression, multi_value_type: i32) -> (bool, i32) {
    match filter_values(filter).len() {
        0 => (false, UNSPECIFIED_FILTER_TP),
        1 => (true, SINGLE_VALUE_MV_TP),
        _ => (true, multi_value_type),
    }
}
/// 将 JSON 数组表达式拆成目标类型的常量表达式列表。
pub fn jsonArrayExpr2Exprs(
    expression: &Expression,
    targetType: &FieldType,
) -> Result<Vec<Expression>> {
    filter_values(expression)
        .into_iter()
        .map(|value| {
            jsonValue2Expr(&value, targetType)
                .ok_or_else(|| format!("JSON value {value} cannot be converted"))
        })
        .collect()
}
/// 把单个 JSON 文本值转为对应 FieldType 的表达式。
pub fn jsonValue2Expr(value: &str, targetType: &FieldType) -> Option<Expression> {
    let valid = match targetType.code {
        TypeCode::Int => value.parse::<i64>().is_ok(),
        TypeCode::UInt => value.parse::<u64>().is_ok(),
        TypeCode::Float | TypeCode::Decimal => value.parse::<f64>().is_ok(),
        TypeCode::String | TypeCode::Bytes => true,
        _ => false,
    };
    valid.then(|| Expression {
        name: value.into(),
        return_type: Some(targetType.clone()),
        ..Expression::default()
    })
}
/// 剥掉 `json-cast:` 前缀，还原被包装的表达式。
pub fn unwrapJSONCast(expression: &Expression) -> Option<Expression> {
    expression
        .name
        .strip_prefix("json-cast:")
        .map(|name| Expression {
            name: name.into(),
            ..expression.clone()
        })
}
/// 路径是否指向多值（multi-valued）索引。
pub fn isMVIndexPath(path: &AccessPath) -> bool {
    path.index.as_ref().is_some_and(|i| i.multi_valued)
}

/// 组装 IndexMerge AccessPath；intersection 为真时取行数最小值，否则求和。
fn merge_path(
    partial: Vec<AccessPath>,
    filters: Vec<Expression>,
    intersection: bool,
) -> AccessPath {
    let count = if intersection {
        partial
            .iter()
            .map(|p| p.count_after_access)
            .fold(f64::INFINITY, f64::min)
    } else {
        partial.iter().map(|p| p.count_after_access).sum()
    };
    AccessPath {
        partial_index_paths: partial,
        table_filters: filters,
        count_after_access: count,
        ..AccessPath::default()
    }
}
/// 返回尚未被局部路径覆盖的过滤条件。
fn uncovered(filters: &[Expression], used: &HashMap<String, Expression>) -> Vec<Expression> {
    filters
        .iter()
        .filter(|f| !used.contains_key(&expr_hash(f)))
        .cloned()
        .collect()
}
/// 过滤表达式的简易哈希键（名字+列）。
fn expr_hash(expression: &Expression) -> String {
    format!(
        "{}:{:?}",
        expression.name.to_ascii_lowercase(),
        expression.column
    )
}
/// 是否可下推到索引侧（排除 root-only / 非确定性）。
fn is_pushable(expression: &Expression) -> bool {
    !expression.name.starts_with("root-only:") && !expression.name.starts_with("nondeterministic:")
}
/// 按 `and:` 前缀拆分合取式。
fn split_cnf(expression: &Expression) -> Vec<Expression> {
    expression
        .name
        .strip_prefix("and:")
        .map(|s| {
            s.split('|')
                .map(|name| Expression {
                    name: name.into(),
                    column: expression.column,
                    ..Expression::default()
                })
                .collect()
        })
        .unwrap_or_else(|| vec![expression.clone()])
}
/// 按 `or:` 前缀拆分析取式；无前缀则返回空。
pub(crate) fn split_dnf(expression: &Expression) -> Vec<Expression> {
    expression
        .name
        .strip_prefix("or:")
        .map(|s| {
            s.split('|')
                .map(|name| Expression {
                    name: name.into(),
                    column: expression.column,
                    ..Expression::default()
                })
                .collect()
        })
        .unwrap_or_default()
}
/// 从 `name:values` 形式解析过滤中的值列表。
fn filter_values(expression: &Expression) -> Vec<String> {
    expression
        .name
        .split_once(':')
        .map(|(_, values)| {
            values
                .trim_matches(['[', ']'])
                .split(',')
                .filter(|v| !v.is_empty())
                .map(|v| v.trim().to_string())
                .collect()
        })
        .unwrap_or_default()
}
/// 用过滤首个值构造点查 Range。
fn range_from_filter(expression: &Expression) -> Option<Range> {
    let value = filter_values(expression).into_iter().next()?;
    let datum = value
        .parse::<i64>()
        .map(Datum::Int)
        .unwrap_or_else(|_| Datum::Bytes(value.into_bytes()));
    Some(Range {
        low: vec![datum.clone()],
        high: vec![datum],
        ..Range::default()
    })
}
