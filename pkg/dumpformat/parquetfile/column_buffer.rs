// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// Parquet 列批量写入缓冲区。
//
// 按物理类型（Boolean/Int32/.../FixedLenByteArray）预分配对应 Vec；
// nullable 或需空值编码时额外维护 definition levels（定义级别，标识值是否存在）。
// 文件前半为 Go 机械翻译草稿，后半为可编译实现。

// Parquet 写入路径如何按列的物理类型准备批量缓冲区。
//
// columnBuffer 对应 Go 的内部结构体：每种 Parquet physical type 都有独立 Vec 缓冲。
// defLevels 仅在 nullable 或 timestamp 兜底为空值编码时使用。
// pub struct columnBuffer {
//     pub defLevels: Vec<i16>,
//     pub boolValues: Vec<bool>,
//     pub int32Values: Vec<i32>,
//     pub int64Values: Vec<i64>,
//     pub float32Values: Vec<f32>,
//     pub float64Values: Vec<f64>,
//     pub byteArrayValues: Vec<parquet::ByteArray>,
//     pub fixedLenByteArrayValues: Vec<parquet::FixedLenByteArray>,
// }
//
// impl columnBuffer {
// reset 对应 Go 方法：保留已分配容量，只把各列缓冲长度截断到 0，供下一个 row group 复用。
//     pub fn reset(&mut self) {
//         self.defLevels.clear();
//         self.boolValues.clear();
//         self.int32Values.clear();
//         self.int64Values.clear();
//         self.float32Values.clear();
//         self.float64Values.clear();
//         self.byteArrayValues.clear();
//         self.fixedLenByteArrayValues.clear();
//     }
// }
//
// newColumnBuffers 对应 Go 的批量初始化函数。
// 任一列初始化失败时补上列名上下文，保持 Go 中 fmt.Errorf("init parquet buffer for column ...") 的错误语义。
// pub fn newColumnBuffers(columns: &[column], capacity: usize) -> Result<Vec<columnBuffer>, Error> {
//     let mut buffers: Vec<columnBuffer> = Vec::with_capacity(columns.len());
//     for column in columns {
//         let buffer = newColumnBuffer(column.clone(), capacity)
//             .map_err(|err| Error::wrap(format!("init parquet buffer for column {}", column.Name), err))?;
//         buffers.push(buffer);
//     }
//     Ok(buffers)
// }
//
// newColumnBuffer 对应 Go 的单列缓冲构造逻辑。
// Rust 按 physical type 只初始化会被写入的 Vec，避免误导为所有类型都会同时填充。
// pub fn newColumnBuffer(column: column, capacity: usize) -> Result<columnBuffer, Error> {
//     let mut buffer = columnBuffer {
//         defLevels: Vec::new(),
//         boolValues: Vec::new(),
//         int32Values: Vec::new(),
//         int64Values: Vec::new(),
//         float32Values: Vec::new(),
//         float64Values: Vec::new(),
//         byteArrayValues: Vec::new(),
//         fixedLenByteArrayValues: Vec::new(),
//     };
//
//     if column.allowsNullEncoding {
// Go 使用 make([]int16, 0, capacity)，这里保留容量预分配语义。
//         buffer.defLevels = Vec::with_capacity(capacity);
//     }
//
//     match column.Physical {
//         parquet::Types::Boolean => {
//             buffer.boolValues = Vec::with_capacity(capacity);
//         }
//         parquet::Types::Int32 => {
//             buffer.int32Values = Vec::with_capacity(capacity);
//         }
//         parquet::Types::Int64 => {
//             buffer.int64Values = Vec::with_capacity(capacity);
//         }
//         parquet::Types::Float => {
//             buffer.float32Values = Vec::with_capacity(capacity);
//         }
//         parquet::Types::Double => {
//             buffer.float64Values = Vec::with_capacity(capacity);
//         }
//         parquet::Types::ByteArray => {
//             buffer.byteArrayValues = Vec::with_capacity(capacity);
//         }
//         parquet::Types::FixedLenByteArray => {
//             if column.TypeLength <= 0 {
// Go 在固定宽度字节数组宽度非法时立即返回错误，避免后续 writer panic。
//                 return Err(Error::new(format!("invalid fixed-size byte width {}", column.TypeLength)));
//             }
//             buffer.fixedLenByteArrayValues = Vec::with_capacity(capacity);
//         }
//         _ => {
//             return Err(Error::new(format!(
//                 "unsupported parquet physical type {}",
//                 column.Physical
//             )));
//         }
//     }
//     Ok(buffer)
// }
// */
use crate::column_type::{Column, PhysicalType};
use crate::{Error, Result};

#[derive(Clone, Debug, Default, PartialEq)]
/// 单列写入缓冲：defLevels + 各物理类型值向量（同一时刻通常只用一种）。
pub struct ColumnBuffer {
    /// 定义级别，用于可空列的 null 编码。
    pub definition_levels: Vec<i16>,
    /// Boolean 物理类型值。
    pub bool_values: Vec<bool>,
    /// Int32 物理类型值。
    pub int32_values: Vec<i32>,
    /// Int64 物理类型值。
    pub int64_values: Vec<i64>,
    /// Float 物理类型值。
    pub float32_values: Vec<f32>,
    /// Double 物理类型值。
    pub float64_values: Vec<f64>,
    /// BYTE_ARRAY 值。
    pub byte_array_values: Vec<Vec<u8>>,
    /// FIXED_LEN_BYTE_ARRAY 值。
    pub fixed_len_byte_array_values: Vec<Vec<u8>>,
}
impl ColumnBuffer {
    /// 清空各缓冲长度但保留容量，供下一 row group 复用。
    pub fn reset(&mut self) {
        self.definition_levels.clear();
        self.bool_values.clear();
        self.int32_values.clear();
        self.int64_values.clear();
        self.float32_values.clear();
        self.float64_values.clear();
        self.byte_array_values.clear();
        self.fixed_len_byte_array_values.clear();
    }
}
/// 按列物理类型构造单列缓冲；非法 fixed 宽度或 Int96 返回错误。
pub fn new_column_buffer(column: &Column, capacity: usize) -> Result<ColumnBuffer> {
    // FixedLenByteArray 宽度必须为正，避免后续 writer panic。
    if column.column_type.physical == PhysicalType::FixedLenByteArray
        && column.column_type.type_length <= 0
    {
        return Err(Error(format!(
            "invalid fixed-size byte width {}",
            column.column_type.type_length
        )));
    }
    let mut b = ColumnBuffer::default();
    // 可空编码时预分配 definition levels。
    if column.allows_null_encoding {
        b.definition_levels = Vec::with_capacity(capacity);
    }
    // 仅初始化实际会写入的那一类 Vec。
    match column.column_type.physical {
        PhysicalType::Boolean => b.bool_values = Vec::with_capacity(capacity),
        PhysicalType::Int32 => b.int32_values = Vec::with_capacity(capacity),
        PhysicalType::Int64 => b.int64_values = Vec::with_capacity(capacity),
        PhysicalType::Float => b.float32_values = Vec::with_capacity(capacity),
        PhysicalType::Double => b.float64_values = Vec::with_capacity(capacity),
        PhysicalType::ByteArray => b.byte_array_values = Vec::with_capacity(capacity),
        PhysicalType::FixedLenByteArray => {
            b.fixed_len_byte_array_values = Vec::with_capacity(capacity)
        }
        PhysicalType::Int96 => return Err(Error("unsupported parquet physical type Int96".into())),
    }
    Ok(b)
}
/// 批量为多列创建缓冲；失败时错误信息带上列名上下文。
pub fn new_column_buffers(columns: &[Column], capacity: usize) -> Result<Vec<ColumnBuffer>> {
    columns
        .iter()
        .map(|column| {
            new_column_buffer(column, capacity).map_err(|error| {
                Error(format!(
                    "init parquet buffer for column {}: {}",
                    column.info.name, error
                ))
            })
        })
        .collect()
}
/// Go 风格别名：转发到 `new_column_buffer`。
pub fn newColumnBuffer(c: &Column, n: usize) -> Result<ColumnBuffer> {
    new_column_buffer(c, n)
}
/// Go 风格别名：转发到 `new_column_buffers`。
pub fn newColumnBuffers(c: &[Column], n: usize) -> Result<Vec<ColumnBuffer>> {
    new_column_buffers(c, n)
}
