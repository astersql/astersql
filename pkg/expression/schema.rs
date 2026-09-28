// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// Schema：表达式输出列与唯一键元数据。
//
// 对应 Go `schema.go`。Schema 描述算子输出列集合，以及主键/非空唯一键（PKOrUK）
// 与可空唯一键（NullableUK）。用于判断列归属、唯一性、模式合并与列裁剪。

use crate::*;

/// KeyInfo 对应一组主键或唯一键列；顺序与 Go 切片一致。
pub type KeyInfo = Vec<Column>;

/// KeyInfo 的扩展方法：克隆键列与调试字符串。
pub trait KeyInfoExt {
    fn CloneKey(&self) -> KeyInfo;
    fn String(&self) -> String;
}

impl KeyInfoExt for KeyInfo {
    /// 深克隆键内每一列，避免模式副本共享可变列状态。
    fn CloneKey(&self) -> KeyInfo {
        self.clone()
    }
    /// 将键列序列格式化为 `[col,...]`。
    fn String(&self) -> String {
        format!(
            "[{}]",
            self.iter()
                .map(Column::String)
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}

/// Schema 保存输出列，以及不允许 NULL 和允许 NULL 的两类唯一键集合。
pub struct Schema {
    pub Columns: Vec<Column>,
    pub PKOrUK: Vec<KeyInfo>,
    /// NullableUK 中的唯一索引允许 NULL；等价条件过滤 NULL 后仍可把它当作键使用。
    pub NullableUK: Vec<KeyInfo>,
}

impl Schema {
    /// Capture an owned representation suitable for the instance plan cache.
    pub fn ToCacheSnapshot(&self) -> Result<CachedSchema, CacheSnapshotError> {
        CachedSchema::try_from_schema(self)
    }

    /// 汇总列、非空唯一键与可空唯一键的调试字符串。
    pub fn String(&self) -> String {
        let columns = self
            .Columns
            .iter()
            .map(Column::String)
            .collect::<Vec<_>>()
            .join(",");
        let keys = self
            .PKOrUK
            .iter()
            .map(KeyInfoExt::String)
            .collect::<Vec<_>>()
            .join(",");
        let nullable = self
            .NullableUK
            .iter()
            .map(KeyInfoExt::String)
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "Column: [{}] PKOrUK: [{}] NullableUK: [{}]",
            columns, keys, nullable
        )
    }

    /// 克隆列和两类键；NullableUK 的空/非空形状在 Vec 这里统一为空 Vec 或深拷贝。
    pub fn Clone(&self) -> Schema {
        let mut schema = NewSchema(self.Columns.clone());
        schema.SetKeys(self.PKOrUK.iter().map(KeyInfoExt::CloneKey).collect());
        schema.SetUniqueKeys(self.NullableUK.iter().map(KeyInfoExt::CloneKey).collect());
        schema
    }

    /// Equal 与 Go 一样只比较列序列，不把键元数据纳入相等判断。
    pub fn Equal(&self, other: &Schema) -> bool {
        self.Columns.len() == other.Columns.len()
            && self
                .Columns
                .iter()
                .zip(&other.Columns)
                .all(|(left, right)| left.EqualColumn(right))
    }

    /// 按 UniqueID 取回模式中的列引用。
    pub fn RetrieveColumn(&self, column: &Column) -> Option<&Column> {
        self.ColumnIndex(column).map(|index| &self.Columns[index])
    }

    /// strong=true 检查非空 PK/UK，false 检查允许 NULL 的 UK；传入列可多于键列。
    pub fn IsUnique(&self, strong: bool, columns: &[Column]) -> bool {
        let keys = if strong {
            &self.PKOrUK
        } else {
            &self.NullableUK
        };
        keys.iter().any(|key| {
            key.len() <= columns.len()
                && key
                    .iter()
                    .all(|key_column| columns.iter().any(|column| key_column.EqualColumn(column)))
        })
    }

    /// 优先返回完整列；若同一 UniqueID 只找到前缀列，则回退到最后一个前缀位置。
    pub fn ColumnIndex(&self, column: &Column) -> Option<usize> {
        let mut backup = None;
        for (index, current) in self.Columns.iter().enumerate() {
            if current.UniqueID == column.UniqueID {
                backup = Some(index);
                if current.IsPrefix {
                    continue;
                }
                return Some(index);
            }
        }
        backup
    }

    /// 判断模式是否包含给定列。
    pub fn Contains(&self, column: &Column) -> bool {
        self.ColumnIndex(column).is_some()
    }
    /// 返回输出列数量。
    pub fn Len(&self) -> usize {
        self.Columns.len()
    }
    /// 向模式追加输出列。
    pub fn Append(&mut self, columns: impl IntoIterator<Item = Column>) {
        self.Columns.extend(columns);
    }
    /// 设置主键或非空唯一键集合。
    pub fn SetKeys(&mut self, keys: Vec<KeyInfo>) {
        self.PKOrUK = keys;
    }
    /// 设置允许 NULL 的唯一键集合。
    pub fn SetUniqueKeys(&mut self, keys: Vec<KeyInfo>) {
        self.NullableUK = keys;
    }

    /// 返回每个列在当前模式中的位置；任意列缺失时整体返回 None。
    pub fn ColumnsIndices(&self, columns: &[Column]) -> Option<Vec<usize>> {
        columns
            .iter()
            .map(|column| self.ColumnIndex(column))
            .collect()
    }

    /// 调用方保证 offset 合法；这里按给定顺序返回列引用，不额外改变越界语义。
    pub fn ColumnsByIndices(&self, offsets: &[usize]) -> Vec<&Column> {
        offsets
            .iter()
            .map(|&offset| &self.Columns[offset])
            .collect()
    }

    /// 过滤完全属于当前模式的列组，并同时返回原输入中的组下标。
    pub fn ExtractColGroups(&self, groups: &[Vec<Column>]) -> (Vec<Vec<usize>>, Vec<usize>) {
        let mut extracted = Vec::with_capacity(groups.len());
        let mut offsets = Vec::with_capacity(groups.len());
        for (index, group) in groups.iter().enumerate() {
            if let Some(indices) = self.ColumnsIndices(group) {
                extracted.push(indices);
                offsets.push(index);
            }
        }
        (extracted, offsets)
    }

    /// 统计 Schema 容器容量以及所有列和键列的深层内存，保留 Go 对重复列逐次计费的做法。
    pub fn MemoryUsage(&self) -> i64 {
        let mut sum = emptySchemaSize
            + self.Columns.capacity() as i64 * size::SizeOfPointer
            + (self.PKOrUK.capacity() + self.NullableUK.capacity()) as i64 * size::SizeOfSlice;
        sum += self.Columns.iter().map(Column::MemoryUsage).sum::<i64>();
        for key in self.PKOrUK.iter().chain(&self.NullableUK) {
            sum += key.capacity() as i64 * size::SizeOfPointer;
            sum += key.iter().map(Column::MemoryUsage).sum::<i64>();
        }
        sum
    }

    /// ExtraHandle 通常位于最后一列，也兼容它因额外列而处于倒数第二位的布局。
    pub fn GetExtraHandleColumn(&self) -> Option<&Column> {
        let len = self.Columns.len();
        if len > 0 && self.Columns[len - 1].ID == model::ExtraHandleID {
            return Some(&self.Columns[len - 1]);
        }
        if len > 1 && self.Columns[len - 2].ID == model::ExtraHandleID {
            return Some(&self.Columns[len - 2]);
        }
        None
    }
}

/// 检查表达式是否引用当前模式中的任意普通列；关联列和常量不计入引用。
pub fn ExprReferenceSchema(expr: &dyn Expression, schema: &Schema) -> bool {
    if let Some(column) = expr.as_any().downcast_ref::<Column>() {
        return schema.Contains(column);
    }
    if let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() {
        return function
            .GetArgs()
            .iter()
            .any(|arg| ExprReferenceSchema(arg.as_ref(), schema));
    }
    false
}

/// 检查表达式的全部普通列是否来自同一模式；常量与关联列天然满足该条件。
pub fn ExprFromSchema(expr: &dyn Expression, schema: &Schema) -> bool {
    if let Some(column) = expr.as_any().downcast_ref::<Column>() {
        return schema.Contains(column);
    }
    if let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() {
        return function
            .GetArgs()
            .iter()
            .all(|arg| ExprFromSchema(arg.as_ref(), schema));
    }
    expr.as_any().is::<CorrelatedColumn>() || expr.as_any().is::<Constant>()
}

/// 对应 unsafe.Sizeof(Schema{})，只计算空结构本体。
pub const emptySchemaSize: i64 = std::mem::size_of::<Schema>() as i64;

/// 合并左右模式时分别深克隆列；唯一键信息由后续 build_key_info 阶段重建，因此这里不合并键。
pub fn MergeSchema(left: Option<&Schema>, right: Option<&Schema>) -> Option<Schema> {
    match (left, right) {
        (None, None) => None,
        (Some(schema), None) | (None, Some(schema)) => Some(schema.Clone()),
        (Some(left), Some(right)) => {
            let mut columns = left.Clone().Columns;
            columns.extend(right.Clone().Columns);
            Some(NewSchema(columns))
        }
    }
}

/// 标记 schema 中哪些列被 usedCols 使用；生成列命中后，还会按虚拟表达式和返回类型扩散到等价列。
pub fn GetUsedList(ctx: &dyn EvalContext, used_columns: Vec<Column>, schema: &Schema) -> Vec<bool> {
    let used_schema = NewSchema(used_columns);
    let mut used = vec![false; schema.Len()];
    for (index, column) in schema.Columns.iter().enumerate() {
        if used[index] {
            continue;
        }
        used[index] = used_schema.Contains(column);
        if !used[index] {
            continue;
        }
        let Some(expr) = column
            .VirtualExpr
            .as_ref()
            .and_then(|expr| expr.as_any().downcast_ref::<ScalarFunction>())
        else {
            continue;
        };
        for (other_index, other) in schema.Columns.iter().enumerate() {
            if !used[other_index]
                && other_index != index
                && other
                    .VirtualExpr
                    .as_ref()
                    .is_some_and(|candidate| expr.Equal(ctx, candidate.as_ref()))
                && column.RetType == other.RetType
            {
                used[other_index] = true;
            }
        }
    }
    used
}

/// NewSchema 保留变参构造语义；Rust 以 Vec 接收同一列顺序。
pub fn NewSchema(columns: Vec<Column>) -> Schema {
    Schema {
        Columns: columns,
        PKOrUK: Vec::new(),
        NullableUK: Vec::new(),
    }
}
