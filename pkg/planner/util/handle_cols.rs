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

// 行句柄列（HandleCols）：从行数据构造 KV Handle 的规划侧抽象。
//
// Handle 是表行在存储层的唯一标识：整型主键为 `IntHandle`，
// 聚簇索引（common handle）则为编码后的多列键。
// 本模块提供：
// - [`HandleCols`] trait：统一从 chunk 行 / Datum 行 / 索引回表行构建 Handle；
// - [`CommonHandleCols`]：多列公共句柄；
// - [`IntHandleCols`]：单列整型主键句柄。

use std::any::Any;

use cascades_base::{Equals, Hash64, Hasher, NilFlag, NotNilFlag};
use expression::{Expression as _, StringerWithCtx};

/// 行句柄列集合：描述如何从投影/索引行抽出并编码成 `kv::Handle`。
///
/// 实现需支持哈希/相等（Cascades Memo）以及带上下文的 Explain 字符串化。
pub trait HandleCols: StringerWithCtx + Hash64 + Equals {
    /// Return owned metadata needed to rebuild a common handle for plan-cache snapshots.
    fn CacheCommonHandleMetadata(&self) -> Option<(model::TableInfo, model::IndexInfo)> {
        None
    }
    /// 从普通表行（按列 Index）构建 Handle。
    fn BuildHandle(
        &self,
        statement_context: &stmtctx::StatementContext,
        row: &chunk::Row,
    ) -> Result<Box<dyn kv::Handle>, expression::Error>;
    /// 从 Datum 切片（按列 Index）构建 Handle。
    fn BuildHandleByDatums(
        &self,
        statement_context: &stmtctx::StatementContext,
        row: &[expression::types::Datum],
    ) -> Result<Box<dyn kv::Handle>, expression::Error>;
    /// 从索引扫描回表行构建 Handle：句柄列位于行尾。
    fn BuildHandleFromIndexRow(
        &self,
        statement_context: &stmtctx::StatementContext,
        row: &chunk::Row,
    ) -> Result<Box<dyn kv::Handle>, expression::Error>;
    /// 从分区表索引行构建 PartitionHandle：末列为分区 ID，其前为句柄列。
    fn BuildPartitionHandleFromIndexRow(
        &self,
        statement_context: &stmtctx::StatementContext,
        row: &chunk::Row,
    ) -> Result<kv::PartitionHandle, expression::Error>;
    /// 按输出 Schema 解析列下标，返回新的 HandleCols。
    fn ResolveIndices(
        &self,
        schema: &expression::Schema,
    ) -> Result<Box<dyn HandleCols>, expression::Error>;
    /// 是否为整型主键句柄（相对 common handle）。
    fn IsInt(&self) -> bool;
    /// 取第 `index` 个句柄列。
    fn GetCol(&self, index: usize) -> Option<&expression::Column>;
    /// 句柄列个数。
    fn NumCols(&self) -> usize;
    /// 按句柄列字典序比较两行 Datum（使用对应 collation）。
    fn Compare(
        &self,
        left: &[expression::types::Datum],
        right: &[expression::types::Datum],
        collators: &[Box<dyn collate::Collator>],
        type_context: expression::types::Context,
    ) -> Result<i32, expression::Error>;
    /// 各句柄列的字段类型。
    fn GetFieldsTypes(&self) -> Vec<expression::types::FieldType>;
    /// 估算本对象内存占用（字节）。
    fn MemoryUsage(&self) -> i64;
    /// 深拷贝为独立的 `Box<dyn HandleCols>`。
    fn CloneHandleCols(&self) -> Box<dyn HandleCols>;
    /// 迭代所有句柄列。
    fn IterColumns(&self) -> Box<dyn Iterator<Item = &expression::Column> + '_>;
    /// 带下标迭代句柄列。
    fn IterColumns2(&self) -> Box<dyn Iterator<Item = (usize, &expression::Column)> + '_>;
}

/// 公共句柄（common handle / 聚簇索引）列集合：由主键索引列组成。
pub struct CommonHandleCols {
    /// 表元信息（用于索引值截断与编码）。
    /// 主键/公共句柄对应的索引元信息。
    /// 组成句柄的表达式列（顺序与索引列一致）。
    tblInfo: Box<model::TableInfo>,
    idxInfo: Box<model::IndexInfo>,
    columns: Vec<expression::Column>,
}

/// 克隆表/索引元信息与列向量。
impl Clone for CommonHandleCols {
    fn clone(&self) -> Self {
        Self {
            tblInfo: Box::new(self.tblInfo.Clone()),
            idxInfo: Box::new(self.idxInfo.Clone()),
            columns: self.columns.clone(),
        }
    }
}

/// Cascades 指纹：表、索引与各列参与 Hash64。
impl Hash64 for CommonHandleCols {
    fn Hash64(&self, hasher: &mut dyn Hasher) {
        hasher.HashByte(NotNilFlag);
        self.tblInfo.Hash64(hasher);
        hasher.HashByte(NotNilFlag);
        self.idxInfo.Hash64(hasher);
        hasher.HashByte(NotNilFlag);
        hasher.HashInt(self.columns.len() as isize);
        for column in &self.columns {
            column.Hash64(hasher);
        }
    }
}

/// 结构相等：表、索引与列一一 Equals。
impl Equals for CommonHandleCols {
    fn Equals(&self, other: &dyn Any) -> bool {
        let Some(other) = other.downcast_ref::<CommonHandleCols>() else {
            return false;
        };
        self.tblInfo.Equals(other.tblInfo.as_ref())
            && self.idxInfo.Equals(other.idxInfo.as_ref())
            && self.columns.len() == other.columns.len()
            && self
                .columns
                .iter()
                .zip(&other.columns)
                .all(|(left, right)| left.Equals(right))
    }
}

/// Explain：将句柄列列表格式化为 `[col, ...]`。
impl StringerWithCtx for CommonHandleCols {
    fn StringWithCtx(
        &self,
        context: Option<&dyn expression::exprctx::ParamValues>,
        _redact: &str,
    ) -> String {
        let context = context.unwrap_or(&expression::exprctx::EmptyParamValues);
        format!(
            "[{}]",
            self.columns
                .iter()
                .map(|column| column.ColumnExplainInfo(context, false))
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}

/// 公共句柄辅助方法：Datum 编码与列访问。
impl CommonHandleCols {
    /// 截断索引值后按时区编码为 CommonHandle。
    /// `TruncateIndexValues` 按列类型截断前缀索引等，保证编码与存储一致。
    fn buildHandleByDatumsBuffer(
        &self,
        statement_context: &stmtctx::StatementContext,
        mut datums: Vec<expression::types::Datum>,
    ) -> Result<Box<dyn kv::Handle>, expression::Error> {
        tablecodec::TruncateIndexValues(
            Box::new(tablecodec::model::TableInfo {
                Columns: self.tblInfo.Columns.clone(),
                Indices: self.tblInfo.Indices.clone(),
                PKIsHandle: self.tblInfo.PKIsHandle,
                IsCommonHandle: self.tblInfo.IsCommonHandle,
                CommonHandleVersion: self.tblInfo.CommonHandleVersion,
                ..Default::default()
            }),
            Box::new(self.idxInfo.Clone()),
            &mut datums,
        );
        let encoded = match codec::EncodeKey(statement_context.TimeZone(), Vec::new(), datums) {
            Ok(encoded) => encoded,
            Err(error) => {
                if let Some(error) = statement_context.HandleError(Some(error)) {
                    return Err(expression::errors::New(error.to_string()));
                }
                Vec::new()
            }
        };
        kv::NewCommonHandle(encoded)
            .map(|handle| Box::new(handle) as Box<dyn kv::Handle>)
            .map_err(|error| expression::errors::New(error.to_string()))
    }

    /// 返回组成公共句柄的列切片。
    pub fn GetColumns(&self) -> &[expression::Column] {
        &self.columns
    }
}

/// CommonHandleCols 对 [`HandleCols`] 的实现。
impl HandleCols for CommonHandleCols {
    fn CacheCommonHandleMetadata(&self) -> Option<(model::TableInfo, model::IndexInfo)> {
        Some((self.tblInfo.as_ref().Clone(), self.idxInfo.as_ref().Clone()))
    }
    fn CloneHandleCols(&self) -> Box<dyn HandleCols> {
        Box::new(self.clone())
    }

    fn IterColumns(&self) -> Box<dyn Iterator<Item = &expression::Column> + '_> {
        Box::new(self.columns.iter())
    }

    fn IterColumns2(&self) -> Box<dyn Iterator<Item = (usize, &expression::Column)> + '_> {
        Box::new(self.columns.iter().enumerate())
    }

    fn BuildHandle(
        &self,
        statement_context: &stmtctx::StatementContext,
        row: &chunk::Row,
    ) -> Result<Box<dyn kv::Handle>, expression::Error> {
        let datums = self
            .columns
            .iter()
            .map(|column| {
                row.GetDatum(
                    column.Index as usize,
                    column
                        .RetType
                        .as_ref()
                        .expect("handle column must have a type"),
                )
            })
            .collect();
        self.buildHandleByDatumsBuffer(statement_context, datums)
    }

    fn BuildHandleByDatums(
        &self,
        statement_context: &stmtctx::StatementContext,
        row: &[expression::types::Datum],
    ) -> Result<Box<dyn kv::Handle>, expression::Error> {
        let datums = self
            .columns
            .iter()
            .map(|column| row[column.Index as usize].clone())
            .collect();
        self.buildHandleByDatumsBuffer(statement_context, datums)
    }

    fn BuildHandleFromIndexRow(
        &self,
        statement_context: &stmtctx::StatementContext,
        row: &chunk::Row,
    ) -> Result<Box<dyn kv::Handle>, expression::Error> {
        // 索引回表行：句柄列紧挨行尾，从 Len-NumCols 起取。
        let start = row.Len() - self.NumCols();
        let datums = self
            .columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                row.GetDatum(
                    start + index,
                    column
                        .RetType
                        .as_ref()
                        .expect("handle column must have a type"),
                )
            })
            .collect();
        self.buildHandleByDatumsBuffer(statement_context, datums)
    }

    fn BuildPartitionHandleFromIndexRow(
        &self,
        statement_context: &stmtctx::StatementContext,
        row: &chunk::Row,
    ) -> Result<kv::PartitionHandle, expression::Error> {
        // 分区索引行：末列为物理分区 ID，其前 NumCols 列为句柄。
        let start = row.Len() - 1 - self.NumCols();
        let datums = self
            .columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                row.GetDatum(
                    start + index,
                    column
                        .RetType
                        .as_ref()
                        .expect("handle column must have a type"),
                )
            })
            .collect();
        let handle = self.buildHandleByDatumsBuffer(statement_context, datums)?;
        Ok(kv::NewPartitionHandle(row.GetInt64(row.Len() - 1), handle))
    }

    fn ResolveIndices(
        &self,
        schema: &expression::Schema,
    ) -> Result<Box<dyn HandleCols>, expression::Error> {
        let columns = self
            .columns
            .iter()
            .map(|column| column.ResolveIndices(schema))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Box::new(CommonHandleCols {
            tblInfo: Box::new(self.tblInfo.Clone()),
            idxInfo: Box::new(self.idxInfo.Clone()),
            columns,
        }))
    }

    fn IsInt(&self) -> bool {
        false
    }

    fn GetCol(&self, index: usize) -> Option<&expression::Column> {
        self.columns.get(index)
    }

    fn NumCols(&self) -> usize {
        self.columns.len()
    }

    fn Compare(
        &self,
        left: &[expression::types::Datum],
        right: &[expression::types::Datum],
        collators: &[Box<dyn collate::Collator>],
        type_context: expression::types::Context,
    ) -> Result<i32, expression::Error> {
        // 按句柄列顺序逐列比较，首个非零结果即总序。
        for (index, column) in self.columns.iter().enumerate() {
            let column_index = column.Index as usize;
            let compared = left[column_index].Compare(
                type_context.clone(),
                &right[column_index],
                collators[index].as_ref(),
            )?;
            if compared != 0 {
                return Ok(compared);
            }
        }
        Ok(0)
    }

    fn GetFieldsTypes(&self) -> Vec<expression::types::FieldType> {
        self.columns
            .iter()
            .map(|column| {
                column
                    .RetType
                    .clone()
                    .expect("handle column must have a type")
            })
            .collect()
    }

    fn MemoryUsage(&self) -> i64 {
        std::mem::size_of::<CommonHandleCols>() as i64
            + self.columns.capacity() as i64 * size::SizeOfPointer
            + self
                .columns
                .iter()
                .map(expression::Column::MemoryUsage)
                .sum::<i64>()
    }
}

/// 按索引列 Offset 从表列中对齐，构造 [`CommonHandleCols`]。
pub fn NewCommonHandleCols(
    table: model::TableInfo,
    index: model::IndexInfo,
    table_columns: &[expression::Column],
) -> CommonHandleCols {
    let columns = index
        .Columns
        .iter()
        .map(|index_column| table_columns[index_column.Offset as usize].clone())
        .collect();
    CommonHandleCols {
        tblInfo: Box::new(table),
        idxInfo: Box::new(index),
        columns,
    }
}

/// 不按 Offset 对齐，直接使用给定列向量构造公共句柄列。
pub fn NewCommonHandlesColsWithoutColsAlign(
    table: model::TableInfo,
    index: model::IndexInfo,
    columns: Vec<expression::Column>,
) -> CommonHandleCols {
    CommonHandleCols {
        tblInfo: Box::new(table),
        idxInfo: Box::new(index),
        columns,
    }
}

#[derive(Clone, Default)]
/// 整型主键句柄列：仅含一列 `_tidb_rowid` 或整型 PK。
pub struct IntHandleCols {
    /// 唯一的整型句柄列。
    col: expression::Column,
}

/// 整型句柄列的 Hash64。
impl Hash64 for IntHandleCols {
    fn Hash64(&self, hasher: &mut dyn Hasher) {
        hasher.HashByte(NotNilFlag);
        self.col.Hash64(hasher);
    }
}

/// 整型句柄列的 Equals。
impl Equals for IntHandleCols {
    fn Equals(&self, other: &dyn Any) -> bool {
        other
            .downcast_ref::<IntHandleCols>()
            .is_some_and(|other| self.col.Equals(&other.col))
    }
}

/// Explain：输出单列信息。
impl StringerWithCtx for IntHandleCols {
    fn StringWithCtx(
        &self,
        context: Option<&dyn expression::exprctx::ParamValues>,
        _redact: &str,
    ) -> String {
        self.col.ColumnExplainInfo(
            context.unwrap_or(&expression::exprctx::EmptyParamValues),
            false,
        )
    }
}

/// IntHandleCols 对 [`HandleCols`] 的实现：直接读写 i64。
impl HandleCols for IntHandleCols {
    fn CloneHandleCols(&self) -> Box<dyn HandleCols> {
        Box::new(self.clone())
    }

    fn IterColumns(&self) -> Box<dyn Iterator<Item = &expression::Column> + '_> {
        Box::new(std::iter::once(&self.col))
    }

    fn IterColumns2(&self) -> Box<dyn Iterator<Item = (usize, &expression::Column)> + '_> {
        Box::new(std::iter::once((0, &self.col)))
    }

    fn BuildHandle(
        &self,
        _statement_context: &stmtctx::StatementContext,
        row: &chunk::Row,
    ) -> Result<Box<dyn kv::Handle>, expression::Error> {
        Ok(Box::new(kv::IntHandle(
            row.GetInt64(self.col.Index as usize),
        )))
    }

    fn BuildHandleFromIndexRow(
        &self,
        _statement_context: &stmtctx::StatementContext,
        row: &chunk::Row,
    ) -> Result<Box<dyn kv::Handle>, expression::Error> {
        // 索引回表：整型句柄在行尾。
        Ok(Box::new(kv::IntHandle(row.GetInt64(row.Len() - 1))))
    }

    fn BuildPartitionHandleFromIndexRow(
        &self,
        _statement_context: &stmtctx::StatementContext,
        row: &chunk::Row,
    ) -> Result<kv::PartitionHandle, expression::Error> {
        // 分区索引行：最后为分区 ID，倒数第二为整型句柄。
        Ok(kv::NewPartitionHandle(
            row.GetInt64(row.Len() - 1),
            Box::new(kv::IntHandle(row.GetInt64(row.Len() - 2))),
        ))
    }

    fn BuildHandleByDatums(
        &self,
        _statement_context: &stmtctx::StatementContext,
        row: &[expression::types::Datum],
    ) -> Result<Box<dyn kv::Handle>, expression::Error> {
        Ok(Box::new(kv::IntHandle(
            row[self.col.Index as usize].GetInt64(),
        )))
    }

    fn ResolveIndices(
        &self,
        schema: &expression::Schema,
    ) -> Result<Box<dyn HandleCols>, expression::Error> {
        Ok(Box::new(Self {
            col: self.col.ResolveIndices(schema)?,
        }))
    }

    fn IsInt(&self) -> bool {
        true
    }

    fn GetCol(&self, index: usize) -> Option<&expression::Column> {
        (index == 0).then_some(&self.col)
    }

    fn NumCols(&self) -> usize {
        1
    }

    fn Compare(
        &self,
        left: &[expression::types::Datum],
        right: &[expression::types::Datum],
        collators: &[Box<dyn collate::Collator>],
        _type_context: expression::types::Context,
    ) -> Result<i32, expression::Error> {
        let index = self.col.Index as usize;
        left[index].Compare(
            (*expression::types::DefaultStmtNoWarningContext).clone(),
            &right[index],
            collators[index].as_ref(),
        )
    }

    fn GetFieldsTypes(&self) -> Vec<expression::types::FieldType> {
        vec![*expression::types::NewFieldType(
            mysql::r#type::TypeLonglong,
        )]
    }

    fn MemoryUsage(&self) -> i64 {
        self.col.MemoryUsage()
    }
}

/// 由单列构造装箱的整型 [`HandleCols`]。
pub fn NewIntHandleCols(column: expression::Column) -> Box<dyn HandleCols> {
    Box::new(IntHandleCols { col: column })
}

/// 若为公共句柄，从行中取出各句柄列 Datum；整型句柄则返回空向量。
pub fn GetCommonHandleDatum(
    columns: &dyn HandleCols,
    row: &chunk::Row,
) -> Vec<expression::types::Datum> {
    if columns.IsInt() {
        return Vec::new();
    }
    columns
        .IterColumns()
        .map(|column| {
            row.GetDatum(
                column.Index as usize,
                column
                    .RetType
                    .as_ref()
                    .expect("handle column must have a type"),
            )
        })
        .collect()
}
