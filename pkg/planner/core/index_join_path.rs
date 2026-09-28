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
// Copyright 2026 AsterSQL.

// Index Join（索引嵌套循环连接）内侧访问路径与范围构建。
//
// Index Join 用外表连接键探测内表索引。本模块根据内表下推条件与连接键构造
// 连续索引前缀范围（Range），支持计划缓存下的可变范围重建，并在超出
// `tidb_opt_range_max_size` 时回退（range fallback）。

use crate::find_best_task::{AccessPath, DataSource, Datum, Range};
use crate::task::{Expression, StatsInfo};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

/// 本模块统一的错误类型别名。
pub type Result<T> = std::result::Result<T, String>;

/// 计划缓存场景下可重建的 Index Join 范围包装。
#[derive(Clone, Debug, Default)]
pub struct mutableIndexJoinRange {
    pub ranges: Vec<Range>,
    pub rangeInfo: String,
    pub indexJoinInfo: indexJoinPathInfo,
    pub path: AccessPath,
}
impl mutableIndexJoinRange {
    /// 为计划缓存做浅克隆（结构体 `Clone`）。
    pub fn CloneForPlanCache(&self) -> Self {
        self.clone()
    }
    /// 返回当前范围副本。
    pub fn Range(&self) -> Vec<Range> {
        self.ranges.clone()
    }
    /// 按缓存的路径信息重建范围；宽度变化或空范围视为失败。
    pub fn Rebuild(&mut self, context: &IndexJoinContext) -> Result<()> {
        let (result, empty) = indexJoinPathBuild(context, &self.path, &self.indexJoinInfo, true)?;
        if empty {
            return Err("failed to rebuild range: empty range".into());
        }
        let result = result.ok_or_else(|| "failed to rebuild range: missing path".to_string())?;
        if self.ranges.len() != result.chosenRanges.len()
            || self.ranges.first().map(|r| r.low.len())
                != result.chosenRanges.first().map(|r| r.low.len())
        {
            return Err("failed to rebuild range: range width changed".into());
        }
        self.rangeInfo = indexJoinPathRangeInfo(&self.indexJoinInfo.outerJoinKeys, &result);
        self.ranges = result.chosenRanges;
        Ok(())
    }
}

/// 一次 Index Join 路径构建的结果：选中路径、访问/剩余条件与范围等。
#[derive(Clone, Debug)]
pub struct indexJoinPathResult {
    pub chosenPath: AccessPath,
    pub chosenAccess: Vec<Expression>,
    pub chosenRemained: Vec<Expression>,
    pub chosenRanges: Vec<Range>,
    pub usedColsLen: usize,
    pub eqUsedColsNDV: f64,
    pub idxOff2KeyOff: Vec<i32>,
    pub lastColManager: Option<ColWithCmpFuncManager>,
    pub mutableRange: Option<mutableIndexJoinRange>,
}
/// 构建 Index Join 路径所需的连接键、下推条件与统计信息。
#[derive(Clone, Debug, Default)]
pub struct indexJoinPathInfo {
    pub joinOtherConditions: Vec<Expression>,
    pub outerJoinKeys: Vec<usize>,
    pub innerJoinKeys: Vec<usize>,
    pub innerSchema: Vec<usize>,
    pub innerPushedConditions: Vec<Expression>,
    pub innerTableStats: Option<StatsInfo>,
    pub columnNDV: HashMap<usize, f64>,
}
/// Index Join 范围构建上下文：内存上限、计划缓存开关与回退状态。
#[derive(Clone, Debug, Default)]
pub struct IndexJoinContext {
    pub rangeMaxSize: usize,
    pub planCacheEnabled: bool,
    pub expectedCount: f64,
    rangeFallback: Cell<bool>,
    rangeFallbackWarnings: RefCell<Vec<String>>,
}
impl IndexJoinContext {
    /// 记录因范围内存超限触发的回退告警。
    fn RecordRangeFallback(&self) {
        self.rangeFallback.set(true);
        self.rangeFallbackWarnings.borrow_mut().push(format!(
            "'tidb_opt_range_max_size' exceeded when building ranges: {}",
            self.rangeMaxSize
        ));
    }

    /// 是否已发生范围回退。
    pub fn HasRangeFallback(&self) -> bool {
        self.rangeFallback.get()
    }

    /// 返回范围回退告警列表副本。
    pub fn RangeFallbackWarnings(&self) -> Vec<String> {
        self.rangeFallbackWarnings.borrow().clone()
    }

    /// 清空范围回退标志与告警。
    pub fn ResetRangeFallback(&self) {
        self.rangeFallback.set(false);
        self.rangeFallbackWarnings.borrow_mut().clear();
    }
}
/// 管理索引下一列上的相关不等式条件（用于扩展范围尾部）。
#[derive(Clone, Debug, Default)]
pub struct ColWithCmpFuncManager {
    pub targetColumn: usize,
    pub affectedColumns: Vec<usize>,
    pub conditions: Vec<Expression>,
    pub desc: bool,
}
/// 构建过程中的临时状态：可用键、未用列与索引偏移到连接键映射。
#[derive(Clone, Debug, Default)]
struct indexJoinPathTmp {
    curPossibleUsedKeys: Vec<usize>,
    curNotUsedIndexCols: Vec<usize>,
    curNotUsedColLens: Vec<Option<usize>>,
    curIdxOff2KeyOff: Vec<i32>,
}
/// 临时范围：点查组合、空范围标记与已纳入的键/EQ 计数。
#[derive(Clone, Debug, Default)]
struct indexJoinTmpRange {
    ranges: Vec<Range>,
    emptyRange: bool,
    keyCntInRange: usize,
    eqAndInCntInRange: usize,
    nextColInRange: bool,
    extraColInRange: bool,
}

/// 计划缓存且条件含参数时包装为可变范围；否则返回原始 ranges。
fn indexJoinPathNewMutableRange(
    context: &IndexJoinContext,
    info: &indexJoinPathInfo,
    related: &[Expression],
    ranges: Vec<Range>,
    path: &AccessPath,
) -> (Vec<Range>, Option<mutableIndexJoinRange>) {
    if context.planCacheEnabled && related.iter().any(is_parameterized) {
        let wrapper = mutableIndexJoinRange {
            ranges: ranges.clone(),
            rangeInfo: String::new(),
            indexJoinInfo: info.clone(),
            path: path.clone(),
        };
        (ranges, Some(wrapper))
    } else {
        (ranges, None)
    }
}
/// 按临时范围裁剪已用键映射，并拆分已选/剩余访问条件。
fn indexJoinPathUpdateTmpRange(
    tmp: &mut indexJoinPathTmp,
    range: &indexJoinTmpRange,
    accesses: &[Expression],
    mut remained: Vec<Expression>,
) -> (usize, Vec<Expression>, Vec<Expression>) {
    let last = range.keyCntInRange + range.eqAndInCntInRange;
    tmp.curPossibleUsedKeys.truncate(range.keyCntInRange);
    for offset in tmp.curIdxOff2KeyOff.iter_mut().skip(last) {
        *offset = -1;
    }
    let selected = accesses
        .iter()
        .take(range.eqAndInCntInRange)
        .cloned()
        .collect();
    append_conditions(
        &mut remained,
        accesses.iter().skip(range.eqAndInCntInRange).cloned(),
    );
    (last, selected, remained)
}

/// 核心：按索引列顺序拼接点前缀与可选区间/相关条件，产出路径结果。
pub fn indexJoinPathBuild(
    context: &IndexJoinContext,
    path: &AccessPath,
    info: &indexJoinPathInfo,
    rebuildMode: bool,
) -> Result<(Option<indexJoinPathResult>, bool)> {
    // 恒假下推条件：空范围，直接返回 empty。
    if info
        .innerPushedConditions
        .iter()
        .any(|condition| condition.name == "false")
    {
        return Ok((None, true));
    }

    let columns = if path.index_columns.is_empty() {
        path.index
            .as_ref()
            .map(|index| index.columns.clone())
            .unwrap_or_default()
    } else {
        path.index_columns.clone()
    };
    if columns.is_empty() {
        return Ok((None, false));
    }
    let lengths = path
        .index
        .as_ref()
        .map(|index| index.prefix_lengths.clone())
        .unwrap_or_else(|| vec![None; columns.len()]);

    let range_limit = if rebuildMode { 0 } else { context.rangeMaxSize };
    let mut build = indexJoinPathTmpInit(info, &columns, &lengths);

    // Go first extracts all EQ/IN predicates and then removes every join key
    // and predicate after the first uncovered index column.  Keeping this as a
    // separate pass is important: a later join key must not bridge a gap.
    let mut eq_by_column = HashMap::<usize, Expression>::new();
    for condition in &info.innerPushedConditions {
        if is_eq_or_in(condition) {
            if let Some(column) = condition.column {
                eq_by_column
                    .entry(column)
                    .or_insert_with(|| condition.clone());
            }
        }
    }
    let mut point_conditions = Vec::<Expression>::new();
    let mut continuous = true;
    let mut static_eq_allowed = true;
    let mut invalidated_join_key = false;
    build.curPossibleUsedKeys.clear();
    for (offset, column) in columns.iter().copied().enumerate() {
        if build.curIdxOff2KeyOff[offset] >= 0 {
            if continuous {
                build.curPossibleUsedKeys.push(column);
            } else {
                build.curIdxOff2KeyOff[offset] = -1;
                invalidated_join_key = true;
            }
        } else if continuous {
            if let Some(condition) = eq_by_column.get(&column).filter(|_| static_eq_allowed) {
                point_conditions.push(condition.clone());
                if lengths.get(offset).copied().flatten().is_some() {
                    // A prefix EQ/IN can remain in the point template and a
                    // following dynamic join key can still be appended, but
                    // another static predicate must not extend the range.
                    static_eq_allowed = false;
                }
            } else {
                continuous = false;
            }
        }
    }

    let matched_keys = build.curPossibleUsedKeys.len();
    if matched_keys == 0 && !info.innerJoinKeys.is_empty() {
        return Ok((None, false));
    }

    let point_condition_keys: HashSet<_> = point_conditions.iter().map(expression_key).collect();
    let mut ranges = vec![Range::default()];
    let mut accesses = Vec::<Expression>::new();
    let mut used_keys = 0usize;
    let mut stopped_for_fallback = false;

    // Build the continuous point prefix in index order.  Each append checks
    // the memory limit independently, exactly like appendTailTemplateRange /
    // AppendRanges2PointRanges in Go, so fallback retains the valid prefix.
    for (offset, column) in columns.iter().copied().enumerate() {
        if build.curIdxOff2KeyOff[offset] >= 0 {
            let (next, fallback) =
                append_point_column(ranges, &[Datum::Null], range_limit, context);
            ranges = next;
            if fallback {
                stopped_for_fallback = true;
                break;
            }
            used_keys += 1;
            continue;
        }
        let Some(condition) = point_conditions
            .iter()
            .find(|condition| condition.column == Some(column))
        else {
            break;
        };
        let mut values = condition_values(condition);
        if values.is_empty() {
            return Ok((None, true));
        }
        let prefix = lengths.get(offset).copied().flatten();
        if let Some(prefix_length) = prefix {
            for value in &mut values {
                truncate_datum_to_prefix(value, prefix_length);
            }
        }
        let (next, fallback) = append_point_column(ranges, &values, range_limit, context);
        ranges = next;
        if fallback {
            stopped_for_fallback = true;
            break;
        }
        accesses.push(condition.clone());
    }

    let mut used_columns = ranges.first().map(|range| range.low.len()).unwrap_or(0);
    if used_keys == 0 || used_columns == 0 {
        return Ok((None, false));
    }
    for offset in used_columns..build.curIdxOff2KeyOff.len() {
        build.curIdxOff2KeyOff[offset] = -1;
    }

    let mut used_condition_keys: HashSet<_> = accesses.iter().map(expression_key).collect();
    let mut manager = None;
    if !stopped_for_fallback {
        let next_offset = used_columns;
        if next_offset < columns.len() {
            let next_column = columns[next_offset];
            let correlated: Vec<_> = info
                .joinOtherConditions
                .iter()
                .filter(|condition| {
                    condition.column == Some(next_column)
                        && is_inequality(condition)
                        && is_pushable_correlated_condition(condition)
                })
                .cloned()
                .collect();
            if !correlated.is_empty() {
                let (next, fallback) =
                    append_point_column(ranges, &[Datum::Null], range_limit, context);
                ranges = next;
                if !fallback {
                    accesses.extend(correlated.iter().cloned());
                    manager = Some(ColWithCmpFuncManager {
                        targetColumn: next_column,
                        affectedColumns: correlated
                            .iter()
                            .filter_map(|condition| condition.column)
                            .collect(),
                        conditions: correlated.clone(),
                        desc: correlated.iter().any(|condition| {
                            condition.name.starts_with("lt:") || condition.name.starts_with("le:")
                        }),
                    });
                    used_columns += 1;
                }
            } else {
                let range_conditions: Vec<_> = info
                    .innerPushedConditions
                    .iter()
                    .filter(|condition| {
                        condition.column == Some(next_column) && is_inequality(condition)
                    })
                    .cloned()
                    .collect();
                if let Some(interval) = build_column_interval(
                    &range_conditions,
                    lengths.get(next_offset).copied().flatten(),
                ) {
                    let (next, fallback) =
                        append_interval_column(ranges, &interval, range_limit, context);
                    ranges = next;
                    if !fallback {
                        accesses.extend(range_conditions.iter().cloned());
                        used_condition_keys.extend(range_conditions.iter().map(expression_key));
                        used_columns += 1;
                    }
                } else if invalidated_join_key && accesses.is_empty() {
                    // The reduced Rust range model spells Go's open bound as a
                    // trailing NULL datum.  Only a genuine join-key gap needs
                    // this marker; an ordinary completed point prefix does not.
                    let (next, _) =
                        append_point_column(ranges, &[Datum::Null], range_limit, context);
                    ranges = next;
                    used_columns = ranges[0].low.len();
                }
            }
        }
    }

    let remained = info
        .innerPushedConditions
        .iter()
        .filter(|condition| {
            let key = expression_key(condition);
            !used_condition_keys.contains(&key)
                || (point_condition_keys.contains(&key)
                    && condition
                        .column
                        .and_then(|column| {
                            columns
                                .iter()
                                .position(|candidate| *candidate == column)
                                .and_then(|offset| lengths.get(offset).copied().flatten())
                        })
                        .is_some())
                || (is_inequality(condition)
                    && condition
                        .column
                        .and_then(|column| {
                            columns
                                .iter()
                                .position(|candidate| *candidate == column)
                                .and_then(|offset| lengths.get(offset).copied().flatten())
                        })
                        .is_some())
        })
        .cloned()
        .collect();
    // 访问条件 + Join 其它条件：计划缓存参数化时包装为可变范围。
    let mut related = accesses.clone();
    related.extend(info.joinOtherConditions.clone());
    let (ranges, mutable) = indexJoinPathNewMutableRange(context, info, &related, ranges, path);
    Ok((
        Some(indexJoinPathConstructResult(
            path,
            accesses,
            remained,
            ranges,
            used_columns,
            build.curIdxOff2KeyOff,
            manager,
            mutable,
            info,
        )),
        false,
    ))
}

/// 是否为等值或 IN 谓词。
fn is_eq_or_in(condition: &Expression) -> bool {
    condition.name.starts_with("eq:") || condition.name.starts_with("in:")
}

/// 以名字与列 ID 作为表达式去重键。
fn expression_key(condition: &Expression) -> (String, Option<usize>) {
    (condition.name.clone(), condition.column)
}

/// 粗估 ranges 内存占用（条数 × 宽度 × 16）。
fn ranges_mem_usage(ranges: &[Range]) -> usize {
    ranges
        .len()
        .saturating_mul(ranges.first().map(|range| range.low.len()).unwrap_or(0))
        .saturating_mul(16)
}

/// 将点值笛卡尔扩展到现有 ranges；超限则回退并保留原 ranges。
fn append_point_column(
    ranges: Vec<Range>,
    values: &[Datum],
    range_max_size: usize,
    context: &IndexJoinContext,
) -> (Vec<Range>, bool) {
    let mut result = Vec::with_capacity(ranges.len().saturating_mul(values.len()));
    for range in &ranges {
        for value in values {
            let mut next = range.clone();
            next.low.push(value.clone());
            next.high.push(value.clone());
            result.push(next);
        }
    }
    if range_max_size > 0 && ranges_mem_usage(&result) > range_max_size {
        context.RecordRangeFallback();
        (ranges, true)
    } else {
        (result, false)
    }
}

/// 将区间边界追加到现有 ranges；超限则回退。
fn append_interval_column(
    ranges: Vec<Range>,
    interval: &Range,
    range_max_size: usize,
    context: &IndexJoinContext,
) -> (Vec<Range>, bool) {
    let mut result = Vec::with_capacity(ranges.len());
    for range in &ranges {
        let mut next = range.clone();
        next.low.extend(interval.low.iter().cloned());
        next.high.extend(interval.high.iter().cloned());
        next.low_exclusive = interval.low_exclusive;
        next.high_exclusive = interval.high_exclusive;
        result.push(next);
    }
    if range_max_size > 0 && ranges_mem_usage(&result) > range_max_size {
        context.RecordRangeFallback();
        (ranges, true)
    } else {
        (result, false)
    }
}

/// 从不等式条件构造单列区间，并按前缀长度截断。
fn build_column_interval(conditions: &[Expression], prefix_length: Option<usize>) -> Option<Range> {
    if conditions.is_empty() {
        return None;
    }
    let lower = conditions
        .iter()
        .find(|condition| condition.name.starts_with("gt:") || condition.name.starts_with("ge:"));
    let upper = conditions
        .iter()
        .find(|condition| condition.name.starts_with("lt:") || condition.name.starts_with("le:"));
    let mut low = lower
        .and_then(|condition| condition_values(condition).into_iter().next())
        .unwrap_or(Datum::Null);
    let mut high = upper
        .and_then(|condition| condition_values(condition).into_iter().next())
        .unwrap_or_else(|| Datum::Bytes(vec![0xff]));
    if let Some(prefix_length) = prefix_length {
        truncate_datum_to_prefix(&mut low, prefix_length);
        truncate_datum_to_prefix(&mut high, prefix_length);
    }
    Some(Range {
        low: vec![low],
        high: vec![high],
        low_exclusive: lower.is_some_and(|condition| condition.name.starts_with("gt:")),
        high_exclusive: prefix_length.is_none()
            && upper.is_some_and(|condition| condition.name.starts_with("lt:")),
    })
}

/// 相关条件是否可下推（排除含 cast 的表达式）。
fn is_pushable_correlated_condition(condition: &Expression) -> bool {
    !condition.name.contains("cast")
}

/// 将字节/字符串 Datum 截断到前缀索引长度。
fn truncate_datum_to_prefix(value: &mut Datum, prefix_length: usize) {
    let Datum::Bytes(bytes) = value else {
        return;
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        bytes.truncate(prefix_length.min(bytes.len()));
        return;
    };
    *bytes = text
        .chars()
        .take(prefix_length)
        .collect::<String>()
        .into_bytes();
}

/// 比较两条路径结果，决定 `current` 是否优于 `best`。
pub fn indexJoinPathCompare(
    _ds: &DataSource,
    best: Option<&indexJoinPathResult>,
    current: Option<&indexJoinPathResult>,
) -> bool {
    match (best, current) {
        (_, None) => false,
        (_, Some(current)) if current.chosenRanges.is_empty() => false,
        (None, Some(_)) => true,
        (Some(best), Some(current)) => indexJoinPathCmp4UnComparableOnes(best, current),
    }
}
/// 按 Go 的 NDV、已用列数与 Join Key 覆盖数依次决胜。
pub fn indexJoinPathCmp4UnComparableOnes(
    best: &indexJoinPathResult,
    current: &indexJoinPathResult,
) -> bool {
    if !isNDVClose(current.eqUsedColsNDV, best.eqUsedColsNDV) {
        return current.eqUsedColsNDV > best.eqUsedColsNDV;
    }
    if current.usedColsLen != best.usedColsLen {
        return current.usedColsLen > best.usedColsLen;
    }
    let current_cover = current
        .idxOff2KeyOff
        .iter()
        .filter(|offset| **offset != -1)
        .count();
    let best_cover = best
        .idxOff2KeyOff
        .iter()
        .filter(|offset| **offset != -1)
        .count();
    if current_cover != best_cover {
        return current_cover > best_cover;
    }
    current.eqUsedColsNDV > best.eqUsedColsNDV
}
/// Go 的 NDV 接近规则：小值域、绝对差或 20% 相对差任一满足即可。
pub fn isNDVClose(lhs: f64, rhs: f64) -> bool {
    if lhs == 0.0 || rhs == 0.0 {
        return lhs == rhs;
    }
    let min = lhs.min(rhs);
    let max = lhs.max(rhs);
    let diff = (lhs - rhs).abs();
    max <= 20.0 || (diff < 200.0 && min >= 20.0) || diff / max < 0.2
}

/// 组装 `indexJoinPathResult`，并用连接键 NDV 估算等值列基数。
fn indexJoinPathConstructResult(
    path: &AccessPath,
    access: Vec<Expression>,
    remained: Vec<Expression>,
    ranges: Vec<Range>,
    used: usize,
    mapping: Vec<i32>,
    manager: Option<ColWithCmpFuncManager>,
    mutable: Option<mutableIndexJoinRange>,
    info: &indexJoinPathInfo,
) -> indexJoinPathResult {
    let ndv = info
        .innerJoinKeys
        .iter()
        .take(used)
        .map(|k| info.columnNDV.get(k).copied().unwrap_or(1.0))
        .fold(1.0, f64::max);
    indexJoinPathResult {
        chosenPath: path.clone(),
        chosenAccess: access,
        chosenRemained: remained,
        chosenRanges: ranges,
        usedColsLen: used,
        eqUsedColsNDV: ndv,
        idxOff2KeyOff: mapping,
        lastColManager: manager,
        mutableRange: mutable,
    }
}
/// 由 EQ/IN 条件枚举点组合生成临时范围，受 rangeMaxSize 约束。
fn indexJoinPathBuildTmpRange(
    tmp: &indexJoinPathTmp,
    eqIn: &[Expression],
    candidates: &[Expression],
    rangeMaxSize: usize,
) -> indexJoinTmpRange {
    let key_count = tmp.curPossibleUsedKeys.len();
    let mut combinations = vec![Vec::<Datum>::new()];
    for condition in eqIn {
        let values = condition_values(condition);
        if values.is_empty() {
            continue;
        }
        let mut next = Vec::new();
        for prefix in &combinations {
            for value in &values {
                let mut row = prefix.clone();
                row.push(value.clone());
                let width = row.len();
                next.push(row);
                if rangeMaxSize > 0
                    && next.len().saturating_mul(width).saturating_mul(16) > rangeMaxSize
                {
                    return indexJoinTmpRange::default();
                }
            }
        }
        combinations = next;
    }
    if combinations.iter().any(Vec::is_empty) {
        combinations = vec![vec![Datum::Null; key_count]];
    }
    let ranges = combinations
        .into_iter()
        .map(|row| Range {
            low: row.clone(),
            high: row,
            ..Range::default()
        })
        .collect();
    indexJoinTmpRange {
        ranges,
        emptyRange: eqIn.iter().any(|e| e.name == "false"),
        keyCntInRange: key_count,
        eqAndInCntInRange: eqIn.len(),
        nextColInRange: !candidates.is_empty(),
        extraColInRange: candidates.len() > 1,
    }
}
/// 整型主键 Index Join 的范围信息描述字符串。
pub fn indexJoinIntPKRangeInfo(outerJoinKeys: &[usize]) -> String {
    outerJoinKeys
        .first()
        .map(|key| format!("range: decided by outer row column {key}"))
        .unwrap_or_else(|| "range: full integer handle".into())
}
/// 根据外表连接键与构建结果生成范围信息文本。
pub fn indexJoinPathRangeInfo(outerJoinKeys: &[usize], result: &indexJoinPathResult) -> String {
    let mut columns = Vec::new();
    for offset in &result.idxOff2KeyOff {
        if *offset >= 0 {
            columns.push(
                outerJoinKeys
                    .get(*offset as usize)
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "?".into()),
            );
        }
    }
    format!(
        "range: [{}]; access: {}",
        columns.join(","),
        result
            .chosenAccess
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>()
            .join(",")
    )
}
/// 取范围信息，并判断是否可推断最多一行。
pub fn indexJoinPathGetRangeInfoAndMaxOneRow(
    info: &indexJoinPathInfo,
    result: &indexJoinPathResult,
) -> (String, bool) {
    let max_one = result
        .chosenPath
        .index
        .as_ref()
        .is_some_and(|index| index.unique && result.usedColsLen == index.columns.len())
        && result
            .chosenAccess
            .last()
            .is_none_or(|condition| condition.name.starts_with("eq:"));
    (indexJoinPathRangeInfo(&info.outerJoinKeys, result), max_one)
}

/// 初始化临时构建状态：索引列到连接键偏移映射。
fn indexJoinPathTmpInit(
    info: &indexJoinPathInfo,
    indexColumns: &[usize],
    lengths: &[Option<usize>],
) -> indexJoinPathTmp {
    let mut mapping = vec![-1; indexColumns.len()];
    let mut used = Vec::new();
    let mut not_used = Vec::new();
    let mut not_used_lens = Vec::new();
    for (offset, column) in indexColumns.iter().enumerate() {
        if let Some(key) = info.innerJoinKeys.iter().position(|inner| inner == column) {
            mapping[offset] = key as i32;
            used.push(*column);
        } else {
            not_used.push(*column);
            not_used_lens.push(lengths.get(offset).cloned().flatten());
        }
    }
    indexJoinPathTmp {
        curPossibleUsedKeys: used,
        curNotUsedIndexCols: not_used,
        curNotUsedColLens: not_used_lens,
        curIdxOff2KeyOff: mapping,
    }
}
/// 从候选条件中找出对当前索引前缀有用的 EQ/IN。
fn indexJoinPathFindUsefulEQIn(
    info: &indexJoinPathInfo,
    tmp: &indexJoinPathTmp,
) -> (Vec<Expression>, Vec<Expression>, Vec<Expression>, bool) {
    let unused: HashSet<_> = tmp.curNotUsedIndexCols.iter().copied().collect();
    let mut useful = Vec::new();
    let mut remained = Vec::new();
    let mut range_candidates = Vec::new();
    for condition in &info.innerPushedConditions {
        if condition.name == "false" {
            return (Vec::new(), Vec::new(), Vec::new(), true);
        }
        if condition.column.is_some_and(|c| unused.contains(&c))
            && (condition.name.starts_with("eq:") || condition.name.starts_with("in:"))
        {
            useful.push(condition.clone());
        } else if condition.column.is_some_and(|c| unused.contains(&c)) && is_inequality(condition)
        {
            range_candidates.push(condition.clone());
        } else {
            remained.push(condition.clone());
        }
    }
    (useful, remained, range_candidates, false)
}
/// 为下一索引列收集相关比较条件，构造 ColWithCmpFuncManager。
fn indexJoinPathBuildColManager(
    info: &indexJoinPathInfo,
    tmp: &indexJoinPathTmp,
    lastColPos: usize,
) -> Option<ColWithCmpFuncManager> {
    let column = tmp
        .curNotUsedIndexCols
        .get(lastColPos.saturating_sub(tmp.curPossibleUsedKeys.len()))
        .copied()?;
    let conditions: Vec<_> = info
        .joinOtherConditions
        .iter()
        .filter(|e| e.column == Some(column) && is_inequality(e))
        .cloned()
        .collect();
    (!conditions.is_empty()).then(|| ColWithCmpFuncManager {
        targetColumn: column,
        affectedColumns: conditions.iter().filter_map(|e| e.column).collect(),
        desc: conditions
            .iter()
            .any(|e| e.name.starts_with("lt:") || e.name.starts_with("le:")),
        conditions,
    })
}
/// 去掉无法落入连续索引前缀的 EQ/IN，避免跨缺口使用连接键。
fn indexJoinPathRemoveUselessEQIn(
    tmp: &mut indexJoinPathTmp,
    indexColumns: &[usize],
    eqIn: Vec<Expression>,
) -> (Vec<Expression>, Vec<Expression>) {
    let mut by_column: HashMap<usize, Expression> = eqIn
        .into_iter()
        .filter_map(|e| e.column.map(|c| (c, e)))
        .collect();
    let mut useful = Vec::new();
    let mut reached_gap = false;
    for column in indexColumns {
        if tmp.curPossibleUsedKeys.contains(column) {
            continue;
        }
        if reached_gap {
            continue;
        }
        if let Some(condition) = by_column.remove(column) {
            let prefix_limited = tmp
                .curNotUsedIndexCols
                .iter()
                .position(|candidate| candidate == column)
                .and_then(|offset| tmp.curNotUsedColLens.get(offset))
                .is_some_and(Option::is_some);
            useful.push(condition);
            if prefix_limited {
                reached_gap = true;
            }
        } else {
            reached_gap = true;
        }
    }
    (useful, by_column.into_values().collect())
}

/// 获取整型主键 Index Join 的路径与范围信息。
pub fn getIndexJoinIntPKPathInfo(
    ds: &DataSource,
    innerJoinKeys: &[usize],
    outerJoinKeys: &[usize],
    pushed: &[Expression],
) -> Option<indexJoinPathResult> {
    let path = ds.paths.iter().find(|p| p.is_int_handle)?;
    let handle = path
        .index_columns
        .first()
        .copied()
        .or_else(|| ds.columns.first().copied())?;
    let offset = innerJoinKeys.iter().position(|key| *key == handle)?;
    let datum = Datum::Int(0);
    Some(indexJoinPathResult {
        chosenPath: path.clone(),
        chosenAccess: Vec::new(),
        chosenRemained: pushed.to_vec(),
        chosenRanges: vec![Range {
            low: vec![datum.clone()],
            high: vec![datum],
            ..Range::default()
        }],
        usedColsLen: 1,
        eqUsedColsNDV: ds.stats.row_count,
        idxOff2KeyOff: vec![offset as i32],
        lastColManager: None,
        mutableRange: None,
    })
    .filter(|_| outerJoinKeys.get(offset).is_some())
}
/// 按简化物理属性上下文挑选最佳内侧 Index Join 访问路径。
pub fn getBestIndexJoinInnerTaskByProp(
    ds: &DataSource,
    context: &IndexJoinContext,
    info: &indexJoinPathInfo,
) -> Result<Option<AccessPath>> {
    Ok(getBestIndexJoinPathResultByProp(ds, context, info)?.map(|r| r.chosenPath))
}
/// 按物理属性在候选路径中选出最佳 Index Join 构建结果。
pub fn getBestIndexJoinPathResultByProp(
    ds: &DataSource,
    context: &IndexJoinContext,
    info: &indexJoinPathInfo,
) -> Result<Option<indexJoinPathResult>> {
    let mut best = getIndexJoinIntPKPathInfo(
        ds,
        &info.innerJoinKeys,
        &info.outerJoinKeys,
        &info.innerPushedConditions,
    );
    for path in &ds.paths {
        if path.is_int_handle {
            continue;
        }
        let (current, empty) = indexJoinPathBuild(context, path, info, false)?;
        if empty {
            return Ok(current);
        }
        if indexJoinPathCompare(ds, best.as_ref(), current.as_ref()) {
            best = current;
        }
    }
    Ok(best)
}
/// 向模板范围追加尾部占位列（对齐 Go appendTailTemplateRange）。
pub fn appendTailTemplateRange(
    mut originRanges: Vec<Range>,
    rangeMaxSize: usize,
) -> (Vec<Range>, bool) {
    if originRanges.is_empty() {
        return (originRanges, false);
    }
    let estimated = originRanges
        .len()
        .saturating_mul(originRanges[0].low.len().saturating_add(1))
        .saturating_mul(16);
    if rangeMaxSize > 0 && estimated > rangeMaxSize {
        return (originRanges, true);
    }
    for range in &mut originRanges {
        range.low.push(Datum::Null);
        range.high.push(Datum::Null);
    }
    (originRanges, false)
}

/// 从表达式名字解析 EQ/IN 的常量值列表。
fn condition_values(expression: &Expression) -> Vec<Datum> {
    expression
        .name
        .split_once(':')
        .map(|(_, values)| {
            values
                .trim_matches(['[', ']'])
                .split(',')
                .filter(|v| !v.is_empty())
                .map(|v| {
                    v.trim()
                        .parse::<i64>()
                        .map(Datum::Int)
                        .unwrap_or_else(|_| Datum::Bytes(v.trim().as_bytes().to_vec()))
                })
                .collect()
        })
        .unwrap_or_default()
}
/// 表达式是否含计划缓存参数占位。
fn is_parameterized(expression: &Expression) -> bool {
    expression.name.contains('?') || expression.name.starts_with("param:")
}
/// 是否为不等式比较谓词。
fn is_inequality(expression: &Expression) -> bool {
    ["lt:", "le:", "gt:", "ge:"]
        .iter()
        .any(|prefix| expression.name.starts_with(prefix))
}
/// 将迭代器中的条件追加到目标向量。
fn append_conditions(
    target: &mut Vec<Expression>,
    conditions: impl IntoIterator<Item = Expression>,
) {
    let mut keys: HashSet<_> = target.iter().map(|e| (e.name.clone(), e.column)).collect();
    for condition in conditions {
        if keys.insert((condition.name.clone(), condition.column)) {
            target.push(condition);
        }
    }
}
