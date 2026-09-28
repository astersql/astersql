// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 分区表路由与裁剪辅助（对应 Go `tables/partition.go` 的核心定位逻辑）。
//
// 根据行数据定位 Hash/Key/Range/List（及 Columns 变体）分区下标，
// 并支持 LIST 分区位置集合的交并、DDL 重组期间的双写分区集合。
// 分区（Partition）把大表按规则拆成多个物理子表，便于裁剪扫描范围。

use crate::mutation_checker::Datum;
use crc32fast::Hasher;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// BTree 相关结构的默认度数（与 Go 侧常量对齐，供后续移植使用）。
pub const BTREE_DEGREE: usize = 32;

/// 分区类型：Hash/Key 按哈希取模；Range/List 按边界或显式值映射；Columns 变体按多列。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PartitionType {
    Hash,
    Key,
    Range,
    RangeColumns,
    List,
    ListColumns,
}

/// 单个分区定义：物理 ID 与分区名。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartitionDefinition {
    pub id: i64,
    pub name: String,
}

/// 运行时分区实体：物理表 ID + 定义。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Partition {
    pub physical_id: i64,
    pub definition: PartitionDefinition,
}

/// 分区定位错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PartitionError {
    NoPartition,
    NoPartitionForValue,
    InvalidColumnOffset(usize),
    InvalidDefinition(String),
}

/// 分区表达式：封装各类分区的定位（locate）实现。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PartitionExpr {
    Hash {
        column_offset: usize,
        partition_count: u64,
    },
    Key(ForKeyPruning),
    Range(ForRangePruning),
    RangeColumns(ForRangeColumnsPruning),
    List(ForListPruning),
    ListColumns(ForListColumnPruning),
}

impl PartitionExpr {
    /// 根据行数据计算目标分区下标（在 definitions 中的位置）。
    pub fn locate_partition(&self, row: &[Datum]) -> Result<usize, PartitionError> {
        match self {
            Self::Hash {
                column_offset,
                partition_count,
            } => {
                if *partition_count == 0 {
                    return Err(PartitionError::NoPartition);
                }
                let value = row
                    .get(*column_offset)
                    .ok_or(PartitionError::InvalidColumnOffset(*column_offset))?;
                // NULL 视为 0；其它类型尽量转为有符号整数再取模。
                let signed = match value {
                    Datum::Null => 0,
                    Datum::Int(value) => *value,
                    Datum::Uint(value) => *value as i64,
                    Datum::Bytes(value) => String::from_utf8_lossy(value).parse().unwrap_or(0),
                };
                Ok(signed.unsigned_abs() as usize % *partition_count as usize)
            }
            Self::Key(pruner) => pruner.locate_key_partition(row),
            Self::Range(pruner) => pruner.locate(row),
            Self::RangeColumns(pruner) => pruner.locate(row),
            Self::List(pruner) => pruner.locate(row),
            Self::ListColumns(pruner) => pruner.locate(row),
        }
    }
}

/// KEY 分区裁剪：多列 CRC32 哈希后对分区数取模。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForKeyPruning {
    pub column_offsets: Vec<usize>,
    pub partition_count: u64,
}

impl ForKeyPruning {
    /// 对分区键列做 CRC32，定位 KEY 分区下标。
    pub fn locate_key_partition(&self, row: &[Datum]) -> Result<usize, PartitionError> {
        if self.partition_count == 0 {
            return Err(PartitionError::NoPartition);
        }
        let mut hasher = Hasher::new();
        for offset in &self.column_offsets {
            hash_datum(
                row.get(*offset)
                    .ok_or(PartitionError::InvalidColumnOffset(*offset))?,
                &mut hasher,
            );
        }
        Ok(hasher.finalize() as usize % self.partition_count as usize)
    }
}

/// 将 Datum 写入 CRC32 流（NULL 写单字节 0）。
fn hash_datum(value: &Datum, hasher: &mut Hasher) {
    match value {
        Datum::Null => hasher.update(&[0]),
        Datum::Int(value) => hasher.update(&value.to_le_bytes()),
        Datum::Uint(value) => hasher.update(&value.to_le_bytes()),
        Datum::Bytes(value) => hasher.update(value),
    }
}

/// RANGE 分区上界：每个元素为该分区 `VALUES LESS THAN` 边界；`None` 表示 MAXVALUE。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForRangePruning {
    pub column_offset: usize,
    /// `None` represents MAXVALUE.
    pub upper_bounds: Vec<Option<i64>>,
}

impl ForRangePruning {
    /// 二分/partition_point 定位首个上界大于行值的分区。
    pub fn locate(&self, row: &[Datum]) -> Result<usize, PartitionError> {
        let value = row
            .get(self.column_offset)
            .ok_or(PartitionError::InvalidColumnOffset(self.column_offset))?;
        // MySQL RANGE：NULL 落入第一个分区。
        if matches!(value, Datum::Null) {
            return (!self.upper_bounds.is_empty())
                .then_some(0)
                .ok_or(PartitionError::NoPartition);
        }
        self.upper_bounds
            .partition_point(|bound| {
                bound.is_some_and(|bound| match value {
                    Datum::Int(value) => *value >= bound,
                    // Go's `ForRangePruning.Compare` compares unsigned RANGE
                    // values as uint64; narrowing here would wrap values above
                    // i64::MAX into the lowest partition.
                    Datum::Uint(value) => *value >= bound as u64,
                    Datum::Bytes(value) => {
                        String::from_utf8_lossy(value).parse().unwrap_or(0) >= bound
                    }
                    Datum::Null => unreachable!(),
                })
            })
            .checked_sub(0)
            .filter(|index| *index < self.upper_bounds.len())
            .ok_or(PartitionError::NoPartitionForValue)
    }
}

/// RANGE COLUMNS：多列上界；缺失元素视为 MAXVALUE。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForRangeColumnsPruning {
    pub column_offsets: Vec<usize>,
    /// Missing elements in a bound are MAXVALUE.
    pub upper_bounds: Vec<Vec<Option<Datum>>>,
}

impl ForRangeColumnsPruning {
    /// 找到第一个上界字典序大于行值的分区。
    pub fn locate(&self, row: &[Datum]) -> Result<usize, PartitionError> {
        let values = self
            .column_offsets
            .iter()
            .map(|offset| {
                row.get(*offset)
                    .cloned()
                    .ok_or(PartitionError::InvalidColumnOffset(*offset))
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.upper_bounds
            .iter()
            .position(|bound| compare_range_columns(&values, bound) == Ordering::Less)
            .ok_or(PartitionError::NoPartitionForValue)
    }
}

/// 多列 RANGE 比较：边界缺位（MAXVALUE）视为行值更小。
fn compare_range_columns(values: &[Datum], bound: &[Option<Datum>]) -> Ordering {
    for (index, value) in values.iter().enumerate() {
        let ordering = match bound.get(index).and_then(Option::as_ref) {
            None => Ordering::Less,
            Some(bound) => value.cmp(bound),
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}

/// LIST 分区：单列值到分区下标的映射，可带 DEFAULT 分区。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ForListPruning {
    pub column_offset: usize,
    pub value_to_partition: HashMap<Datum, usize>,
    pub default_partition: Option<usize>,
}

impl ForListPruning {
    /// 按列值查表，未命中则回落到 DEFAULT。
    pub fn locate(&self, row: &[Datum]) -> Result<usize, PartitionError> {
        let value = row
            .get(self.column_offset)
            .ok_or(PartitionError::InvalidColumnOffset(self.column_offset))?;
        self.value_to_partition
            .get(value)
            .copied()
            .or(self.default_partition)
            .ok_or(PartitionError::NoPartitionForValue)
    }
}

/// LIST COLUMNS：多列元组到分区下标的有序映射。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ForListColumnPruning {
    pub column_offsets: Vec<usize>,
    pub value_to_partition: BTreeMap<Vec<Datum>, usize>,
    pub default_partition: Option<usize>,
}

impl ForListColumnPruning {
    /// 提取多列键后查映射或 DEFAULT。
    pub fn locate(&self, row: &[Datum]) -> Result<usize, PartitionError> {
        let key = self
            .column_offsets
            .iter()
            .map(|offset| {
                row.get(*offset)
                    .cloned()
                    .ok_or(PartitionError::InvalidColumnOffset(*offset))
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.value_to_partition
            .get(&key)
            .copied()
            .or(self.default_partition)
            .ok_or(PartitionError::NoPartitionForValue)
    }
}

/// LIST 分区裁剪中的一组：同一 group_index 下的候选分区下标集合。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListPartitionGroup {
    pub group_index: usize,
    pub partition_indexes: BTreeSet<usize>,
}

impl ListPartitionGroup {
    /// 同组取交集；不同组则清空并返回 false。
    pub fn intersect(&mut self, other: &Self) -> bool {
        if self.group_index != other.group_index {
            self.partition_indexes.clear();
            return false;
        }
        self.partition_indexes = self
            .partition_indexes
            .intersection(&other.partition_indexes)
            .copied()
            .collect();
        !self.partition_indexes.is_empty()
    }

    /// 同组取并集。
    pub fn union(&mut self, other: &Self) {
        if self.group_index == other.group_index {
            self.partition_indexes
                .extend(other.partition_indexes.iter().copied());
        }
    }
}

/// 多个 ListPartitionGroup 组成的位置集合，供谓词裁剪组合。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ListPartitionLocation(pub Vec<ListPartitionGroup>);

impl ListPartitionLocation {
    /// 所有组的分区集合皆空时视为空位置。
    pub fn is_empty(&self) -> bool {
        self.0
            .iter()
            .all(|group| group.partition_indexes.is_empty())
    }

    /// 按 group_index 合并并集，并按组号排序。
    pub fn union(&mut self, other: &Self) {
        for other_group in &other.0 {
            if let Some(group) = self
                .0
                .iter_mut()
                .find(|group| group.group_index == other_group.group_index)
            {
                group.union(other_group);
            } else {
                self.0.push(other_group.clone());
            }
        }
        self.0.sort_by_key(|group| group.group_index);
    }

    /// 保留能与 other 同组相交的组；返回是否非空。
    pub fn intersect(&mut self, other: &Self) -> bool {
        self.0.retain_mut(|group| {
            other
                .0
                .iter()
                .find(|other| other.group_index == group.group_index)
                .is_some_and(|other| group.intersect(other))
        });
        !self.is_empty()
    }
}

/// 分区表：定义列表、定位表达式、当前分区、重组中分区与双写分区。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartitionedTable {
    pub definitions: Vec<PartitionDefinition>,
    pub expression: PartitionExpr,
    pub partitions: HashMap<i64, Partition>,
    /// DDL 重组过程中的新旧分区映射。
    pub reorganize_partitions: HashMap<i64, Partition>,
    /// 需双写的目标分区（重组过渡态）。
    pub double_write_partitions: HashMap<i64, Partition>,
}

impl PartitionedTable {
    /// 定位行所属主分区。
    pub fn locate_partition(&self, row: &[Datum]) -> Result<&Partition, PartitionError> {
        let index = self.expression.locate_partition(row)?;
        let definition = self
            .definitions
            .get(index)
            .ok_or(PartitionError::NoPartitionForValue)?;
        self.partitions
            .get(&definition.id)
            .ok_or(PartitionError::NoPartition)
    }

    /// 返回可写物理分区 ID：主分区 + 双写集合（去重排序）。
    pub fn writable_partition_ids(&self, row: &[Datum]) -> Result<Vec<i64>, PartitionError> {
        let primary = self.locate_partition(row)?.physical_id;
        let mut ids = vec![primary];
        ids.extend(self.double_write_partitions.keys().copied());
        ids.sort_unstable();
        ids.dedup();
        Ok(ids)
    }
}

/// 构造分区记录键：`t{partition_id}_r{handle}`。
pub fn partition_record_key(partition_id: i64, handle: i64) -> Vec<u8> {
    format!("t{partition_id}_r{handle}").into_bytes()
}
