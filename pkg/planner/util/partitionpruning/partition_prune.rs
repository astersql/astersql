// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 分区剪枝（partition pruning）：按谓词推断需扫描的分区定义下标。
//
// 支持 Hash/Key、Range/RangeColumns、List/ListColumns；
// Range 分区在 DDL 删除过程中可通过 overlapping_dropping 合并重叠分区。
// 返回空向量表示无匹配分区；[`FULL_RANGE`] 表示需扫描全部定义。
use astersql_planner_core_rule::rule_init::{Expr, PartitionInfo, PartitionKind, Value};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// 哨兵值：表示使用全部分区定义（与 Go `rule.FullRange` 对齐）。
pub const FULL_RANGE: isize = -1;

#[derive(Clone, Debug, PartialEq)]
/// 带删除重叠信息的分区表视图，供剪枝与 dropping 合并使用。
pub struct PartitionedTable {
    /// 分区元信息（类型、列、各分区定义）。
    pub partition: PartitionInfo,
    /// 删除中分区到重叠替代分区的映射：
    /// Entry absent means an ordinary partition; `None` means a dropping
    /// partition without overlap; `Some(index)` names its replacement.
    pub overlapping_dropping: BTreeMap<usize, Option<usize>>,
}

/// 查询某分区下标的重叠替代（普通分区返回自身）。
impl PartitionedTable {
    /// 返回替代下标；`None` 表示已删除且无重叠分区可替代。
    fn overlap_for(&self, index: usize) -> Option<usize> {
        self.overlapping_dropping
            .get(&index)
            .cloned()
            .unwrap_or(Some(index))
    }
}

// / Finds the indexes of partitions used by the predicates. Empty output means
/// no partition matches; `FULL_RANGE` means every definition is used.
/// 按条件与可选分区名过滤，返回分区下标或 `[FULL_RANGE]`。
/// 空输出表示无一分区匹配。
pub fn partition_pruning(
    table: &PartitionedTable,
    conditions: &[Expr],
    partition_names: &[String],
) -> Result<Vec<isize>, String> {
    let info = &table.partition;
    if info.definitions.is_empty() {
        return Err("partitioned table has no partition definitions".into());
    }
    if info.columns.is_empty() {
        return Err("partitioned table has no partition columns".into());
    }

    // 各条件取交集：AND 语义收窄分区集合。
    let mut selected = full_set(info.definitions.len());
    for condition in conditions {
        selected = selected
            .intersection(&partitions_for_expr(condition, info))
            .copied()
            .collect();
    }
    let is_list_partition = matches!(info.kind, PartitionKind::List | PartitionKind::ListColumns);
    // Go 的 List 剪枝在 dropping 映射之后按替代分区名过滤；Range 则在
    // ConvertToIntSlice 之前按原始下标过滤。
    if !is_list_partition && !partition_names.is_empty() {
        selected.retain(|index| {
            partition_names
                .iter()
                .any(|name| info.definitions[*index].name.eq_ignore_ascii_case(name))
        });
    }

    let mut selected: Vec<usize> = selected.into_iter().collect();
    if matches!(
        info.kind,
        PartitionKind::Range | PartitionKind::RangeColumns
    ) {
        selected = handle_dropping_for_range(table, partition_names, selected);
    } else if is_list_partition {
        selected = handle_dropping_for_list(table, partition_names, selected);
    }
    if selected.len() == info.definitions.len() {
        return Ok(vec![FULL_RANGE]);
    }
    Ok(selected.into_iter().map(|index| index as isize).collect())
}

/// Range 分区删除过程中：跳过无重叠的 dropping 分区，合并连续 dropping 区间并插入重叠替代。
pub fn handle_dropping_for_range(
    table: &PartitionedTable,
    partition_names: &[String],
    mut used_partitions: Vec<usize>,
) -> Vec<usize> {
    let info = &table.partition;
    if table.overlapping_dropping.is_empty() {
        return used_partitions;
    }

    let mut result = Vec::with_capacity(used_partitions.len());
    let mut position = 0;
    while position < used_partitions.len() {
        let used = used_partitions[position];
        let Some(overlap) = table.overlap_for(used) else {
            position += 1;
            continue;
        };
        if overlap == used {
            result.push(used);
            position += 1;
            continue;
        }

        // 跳过连续的 dropping 下标，直到达到重叠替代下标。
        let mut end = position + 1;
        while end < used_partitions.len() && used_partitions[end] < overlap {
            end += 1;
        }
        let overlap_already_present =
            end < used_partitions.len() && used_partitions[end] == overlap;
        let name_matches = partition_names.is_empty()
            || partition_names.iter().any(|name| {
                info.definitions
                    .get(overlap)
                    .is_some_and(|definition| definition.name.eq_ignore_ascii_case(name))
            });
        if !overlap_already_present && name_matches {
            result.push(overlap);
        }
        if end < used_partitions.len() {
            result.extend_from_slice(&used_partitions[end..]);
        }
        used_partitions = result;
        break;
    }
    used_partitions.sort_unstable();
    used_partitions.dedup();
    used_partitions
}

/// List 剪枝在定位后立即把 dropping 分区替换为有效的重叠分区。
fn handle_dropping_for_list(
    table: &PartitionedTable,
    partition_names: &[String],
    used_partitions: Vec<usize>,
) -> Vec<usize> {
    used_partitions
        .into_iter()
        .filter_map(|index| table.overlap_for(index))
        .filter(|index| {
            partition_names.is_empty()
                || table
                    .partition
                    .definitions
                    .get(*index)
                    .is_some_and(|definition| {
                        partition_names
                            .iter()
                            .any(|name| definition.name.eq_ignore_ascii_case(name))
                    })
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// 单个谓词表达式可触及的分区集合；无法识别则返回全集。
fn partitions_for_expr(expression: &Expr, info: &PartitionInfo) -> BTreeSet<usize> {
    let full = full_set(info.definitions.len());
    let Expr::Scalar { function, args, .. } = expression else {
        return full;
    };
    match function.as_str() {
        // AND：子式分区集合求交。
        "and" => args.iter().fold(full, |current, argument| {
            current
                .intersection(&partitions_for_expr(argument, info))
                .copied()
                .collect()
        }),
        // OR：子式分区集合求并。
        "or" => args.iter().fold(BTreeSet::new(), |current, argument| {
            current
                .union(&partitions_for_expr(argument, info))
                .copied()
                .collect()
        }),
        "in" if args.len() >= 2 => {
            let Expr::Column { id, .. } = &args[0] else {
                return full;
            };
            if !info.columns.contains(id) {
                return full;
            }
            args[1..]
                .iter()
                .fold(BTreeSet::new(), |mut selected, argument| {
                    if let Expr::Constant(value) = argument
                        && *value != Value::Null
                    {
                        selected.extend(partitions_for_value("eq", value, info));
                    } else if !matches!(argument, Expr::Constant(Value::Null)) {
                        selected.extend(&full);
                    }
                    selected
                })
        }
        "is_null" if args.len() == 1 => {
            let Expr::Column { id, .. } = &args[0] else {
                return full;
            };
            if info.columns.contains(id) {
                partitions_for_value("eq", &Value::Null, info)
            } else {
                full
            }
        }
        "eq" | "lt" | "le" | "gt" | "ge" if args.len() == 2 => {
            comparison_partitions(function, &args[0], &args[1], info).unwrap_or(full)
        }
        _ => full,
    }
}

/// 列与常量比较：必要时左右交换并翻转比较符。
fn comparison_partitions(
    operator: &str,
    left: &Expr,
    right: &Expr,
    info: &PartitionInfo,
) -> Option<BTreeSet<usize>> {
    match (left, right) {
        (Expr::Column { id, .. }, Expr::Constant(Value::Null))
        | (Expr::Constant(Value::Null), Expr::Column { id, .. })
            if info.columns.contains(id) =>
        {
            Some(BTreeSet::new())
        }
        (Expr::Column { id, .. }, Expr::Constant(value)) if info.columns.contains(id) => {
            Some(partitions_for_value(operator, value, info))
        }
        (Expr::Constant(value), Expr::Column { id, .. }) if info.columns.contains(id) => {
            let reversed = match operator {
                "lt" => "gt",
                "le" => "ge",
                "gt" => "lt",
                "ge" => "le",
                other => other,
            };
            Some(partitions_for_value(reversed, value, info))
        }
        _ => None,
    }
}

/// 按分区类型将「列 op 常量」映射到分区下标集合。
fn partitions_for_value(operator: &str, value: &Value, info: &PartitionInfo) -> BTreeSet<usize> {
    match info.kind {
        PartitionKind::Hash | PartitionKind::Key => hash_partitions(operator, value, info),
        PartitionKind::Range | PartitionKind::RangeColumns => {
            range_partitions(operator, value, info)
        }
        PartitionKind::List | PartitionKind::ListColumns => list_partitions(operator, value, info),
    }
}

/// Hash/Key：仅等值可定唯一分区，否则全集。
fn hash_partitions(operator: &str, value: &Value, info: &PartitionInfo) -> BTreeSet<usize> {
    if operator != "eq" {
        return full_set(info.definitions.len());
    }
    // 将常量哈希到 [0, n) 分区下标；NULL 映射到 0。
    let hash = match value {
        Value::Null => 0,
        Value::Bool(value) => u64::from(*value),
        Value::Int(value) => value.unsigned_abs(),
        Value::UInt(value) => *value,
        Value::Float(value) => value.to_bits(),
        Value::Text(value) => value.bytes().fold(1469598103934665603_u64, |state, byte| {
            (state ^ u64::from(byte)).wrapping_mul(1099511628211)
        }),
    };
    BTreeSet::from([(hash % info.definitions.len() as u64) as usize])
}

/// Range：按 less_than 定位分区，再按比较符取前缀/后缀/单点。
fn range_partitions(operator: &str, value: &Value, info: &PartitionInfo) -> BTreeSet<usize> {
    if *value == Value::Null {
        return BTreeSet::from([0]);
    }
    let count = info.definitions.len();
    // 找到第一个 less_than 大于 value 的分区，作为等值落点。
    let position = info.definitions.iter().position(|definition| {
        definition.less_than.is_empty()
            || compare_value(value, &definition.less_than[0]) == Some(Ordering::Less)
    });
    let Some(position) = position else {
        return match operator {
            "lt" | "le" => full_set(count),
            "eq" | "gt" | "ge" => BTreeSet::new(),
            _ => full_set(count),
        };
    };
    let value_is_upper_bound = position
        .checked_sub(1)
        .and_then(|previous| info.definitions.get(previous))
        .and_then(|definition| definition.less_than.first())
        .and_then(|bound| compare_value(value, bound))
        == Some(Ordering::Equal);
    match operator {
        "eq" => BTreeSet::from([position]),
        "lt" if value_is_upper_bound => (0..position).collect(),
        "lt" | "le" => (0..=position).collect(),
        "gt" | "ge" => (position..count).collect(),
        _ => full_set(count),
    }
}

/// List：等值匹配 in_values；非等值返回全集。
fn list_partitions(operator: &str, value: &Value, info: &PartitionInfo) -> BTreeSet<usize> {
    if operator != "eq" {
        return full_set(info.definitions.len());
    }
    info.definitions
        .iter()
        .enumerate()
        .filter_map(|(index, definition)| {
            definition
                .in_values
                .iter()
                .any(|tuple| tuple.first() == Some(value))
                .then_some(index)
        })
        .collect()
}

/// `{0..count}` 全集。
fn full_set(count: usize) -> BTreeSet<usize> {
    (0..count).collect()
}

/// 同型 Value 比较；跨 Int/UInt 经 i128；无法比较返回 None。
fn compare_value(left: &Value, right: &Value) -> Option<Ordering> {
    match (left, right) {
        (Value::Null, Value::Null) => Some(Ordering::Equal),
        (Value::Null, _) => Some(Ordering::Less),
        (_, Value::Null) => Some(Ordering::Greater),
        (Value::Bool(left), Value::Bool(right)) => Some(left.cmp(right)),
        (Value::Int(left), Value::Int(right)) => Some(left.cmp(right)),
        (Value::UInt(left), Value::UInt(right)) => Some(left.cmp(right)),
        (Value::Int(left), Value::UInt(right)) => {
            i128::from(*left).partial_cmp(&i128::from(*right))
        }
        (Value::UInt(left), Value::Int(right)) => {
            i128::from(*left).partial_cmp(&i128::from(*right))
        }
        (Value::Float(left), Value::Float(right)) => left.partial_cmp(right),
        (Value::Text(left), Value::Text(right)) => Some(left.cmp(right)),
        _ => None,
    }
}
