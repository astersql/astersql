// Copyright 2026 AsterSQL.
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
// IndexMerge 在 OR（析取）谓词下的未完成访问路径构建。
//
// IndexMerge（索引合并）并行探测多条索引路径再合并行号。本文件处理顶层 OR 分支：
// 先生成未完成路径（unfinished path），再与 AND 条件合并，最终选出各分支最优
// AccessPath（访问路径，描述索引范围扫描方式）。

use crate::find_best_task::AccessPath;
use crate::indexmerge_path::{
    EQ_OR_IN_NON_MV_TP, IndexMergeDataSource, MULTI_VALUES_OR_MV_TP, SINGLE_VALUE_MV_TP,
    accessPathsForConds, buildPartialPaths4MVIndexWithPath, checkAccessFilter4IdxCol,
    collectFilters4MVIndex, generateNormalIndexPartialPath, isMVIndexPath, split_dnf,
};
use crate::task::Expression;
use std::cmp::Ordering;
use std::collections::HashSet;

/// 对过滤条件中的 OR 列表尝试生成 IndexMerge 访问路径并追加到数据源。
pub fn generateORIndexMerge(
    ds: &mut IndexMergeDataSource,
    filters: &[Expression],
) -> Result<(), String> {
    let candidate_count = ds.source.paths.len();
    // 仅处理可拆成至少两个分支的 DNF（析取范式）条件。
    for (or_offset, condition) in filters.iter().enumerate() {
        let branches = split_dnf(condition);
        if branches.len() < 2 {
            continue;
        }
        let candidates = ds.source.paths[..candidate_count].to_vec();
        let unfinished = genUnfinishedPathFromORList(ds, &branches, &candidates);
        if let Some(path) = handleTopLevelANDList(ds, filters, or_offset, &candidates, unfinished) {
            ds.source.paths.push(path);
        }
    }
    Ok(())
}

/// 尚未定稿的索引访问路径：可用过滤、列覆盖标记与 OR 分支列表。
#[derive(Clone, Debug, Default)]
pub struct unfinishedAccessPath {
    pub path: Option<AccessPath>,
    pub usableFilters: Vec<Expression>,
    pub idxColHasUsableFilter: Vec<bool>,
    pub initedWithValidRange: bool,
    pub needKeepFilter: bool,
    pub orBranches: Vec<unfinishedAccessPathList>,
}
/// 与候选 AccessPath 对齐的未完成路径列表（空槽表示该候选不可用）。
pub type unfinishedAccessPathList = Vec<Option<unfinishedAccessPath>>;

/// 将 OR 列表各分支初始化为未完成路径；分支数小于 2 时返回 None。
pub fn genUnfinishedPathFromORList(
    ds: &IndexMergeDataSource,
    orList: &[Expression],
    candidates: &[AccessPath],
) -> Option<unfinishedAccessPath> {
    if orList.len() < 2 {
        return None;
    }
    let mut branches = Vec::new();
    for filter in orList {
        branches.push(initUnfinishedPathsFromExpr(ds, candidates, filter)?);
    }
    Some(unfinishedAccessPath {
        orBranches: branches,
        ..unfinishedAccessPath::default()
    })
}

/// 用单条表达式尝试初始化各候选索引上的未完成路径。
pub fn initUnfinishedPathsFromExpr(
    ds: &IndexMergeDataSource,
    candidates: &[AccessPath],
    expression: &Expression,
) -> Option<unfinishedAccessPathList> {
    let mut result = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let mut unfinished = unfinishedAccessPath {
            path: Some(candidate.clone()),
            ..unfinishedAccessPath::default()
        };
        // 普通索引：尝试生成局部路径；MV（多值）索引走 JSON 成员/包含过滤收集。
        if !isMVIndexPath(candidate) {
            let (path, keep) = generateNormalIndexPartialPath(ds, expression, candidate);
            if path.is_some() {
                unfinished.path = path;
                unfinished.initedWithValidRange = true;
                unfinished.needKeepFilter = keep;
                unfinished.usableFilters.push(expression.clone());
                result.push(Some(unfinished));
                continue;
            }
        }
        let Some(index) = &candidate.index else {
            result.push(None);
            continue;
        };
        let columns = &index.columns;
        let filters = split_cnf(expression);
        unfinished.needKeepFilter = filters.iter().any(|f| f.name.starts_with("root-only:"));
        if isMVIndexPath(candidate) {
            let (access, remaining, kind) = collectFilters4MVIndex(&filters, columns);
            if !access.is_empty() && matches!(kind, MULTI_VALUES_OR_MV_TP | SINGLE_VALUE_MV_TP) {
                unfinished.initedWithValidRange = true;
                unfinished.usableFilters = access;
                unfinished.needKeepFilter |= !remaining.is_empty();
                result.push(Some(unfinished));
                continue;
            }
        }
        unfinished.idxColHasUsableFilter = vec![false; columns.len()];
        let mut collected = vec![false; filters.len()];
        for (column_offset, column) in columns.iter().enumerate() {
            for (filter_offset, item) in filters.iter().enumerate() {
                if collected[filter_offset] {
                    continue;
                }
                let (ok, kind) = checkAccessFilter4IdxCol(item, Some(*column));
                if ok
                    && matches!(
                        kind,
                        EQ_OR_IN_NON_MV_TP | MULTI_VALUES_OR_MV_TP | SINGLE_VALUE_MV_TP
                    )
                {
                    unfinished.usableFilters.push(item.clone());
                    unfinished.idxColHasUsableFilter[column_offset] = true;
                    collected[filter_offset] = true;
                    break;
                }
            }
        }
        unfinished.needKeepFilter |= collected.contains(&false);
        if unfinished.initedWithValidRange || unfinished.idxColHasUsableFilter.contains(&true) {
            result.push(Some(unfinished));
        } else {
            result.push(None);
        }
    }
    result.iter().any(Option::is_some).then_some(result)
}

/// 把顶层 AND 中其余条件并入未完成路径，再物化为 AccessPath。
pub fn handleTopLevelANDList(
    ds: &IndexMergeDataSource,
    allConditions: &[Expression],
    orOffset: usize,
    candidates: &[AccessPath],
    mut unfinished: Option<unfinishedAccessPath>,
) -> Option<AccessPath> {
    for (offset, condition) in allConditions.iter().enumerate() {
        if offset != orOffset {
            let from_and = initUnfinishedPathsFromExpr(ds, candidates, condition);
            unfinished = mergeANDItemIntoUnfinishedIndexMergePath(unfinished, from_and);
        }
    }
    buildIntoAccessPath(ds, unfinished?, allConditions, orOffset)
}

/// 将一条 AND 条件合并进各 OR 分支的未完成路径（补全索引列可用过滤）。
pub fn mergeANDItemIntoUnfinishedIndexMergePath(
    mut index_merge_path: Option<unfinishedAccessPath>,
    path_list_from_and: Option<unfinishedAccessPathList>,
) -> Option<unfinishedAccessPath> {
    let path = index_merge_path.as_mut()?;
    if path.orBranches.is_empty() {
        return None;
    }
    let Some(from_and) = path_list_from_and else {
        return index_merge_path;
    };
    for branch in &mut path.orBranches {
        if branch.len() != from_and.len() {
            continue;
        }
        for (entry, and_entry) in branch.iter_mut().zip(&from_and) {
            let (Some(entry), Some(and_entry)) = (entry, and_entry) else {
                continue;
            };
            if and_entry.initedWithValidRange || and_entry.idxColHasUsableFilter.contains(&true) {
                entry.usableFilters.extend(and_entry.usableFilters.clone());
            }
        }
    }
    index_merge_path
}

/// 从已初始化的 OR 分支选出最优局部路径，组装最终 IndexMerge AccessPath。
pub fn buildIntoAccessPath(
    ds: &IndexMergeDataSource,
    unfinished: unfinishedAccessPath,
    allConditions: &[Expression],
    orOffset: usize,
) -> Option<AccessPath> {
    let mut alternatives: Vec<Vec<Vec<AccessPath>>> = Vec::new();
    for branch in unfinished.orBranches {
        let mut paths = Vec::new();
        for item in branch.into_iter().flatten() {
            let original = item.path?;
            if isMVIndexPath(&original) {
                let (access, remaining, kind) = collectFilters4MVIndex(
                    &item.usableFilters,
                    original.index.as_ref()?.columns.as_slice(),
                );
                if access.is_empty() {
                    continue;
                }
                let Ok(Some(mut built)) =
                    buildPartialPaths4MVIndexWithPath(ds, &original, &access, kind)
                else {
                    continue;
                };
                if item.needKeepFilter || !remaining.is_empty() {
                    for path in &mut built {
                        path.table_filters.push(allConditions[orOffset].clone());
                    }
                }
                paths.push(built);
            } else {
                let Some(mut built) = accessPathsForConds(ds, &item.usableFilters, &original)
                else {
                    continue;
                };
                if item.needKeepFilter {
                    built.table_filters.push(allConditions[orOffset].clone());
                }
                paths.push(vec![built]);
            }
        }
        if paths.is_empty() {
            return None;
        }
        alternatives.push(paths);
    }
    // 每个 OR 分支按比较器取最优局部路径，至少两条才构成 IndexMerge。
    let mut compare = cmpAlternatives(ds.source.stats.row_count.max(1.0));
    let decided: Vec<AccessPath> = alternatives
        .into_iter()
        .filter_map(|mut list| {
            list.sort_by(|left, right| compare(left, right));
            list.into_iter().next()
        })
        .flatten()
        .collect();
    if decided.len() < 2 {
        return None;
    }
    let contain_mv = decided.iter().any(isMVIndexPath);
    let distinct_indexes = decided.iter().map(index_identity).collect::<HashSet<_>>();
    if !contain_mv && distinct_indexes.len() <= 1 {
        return None;
    }
    let count = estimateCountAfterAccessForIndexMergeOR(ds, &decided);
    let mut table_filters = allConditions
        .iter()
        .enumerate()
        .filter(|(offset, _)| *offset != orOffset)
        .map(|(_, condition)| condition.clone())
        .collect::<Vec<_>>();
    if decided.iter().any(|path| {
        path.table_filters
            .iter()
            .any(|condition| condition.name == allConditions[orOffset].name)
    }) {
        table_filters.push(allConditions[orOffset].clone());
    }
    Some(AccessPath {
        partial_index_paths: decided,
        table_filters,
        count_after_access: count,
        ..AccessPath::default()
    })
}

/// 返回 Go 等价比较器：优先空/点范围，再比较候选组最大估算行数。
pub fn cmpAlternatives(
    _tableRows: f64,
) -> impl FnMut(&Vec<AccessPath>, &Vec<AccessPath>) -> Ordering {
    move |left, right| {
        all_point_or_empty(left)
            .cmp(&all_point_or_empty(right))
            .reverse()
            .then_with(|| {
                max_row_count(left)
                    .partial_cmp(&max_row_count(right))
                    .unwrap_or(Ordering::Equal)
            })
    }
}
/// 估算 IndexMerge OR 合并后的访问行数（各分支之和，不超过表行数）。
pub fn estimateCountAfterAccessForIndexMergeOR(
    ds: &IndexMergeDataSource,
    paths: &[AccessPath],
) -> f64 {
    let total = paths
        .iter()
        .map(|p| p.count_after_access.max(0.0))
        .sum::<f64>();
    if ds.source.stats.row_count > 0.0 {
        total.min(ds.source.stats.row_count)
    } else {
        total
    }
}

/// 判断一组路径是否全部为空范围、表点范围或唯一索引点范围。
fn all_point_or_empty(paths: &[AccessPath]) -> bool {
    paths.iter().all(|path| {
        path.ranges.is_empty()
            || (path
                .ranges
                .iter()
                .all(|range| range.is_point_non_nullable())
                && (path.is_table_path() || path.index.as_ref().is_some_and(|index| index.unique)))
    })
}

fn max_row_count(paths: &[AccessPath]) -> f64 {
    paths.iter().fold(0.0_f64, |maximum, path| {
        maximum.max(if path.index_filters.is_empty() {
            path.count_after_access
        } else {
            path.count_after_index
        })
    })
}

fn index_identity(path: &AccessPath) -> String {
    match &path.index {
        None => "table".into(),
        Some(index) => format!(
            "{:?}:{:?}:{}:{}:{}:{}",
            index.columns,
            index.prefix_lengths,
            index.unique,
            index.global,
            index.multi_valued,
            index.vector
        ),
    }
}
/// 按 `and:` 前缀拆分合取范式；否则返回单元素向量。
fn split_cnf(expression: &Expression) -> Vec<Expression> {
    expression
        .name
        .strip_prefix("and:")
        .map(|rest| {
            rest.split('|')
                .map(|name| Expression {
                    name: name.into(),
                    column: expression.column,
                    ..Expression::default()
                })
                .collect()
        })
        .unwrap_or_else(|| vec![expression.clone()])
}
