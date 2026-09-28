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

// 分区表达式与裁剪元数据（对应 Go `tables/partition_expr.go`）。
//
// 供优化器分区裁剪（partition pruning）与执行期行路由共用：保存 RANGE/KEY/LIST
// 的上界表达式、列偏移及 LIST 位置集合辅助结构。依赖 `expression` crate，
// 由 `expression-runtime` feature 启用。

use std::collections::BTreeMap;
use std::sync::Arc;

use expression::{Column, ExprBox};

/// 分区规划与行路由共享的表达式及裁剪元数据。
/// 可选字段对应 Go 中可为 nil 的接口与嵌入指针。
/// The expression and pruning metadata shared by partition planning and row routing.
/// Optional fields preserve nil Go interfaces and embedded pointers.
#[derive(Clone, Default)]
pub struct PartitionExpr {
    /// Upper bounds `(x < yN)` used while locating a range partition.
    /// RANGE 定位用的上界表达式 `(x < yN)`。
    pub UpperBounds: Vec<ExprBox>,
    /// The original partition expression AST used by point get.
    /// Point Get 使用的原始分区表达式 AST。
    pub OrigExpr: Option<expression::ast::ExprNode>,
    /// Hash partition expression.
    /// HASH 分区表达式。
    pub Expr: Option<ExprBox>,
    /// KEY 分区裁剪状态。
    pub ForKeyPruning: Option<ForKeyPruning>,
    /// RANGE 整数上界裁剪状态。
    pub ForRangePruning: Option<ForRangePruning>,
    /// RANGE COLUMNS 多列上界裁剪状态。
    pub ForRangeColumnsPruning: Option<ForRangeColumnsPruning>,
    /// Offsets of partition columns in the table schema.
    /// 分区列在表 schema 中的偏移。
    pub ColumnOffset: Vec<usize>,
    /// LIST 分区裁剪状态。
    pub ForListPruning: Option<ForListPruning>,
}

impl PartitionExpr {
    /// 返回 KEY 分区列，并把列 Index 改写为分区键行内下标（与 Go 指针原地修改一致）。
    /// Returns key-partition columns with indices rewritten for the partition-key
    /// row. The source columns are updated too, preserving Go's pointer mutation.
    pub fn GetPartColumnsForKeyPartition(
        &self,
        columns: &mut [Column],
    ) -> (Vec<Column>, Vec<isize>) {
        let mut part_columns = Vec::with_capacity(self.ColumnOffset.len());
        let mut column_lengths = Vec::with_capacity(self.ColumnOffset.len());

        for (index, &offset) in self.ColumnOffset.iter().enumerate() {
            // 改写 Index，使后续求值按分区键行布局寻址。
            columns[offset].Index = index as isize;
            column_lengths.push(
                columns[offset]
                    .RetType
                    .as_ref()
                    .expect("partition column must have a return type")
                    .GetFlen(),
            );
            part_columns.push(columns[offset].clone());
        }
        (part_columns, column_lengths)
    }
}

/// RANGE COLUMNS 上界；`None` 即 MAXVALUE（对应 Go 中 nil 表达式）。
/// Range-columns bounds. `None` is MAXVALUE, exactly as a nil Go expression.
#[derive(Clone, Default)]
pub struct ForRangeColumnsPruning {
    pub LessThan: Vec<Vec<Option<ExprBox>>>,
}

/// KEY 分区列集合。
#[derive(Clone, Default)]
pub struct ForKeyPruning {
    pub KeyPartCols: Vec<Column>,
}

impl ForKeyPruning {
    /// 用与 Go `crc32.NewIEEE` 相同的字节流定位 KEY 分区。
    /// Locates a key partition using Go's `crc32.NewIEEE` byte stream.
    pub fn LocateKeyPartition(
        &self,
        num_parts: u64,
        row: &[expression::types::Datum],
    ) -> Result<usize, expression::errors::Error> {
        let mut hasher = crc32fast::Hasher::new();
        for column in &self.KeyPartCols {
            let index = usize::try_from(column.Index)
                .expect("key partition column index must be non-negative");
            let value = &row[index];
            if value.Kind() == expression::types::KindNull {
                hasher.update(&[0]);
            } else {
                hasher.update(&value.ToHashKey()?);
            }
        }

        // Go converts the partition count to uint32 before taking the remainder.
        // Go 先把分区数转成 uint32 再取余。
        Ok((hasher.finalize() % num_parts as u32) as usize)
    }
}

/// RANGE 整数上界：`LessThan`、是否含 MAXVALUE、是否无符号比较。
#[derive(Clone, Default)]
pub struct ForRangePruning {
    pub LessThan: Vec<i64>,
    pub MaxValue: bool,
    pub Unsigned: bool,
}

impl ForRangePruning {
    /// 分区定位二分查找用的比较：返回 -1/0/1。
    /// Comparison used by partition-location binary search.
    pub fn Compare(&self, index: usize, value: i64, unsigned: bool) -> i32 {
        // 最后一个边界且为 MAXVALUE 时恒视为更大。
        if index == self.LessThan.len() - 1 && self.MaxValue {
            return 1;
        }
        let ordering = if unsigned {
            (self.LessThan[index] as u64).cmp(&(value as u64))
        } else {
            self.LessThan[index].cmp(&value)
        };
        match ordering {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }
    }
}

/// 与运行时解耦的 LIST 裁剪状态；延迟元数据构建待 table-model 接通后再补。
/// Runtime-independent LIST pruning state. Delayed metadata construction stays
/// outside this type until the table-model/build-context crates are connected.
#[derive(Clone, Default)]
pub struct ForListPruning {
    pub LocateExpr: Option<ExprBox>,
    pub PruneExpr: Option<ExprBox>,
    pub PruneExprCols: Vec<Column>,
    /// 编码后的值 → 分区下标。
    pub ValueToPartitionIdx: Arc<BTreeMap<u64, usize>>,
    /// NULL 值落入的分区；负值表示无专用 NULL 分区。
    pub NullPartitionIdx: isize,
    pub DefaultPartitionIdx: isize,
    pub ColPrunes: Vec<ForListColumnPruning>,
}

impl ForListPruning {
    /// 按整型值（或 NULL）定位 LIST 分区下标。
    pub fn LocatePartition(
        &self,
        context: &dyn expression::exprctx::EvalContext,
        value: i64,
        is_null: bool,
    ) -> isize {
        if is_null {
            return if self.NullPartitionIdx >= 0 {
                self.NullPartitionIdx
            } else {
                self.DefaultPartitionIdx
            };
        }

        let prune_expression = self
            .PruneExpr
            .as_ref()
            .expect("LIST pruning expression must be initialized");
        // 无符号列用原值作 key；有符号则 EncodeIntToCmpUint 保持比较序。
        let unsigned =
            expression::mysql::HasUnsignedFlag(prune_expression.GetType(context).GetFlag());
        let key = if unsigned {
            value as u64
        } else {
            expression::codec::EncodeIntToCmpUint(value)
        };
        self.ValueToPartitionIdx
            .get(&key)
            .map_or(self.DefaultPartitionIdx, |index| *index as isize)
    }

    /// 返回 DEFAULT 分区下标。
    pub fn GetDefaultIdx(&self) -> isize {
        self.DefaultPartitionIdx
    }
}

/// LIST COLUMNS 单列裁剪：值映射与排序映射到 ListPartitionLocation。
#[derive(Clone, Default)]
pub struct ForListColumnPruning {
    pub ExprCol: Option<Column>,
    pub ValueType: Option<expression::types::FieldType>,
    pub ValueMap: Arc<BTreeMap<String, ListPartitionLocation>>,
    pub Sorted: Arc<BTreeMap<String, ListPartitionLocation>>,
    pub DefaultPartID: i64,
}

impl ForListColumnPruning {
    /// `DefaultPartID > 0` 表示存在 DEFAULT 分区；零值表示尚未初始化。
    pub fn HasDefault(&self) -> bool {
        self.DefaultPartID > 0
    }
}

/// LIST COLUMNS 位置中单个分区的组下标列表。
/// Group indices for one partition in a LIST COLUMNS location.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ListPartitionGroup {
    pub PartIdx: usize,
    pub GroupIdxs: Vec<usize>,
}

impl ListPartitionGroup {
    /// 同分区取组下标交集；不同分区返回 false。
    fn intersect(&mut self, other: &ListPartitionGroup) -> bool {
        if self.PartIdx != other.PartIdx {
            return false;
        }
        self.GroupIdxs = other
            .GroupIdxs
            .iter()
            .copied()
            .filter(|group_index| self.GroupIdxs.contains(group_index))
            .collect();
        !self.GroupIdxs.is_empty()
    }

    /// 同分区追加组下标（Go 故意不去重）。
    fn union(&mut self, other: &ListPartitionGroup) {
        if self.PartIdx == other.PartIdx {
            // Go intentionally appends without de-duplicating group indices.
            self.GroupIdxs.extend_from_slice(&other.GroupIdxs);
        }
    }
}

/// 多个 ListPartitionGroup 组成的 LIST COLUMNS 位置。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ListPartitionLocation(pub Vec<ListPartitionGroup>);

impl ListPartitionLocation {
    /// 所有组的 GroupIdxs 皆空时为空。
    pub fn IsEmpty(&self) -> bool {
        self.0.iter().all(|group| group.GroupIdxs.is_empty())
    }

    /// 按分区下标查找组在向量中的位置。
    fn find_by_partition_index(&self, partition_index: usize) -> Option<usize> {
        self.0
            .iter()
            .position(|group| group.PartIdx == partition_index)
    }
}

/// 累积构造 ListPartitionLocation 的辅助器（首次 Intersect 时惰性初始化）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ListPartitionLocationHelper {
    initialized: bool,
    location: ListPartitionLocation,
}

/// 构造空的 ListPartitionLocationHelper。
pub fn NewListPartitionLocationHelper() -> ListPartitionLocationHelper {
    ListPartitionLocationHelper::default()
}

impl ListPartitionLocationHelper {
    /// 当前累积的位置。
    pub fn GetLocation(&self) -> &ListPartitionLocation {
        &self.location
    }

    /// 并入单个分区组。
    pub fn UnionPartitionGroup(&mut self, group: &ListPartitionGroup) {
        match self.location.find_by_partition_index(group.PartIdx) {
            Some(index) => self.location.0[index].union(group),
            None => self.location.0.push(group.clone()),
        }
    }

    /// 并入整个位置。
    pub fn Union(&mut self, location: &ListPartitionLocation) {
        for group in &location.0 {
            self.UnionPartitionGroup(group);
        }
    }

    /// 与给定位置求交；首次调用时直接克隆对方作为初值。
    pub fn Intersect(&mut self, location: &ListPartitionLocation) -> bool {
        if !self.initialized {
            self.initialized = true;
            self.location = location.clone();
            return true;
        }

        let current = &self.location;
        let mut remaining = Vec::with_capacity(location.0.len());
        for other_group in &location.0 {
            let Some(index) = current.find_by_partition_index(other_group.PartIdx) else {
                continue;
            };
            let mut group = current.0[index].clone();
            if group.intersect(other_group) {
                remaining.push(group);
            }
        }
        self.location = ListPartitionLocation(remaining);
        !self.location.0.is_empty()
    }
}
