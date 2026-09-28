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

//! 索引顾问的代价驱动候选枚举算法。
//!
//! 算法从可索引列对应的单列索引开始，按宽度逐轮扩展候选，并结合优化器估算的
//! 工作负载代价选择索引组合；最后通过有限次贪心补充与前缀消除得到推荐结果。

use crate::model::{Column, Index, IndexSetCost, Query};
use crate::optimizer::Optimizer;
use crate::options::AdvisorOptions;
use crate::utils::{collect_indexable_columns, evaluate_index_set_cost};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

/// 为给定查询集合选择能够最大幅度降低加权工作负载代价的索引。
///
/// 搜索同时受最大索引数量、最大索引宽度和超时时间约束。
pub fn advise_indexes(
    queries: &BTreeSet<Query>,
    indexable_columns: &BTreeSet<Column>,
    optimizer: &dyn Optimizer,
    options: &AdvisorOptions,
) -> Result<BTreeSet<Index>, String> {
    if options.max_num_indexes == 0 {
        return Ok(BTreeSet::new());
    }
    if options.max_index_width == 0 {
        return Err("max index width must be positive".to_string());
    }

    let started = Instant::now();
    // 从单列索引开始，并且只扩展上一宽度选出的最优集合。这是 Go auto-admin 算法的
    // 关键搜索边界；若一次性生成全部排列，在索引数量上限较小时会改变推荐结果。
    let mut potential = usable_candidates(
        &single_column_indexes(indexable_columns)?,
        &BTreeSet::new(),
        false,
        optimizer,
        started,
        options,
    )?;
    let mut selected = BTreeSet::new();
    for width in 1..=options.max_index_width {
        check_timeout(started, options)?;
        let candidates = select_index_candidates(queries, &potential, optimizer, started, options)?;
        if candidates.is_empty() {
            break;
        }
        selected = choose_best_indexes(queries, &candidates, options.max_num_indexes, optimizer)?;
        if width < options.max_index_width {
            potential = selected.clone();
            potential.extend(extend_indexes(indexable_columns, &selected, width)?);
        } else {
            potential = candidates;
        }
    }

    // 模拟 Go 实现经过启发式过滤与裁剪后的收尾阶段：最多再从剩余候选中贪心补充三轮。
    for _ in 0..3 {
        if selected.len() >= options.max_num_indexes {
            break;
        }
        check_timeout(started, options)?;
        let current_cost = evaluate_index_set_cost(queries, optimizer, &selected)?;
        let mut best: Option<(Index, IndexSetCost)> = None;
        for candidate in &potential {
            if selected.contains(candidate) {
                continue;
            }
            let mut trial = selected.clone();
            trial.insert(candidate.clone());
            let cost = evaluate_index_set_cost(queries, optimizer, &trial)?;
            if !cost.less(&current_cost)
                || best
                    .as_ref()
                    .is_some_and(|(_, best_cost)| !cost.less(best_cost))
            {
                continue;
            }
            best = Some((candidate.clone(), cost));
        }
        let Some((candidate, _)) = best else {
            break;
        };
        selected.insert(candidate.clone());
        potential.retain(|other| {
            other != &candidate
                && !candidate.prefix_contains(other)
                && !other.prefix_contains(&candidate)
        });
    }
    Ok(cut_down(selected))
}

/// 与 Go `selectIndexCandidates` 一致：为每条查询独立保留最多三个最佳单索引，
/// 再将各查询的结果合并为本轮组合搜索的候选集合。
fn select_index_candidates(
    queries: &BTreeSet<Query>,
    potential: &BTreeSet<Index>,
    optimizer: &dyn Optimizer,
    started: Instant,
    options: &AdvisorOptions,
) -> Result<BTreeSet<Index>, String> {
    let mut candidates = BTreeSet::new();
    for query in queries {
        check_timeout(started, options)?;
        let referenced = collect_indexable_columns(query, optimizer)?;
        let mut indexes =
            usable_candidates(potential, &referenced, true, optimizer, started, options)?;
        let query_set = BTreeSet::from([query.clone()]);
        let mut best_for_query = BTreeSet::new();
        for _ in 0..3 {
            let best = choose_best_indexes(&query_set, &indexes, 1, optimizer)?;
            let Some(index) = best.iter().next().cloned() else {
                break;
            };
            indexes.remove(&index);
            best_for_query.insert(index);
        }
        candidates.extend(best_for_query);
    }
    Ok(candidates)
}

fn single_column_indexes(columns: &BTreeSet<Column>) -> Result<BTreeSet<Index>, String> {
    columns
        .iter()
        .map(|column| {
            Index::with_columns(&format!("idx_{}", column.column_name), vec![column.clone()])
        })
        .collect()
}

/// 过滤与查询无关或已被现有索引前缀覆盖的候选，并在遍历期间检查超时。
#[allow(dead_code)]
fn usable_candidates(
    candidates: &BTreeSet<Index>,
    referenced: &BTreeSet<Column>,
    filter_referenced: bool,
    optimizer: &dyn Optimizer,
    started: Instant,
    options: &AdvisorOptions,
) -> Result<BTreeSet<Index>, String> {
    let mut result = BTreeSet::new();
    for candidate in candidates {
        check_timeout(started, options)?;
        if (filter_referenced
            && !candidate
                .columns
                .first()
                .is_some_and(|column| referenced.contains(column)))
            || optimizer.prefix_contain_index(&candidate)?
        {
            continue;
        }
        result.insert(candidate.clone());
    }
    Ok(result)
}

/// 先枚举空集、单索引及小规模候选的双索引组合，再贪心扩展到数量上限。
fn choose_best_indexes(
    queries: &BTreeSet<Query>,
    candidates: &BTreeSet<Index>,
    max_indexes: usize,
    optimizer: &dyn Optimizer,
) -> Result<BTreeSet<Index>, String> {
    let mut combinations = vec![BTreeSet::new()];
    for candidate in candidates {
        combinations.push(BTreeSet::from([candidate.clone()]));
    }
    // 候选规模较小时沿用 Go 实现先枚举索引对，再继续贪心扩展。成对枚举可以保留
    // `a = ? AND b = ?` 等谓词中两个索引共同使用时才体现的收益。
    if candidates.len() <= 50 && max_indexes >= 2 {
        let list = candidates.iter().collect::<Vec<_>>();
        for left in 0..list.len() {
            for right in left + 1..list.len() {
                combinations.push(BTreeSet::from([list[left].clone(), list[right].clone()]));
            }
        }
    }
    let mut best = BTreeSet::new();
    let mut best_cost = evaluate_index_set_cost(queries, optimizer, &best)?;
    for combination in combinations {
        if combination.len() > max_indexes {
            continue;
        }
        let cost = evaluate_index_set_cost(queries, optimizer, &combination)?;
        if cost.less(&best_cost) {
            best = combination;
            best_cost = cost;
        }
    }
    while best.len() < max_indexes {
        let mut next = None;
        for candidate in candidates {
            if best.contains(candidate) {
                continue;
            }
            let mut trial = best.clone();
            trial.insert(candidate.clone());
            let cost = evaluate_index_set_cost(queries, optimizer, &trial)?;
            if cost.less(&best_cost)
                && next
                    .as_ref()
                    .map_or(true, |(_, current)| cost.less(current))
            {
                next = Some((candidate.clone(), cost));
            }
        }
        let Some((candidate, cost)) = next else { break };
        best.insert(candidate);
        best_cost = cost;
    }
    Ok(best)
}

/// 将已选索引追加同表且尚未使用的列，生成下一宽度的有序候选。
fn extend_indexes(
    columns: &BTreeSet<Column>,
    indexes: &BTreeSet<Index>,
    current_width: usize,
) -> Result<BTreeSet<Index>, String> {
    let mut result = BTreeSet::new();
    for index in indexes {
        for column in columns {
            if column.schema_name == index.schema_name
                && column.table_name == index.table_name
                && index.columns.len() == current_width
                && !index.columns.contains(column)
            {
                let mut next = index.columns.clone();
                next.push(column.clone());
                let name = format!(
                    "idx_{}",
                    next.iter()
                        .map(|column| column.column_name.as_str())
                        .collect::<Vec<_>>()
                        .join("_")
                );
                result.insert(Index::with_columns(&name, next)?);
            }
        }
    }
    Ok(result)
}

/// 为每张表生成宽度从 1 到 `max_width` 的全部有序索引列组合。
///
/// 这里对应 Go 实现将每个已选索引与同表剩余列逐一扩展的语义，刻意不采用滑动窗口搜索。
pub fn create_multi_column_indexes(
    columns: &BTreeSet<Column>,
    max_width: usize,
) -> Result<BTreeSet<Index>, String> {
    if max_width == 0 {
        return Err("max index width must be positive".to_string());
    }
    let mut by_table: BTreeMap<(String, String), Vec<Column>> = BTreeMap::new();
    for column in columns {
        by_table
            .entry((column.schema_name.clone(), column.table_name.clone()))
            .or_default()
            .push(column.clone());
    }

    let mut result = BTreeSet::new();
    for (_, mut table_columns) in by_table {
        table_columns.sort();
        append_permutations(&table_columns, Vec::new(), max_width, &mut result)?;
    }
    Ok(result)
}

fn append_permutations(
    columns: &[Column],
    prefix: Vec<Column>,
    max_width: usize,
    result: &mut BTreeSet<Index>,
) -> Result<(), String> {
    if !prefix.is_empty() {
        let name = format!(
            "idx_{}",
            prefix
                .iter()
                .map(|column| column.column_name.as_str())
                .collect::<Vec<_>>()
                .join("_")
        );
        result.insert(Index::with_columns(&name, prefix.clone())?);
    }
    if prefix.len() == max_width {
        return Ok(());
    }
    for column in columns {
        if prefix.contains(column) {
            continue;
        }
        let mut next = prefix.clone();
        next.push(column.clone());
        append_permutations(columns, next, max_width, result)?;
    }
    Ok(())
}

/// 删除集合中被同表更长索引以前缀方式覆盖的索引。
pub fn cut_down(indexes: BTreeSet<Index>) -> BTreeSet<Index> {
    indexes
        .iter()
        .filter(|index| {
            !indexes
                .iter()
                .any(|other| other != *index && other.prefix_contains(index))
        })
        .cloned()
        .collect()
}

/// 在各搜索阶段检查整个推荐过程是否超过配置的时间预算。
fn check_timeout(started: Instant, options: &AdvisorOptions) -> Result<(), String> {
    if started.elapsed() > options.timeout {
        Err(format!("index advisor timeout after {:?}", options.timeout))
    } else {
        Ok(())
    }
}
