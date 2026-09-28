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

// 行视图 `Row`：廉价的 Chunk 指针 + 行下标，用于按列读取单元格。
//
// 对应 Go `row.go`。调用方必须保证底层 Chunk 在 `Row` 使用期间存活；
// `List`/迭代器通过稳定地址的装箱 Chunk 做到这一点。可选 `owner: Arc`
// 用于拷贝后自持有数据。`Datum` 为 TiDB 内部通用值容器。

#![allow(non_snake_case, non_upper_case_globals)]

use crate::{Chunk, mysql, types};
use std::sync::Arc;

/// RowSize is the Rust size of an empty row handle, matching Go's use of
/// `unsafe.Sizeof(Row{})` as an accounting constant.
///
/// 空行句柄的字节大小，对应 Go `unsafe.Sizeof(Row{})` 的记账常量。
pub const RowSize: i64 = std::mem::size_of::<Row>() as i64;

/// Row is the same cheap pointer-and-index view used by Go.  Callers must keep
/// the owning Chunk alive while a Row is in use; List and iterators do so by
/// retaining boxed chunks at stable addresses.
///
/// 廉价的指针+下标行视图；使用期间须保证拥有方 Chunk 存活。
#[derive(Clone)]
pub struct Row {
    /// 指向拥有方 Chunk 的原始指针（可为空表示空行）。
    pub(crate) c: *mut Chunk,
    /// 行在 Chunk 内的下标。
    pub(crate) idx: usize,
    /// 可选自持有：`CopyConstruct` 等场景下用 Arc 保活数据。
    owner: Option<Arc<Chunk>>,
}

unsafe impl Send for Row {}
unsafe impl Sync for Row {}

impl std::fmt::Debug for Row {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Row")
            .field("chunk", &self.c)
            .field("idx", &self.idx)
            .finish()
    }
}

impl PartialEq for Row {
    fn eq(&self, other: &Self) -> bool {
        self.c == other.c && self.idx == other.idx
    }
}

impl Eq for Row {}

impl Default for Row {
    fn default() -> Self {
        Self {
            c: std::ptr::null_mut(),
            idx: 0,
            owner: None,
        }
    }
}

impl Row {
    /// 从借用 Chunk 构造不拥有所有权的行视图。
    pub(crate) fn view(chunk: &Chunk, idx: usize) -> Self {
        Self {
            c: chunk as *const Chunk as *mut Chunk,
            idx,
            owner: None,
        }
    }

    /// 从拥有的 Chunk 构造行，并通过 Arc 自持有。
    pub(crate) fn from_owned(chunk: Chunk, idx: usize) -> Self {
        let owner = Arc::new(chunk);
        let c = Arc::as_ptr(&owner) as *mut Chunk;
        Self {
            c,
            idx,
            owner: Some(owner),
        }
    }

    /// 解引用底层 Chunk；空行会 panic。
    pub fn Chunk(&self) -> &Chunk {
        assert!(!self.c.is_null(), "empty Row has no Chunk");
        unsafe { &*self.c }
    }

    /// 是否为空行句柄（指针为 null）。
    pub fn IsEmpty(&self) -> bool {
        self.c.is_null()
    }

    /// 行下标。
    pub fn Idx(&self) -> usize {
        self.idx
    }

    /// 列数。
    pub fn Len(&self) -> usize {
        self.Chunk().NumCols()
    }

    /// 取指定列引用。
    fn column(&self, colIdx: usize) -> &crate::Column {
        &self.Chunk().columns[colIdx]
    }

    /// 读取 i64 单元格。
    pub fn GetInt64(&self, colIdx: usize) -> i64 {
        self.column(colIdx).GetInt64(self.idx)
    }

    /// 读取 u64 单元格。
    pub fn GetUint64(&self, colIdx: usize) -> u64 {
        self.column(colIdx).GetUint64(self.idx)
    }

    /// 读取 f32 单元格。
    pub fn GetFloat32(&self, colIdx: usize) -> f32 {
        self.column(colIdx).GetFloat32(self.idx)
    }

    /// 读取 f64 单元格。
    pub fn GetFloat64(&self, colIdx: usize) -> f64 {
        self.column(colIdx).GetFloat64(self.idx)
    }

    /// 读取字符串单元格。
    pub fn GetString(&self, colIdx: usize) -> String {
        self.column(colIdx).GetString(self.idx)
    }

    /// 读取字节单元格（拷贝）。
    pub fn GetBytes(&self, colIdx: usize) -> Vec<u8> {
        self.column(colIdx).GetBytes(self.idx).to_vec()
    }

    /// 读取 MySQL 时间类型单元格。
    pub fn GetTime(&self, colIdx: usize) -> types::Time {
        self.column(colIdx).GetTime(self.idx)
    }

    /// 读取 Duration；`fillFsp` 为小数秒精度填充参数。
    pub fn GetDuration(&self, colIdx: usize, fillFsp: i32) -> types::Duration {
        self.column(colIdx).GetDuration(self.idx, fillFsp)
    }

    /// Enum 的 (Name, Value) 便捷拆包。
    pub fn getNameValue(&self, colIdx: usize) -> (String, u64) {
        let value = self.GetEnum(colIdx);
        (value.Name, value.Value)
    }

    /// 读取 Enum。
    pub fn GetEnum(&self, colIdx: usize) -> types::Enum {
        self.column(colIdx).GetEnum(self.idx)
    }

    /// 读取 Set。
    pub fn GetSet(&self, colIdx: usize) -> types::Set {
        self.column(colIdx).GetSet(self.idx)
    }

    /// 读取 Decimal。
    pub fn GetMyDecimal(&self, colIdx: usize) -> types::MyDecimal {
        self.column(colIdx).GetDecimal(self.idx)
    }

    /// 读取 JSON。
    pub fn GetJSON(&self, colIdx: usize) -> types::BinaryJSON {
        self.column(colIdx).GetJSON(self.idx)
    }

    /// 读取向量类型。
    pub fn GetVectorFloat32(&self, colIdx: usize) -> types::VectorFloat32 {
        self.column(colIdx).GetVectorFloat32(self.idx)
    }

    /// 按字段类型把整行转为 `Datum` 向量（新缓冲）。
    pub fn GetDatumRow(&self, fields: &[types::FieldType]) -> Vec<types::Datum> {
        self.GetDatumRowWithBuffer(fields, vec![types::Datum::default(); self.Len()])
    }

    /// 复用调用方缓冲，按字段类型填充整行 `Datum`。
    pub fn GetDatumRowWithBuffer(
        &self,
        fields: &[types::FieldType],
        mut datumRow: Vec<types::Datum>,
    ) -> Vec<types::Datum> {
        for (colIdx, datum) in datumRow.iter_mut().enumerate() {
            self.DatumWithBuffer(colIdx, &fields[colIdx], datum);
        }
        datumRow
    }

    /// 单列转为新 `Datum`。
    pub fn GetDatum(&self, colIdx: usize, tp: &types::FieldType) -> types::Datum {
        let mut datum = types::Datum::default();
        self.DatumWithBuffer(colIdx, tp, &mut datum);
        datum
    }

    /// DatumWithBuffer mirrors row.go's complete MySQL type switch. In
    /// particular YEAR always remains signed and an unspecified decimal scale
    /// is replaced with the value's actual fraction length.
    ///
    /// 完整 MySQL 类型 switch：YEAR 恒为有符号；未指定 decimal scale 时用实际小数位。
    pub fn DatumWithBuffer(&self, colIdx: usize, tp: &types::FieldType, datum: &mut types::Datum) {
        if self.IsNull(colIdx) {
            datum.SetNull();
            return;
        }

        match tp.GetType() {
            mysql::TypeTiny
            | mysql::TypeShort
            | mysql::TypeInt24
            | mysql::TypeLong
            | mysql::TypeLonglong => {
                // UNSIGNED 标志决定写入 Uint64 还是 Int64。
                if mysql::HasUnsignedFlag(tp.GetFlag()) {
                    datum.SetUint64(self.GetUint64(colIdx));
                } else {
                    datum.SetInt64(self.GetInt64(colIdx));
                }
            }
            mysql::TypeYear => datum.SetInt64(self.GetInt64(colIdx)),
            mysql::TypeFloat => datum.SetFloat32(self.GetFloat32(colIdx)),
            mysql::TypeDouble => datum.SetFloat64(self.GetFloat64(colIdx)),
            mysql::TypeVarchar
            | mysql::TypeVarString
            | mysql::TypeString
            | mysql::TypeBlob
            | mysql::TypeTinyBlob
            | mysql::TypeMediumBlob
            | mysql::TypeLongBlob => {
                datum.SetString(self.GetString(colIdx), tp.GetCollate().to_owned());
            }
            mysql::TypeDate | mysql::TypeDatetime | mysql::TypeTimestamp => {
                datum.SetMysqlTime(self.GetTime(colIdx));
            }
            mysql::TypeDuration => {
                datum.SetMysqlDuration(self.GetDuration(colIdx, tp.GetDecimal() as i32));
            }
            mysql::TypeNewDecimal => {
                let decimal = self.GetMyDecimal(colIdx);
                // UnspecifiedLength 时用值本身的小数位数作为 frac。
                let fraction = if tp.GetDecimal() == types::UnspecifiedLength as isize {
                    decimal.GetDigitsFrac() as i32
                } else {
                    tp.GetDecimal() as i32
                };
                datum.SetMysqlDecimal(decimal);
                datum.SetLength(tp.GetFlen() as i32);
                datum.SetFrac(fraction);
            }
            mysql::TypeEnum => datum.SetMysqlEnum(self.GetEnum(colIdx), tp.GetCollate().to_owned()),
            mysql::TypeSet => datum.SetMysqlSet(self.GetSet(colIdx), tp.GetCollate().to_owned()),
            mysql::TypeBit => datum.SetMysqlBit(types::BinaryLiteral(self.GetBytes(colIdx))),
            mysql::TypeJSON => datum.SetMysqlJSON(self.GetJSON(colIdx)),
            mysql::TypeTiDBVectorFloat32 => datum.SetVectorFloat32(self.GetVectorFloat32(colIdx)),
            _ => {}
        }
    }

    /// 原始编码字节长度。
    pub fn GetRawLen(&self, colIdx: usize) -> usize {
        self.column(colIdx).GetRawLength(self.idx)
    }

    /// 原始编码字节（拷贝）。
    pub fn GetRaw(&self, colIdx: usize) -> Vec<u8> {
        self.column(colIdx).GetRaw(self.idx).to_vec()
    }

    /// 该列当前行是否为 NULL。
    pub fn IsNull(&self, colIdx: usize) -> bool {
        self.column(colIdx).IsNull(self.idx)
    }

    /// 深拷贝本行到新 Chunk，并返回自持有的 `Row`。
    pub fn CopyConstruct(&self) -> Row {
        let mut copied = crate::renewWithCapacity(self.Chunk(), 1, 1);
        copied.AppendRow(self.clone());
        Row::from_owned(*copied, 0)
    }

    /// 按字段类型格式化为逗号分隔调试字符串（NULL 显示为字面量）。
    pub fn ToString(&self, fieldTypes: &[types::FieldType]) -> String {
        let mut values = Vec::with_capacity(self.Len());
        for colIdx in 0..self.Len() {
            if self.IsNull(colIdx) {
                values.push("NULL".to_owned());
                continue;
            }
            let fieldType = &fieldTypes[colIdx];
            let value = match fieldType.EvalType() {
                types::ETInt => self.GetInt64(colIdx).to_string(),
                types::ETString => match fieldType.GetType() {
                    mysql::TypeEnum => self.GetEnum(colIdx).String(),
                    mysql::TypeSet => self.GetSet(colIdx).String(),
                    _ => self.GetString(colIdx),
                },
                types::ETDatetime | types::ETTimestamp => self.GetTime(colIdx).String(),
                types::ETDecimal => {
                    String::from_utf8_lossy(&self.GetMyDecimal(colIdx).ToString()).into_owned()
                }
                types::ETDuration => self
                    .GetDuration(colIdx, fieldType.GetDecimal() as i32)
                    .String(),
                types::ETJson => self.GetJSON(colIdx).String(),
                types::ETReal => match fieldType.GetType() {
                    mysql::TypeFloat => self.GetFloat32(colIdx).to_string(),
                    mysql::TypeDouble => self.GetFloat64(colIdx).to_string(),
                    _ => String::new(),
                },
                types::ETVectorFloat32 => self.GetVectorFloat32(colIdx).String(),
                _ => String::new(),
            };
            values.push(value);
        }
        values.join(", ")
    }
}
