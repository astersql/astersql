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

// 文本协议单列值格式化：按 MySQL 列类型把 chunk 行中的 Datum 转为客户端可见字节。
//
// 对应 server 侧 DumpTextRow 内部对单个单元格的序列化（不含 length-encoded 外框）。
// 数值走 strconv 追加；字符串/Blob/Enum 等经 [`ResultEncoder`] 做字符集转换；
// 不支持的类型（如 Geometry）返回 [`ErrInvalidType`]。

use crate::{ResultEncoder, chunk, mysql, types};

/// Error returned when a MySQL column type has no text-protocol serializer.
/// 列类型没有对应的文本协议序列化器时返回的错误。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidTypeError;

impl std::fmt::Display for InvalidTypeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("invalid column type for text serialization")
    }
}

impl std::error::Error for InvalidTypeError {}

/// 与 Go 侧哨兵错误对应的常量实例。
pub const ErrInvalidType: InvalidTypeError = InvalidTypeError;

/// Per-column attributes required by the text formatter.
/// 文本格式化所需的列属性：表名、collation、标志位、小数位与类型码。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ColumnInfo {
    /// 所属表名；非空时浮点精度不覆盖默认 strconv 行为。
    pub Table: String,
    /// 列 collation / charset ID，用于字符串类编码。
    pub Charset: u16,
    /// 列标志（如 UnsignedFlag）。
    pub Flag: u16,
    /// 小数位数；Duration 与浮点路径会用到。
    pub Decimal: u8,
    /// MySQL 列类型码。
    pub Type: u8,
}

/// 借用 encoder scratch 渲染后回收，减少数值格式化分配。
fn format_with_scratch(
    encoder: &mut ResultEncoder,
    render: impl FnOnce(Vec<u8>) -> Vec<u8>,
) -> Vec<u8> {
    let (scratch, reusable) = encoder.take_scratch();
    let output = render(scratch);
    encoder.recycle_scratch(&output, reusable);
    output
}

/// Returns the unframed text representation of one row value.
/// 返回单列值的无外框文本表示（不含 length-encoded 长度前缀）。
pub fn FormatValueText(
    row: &chunk::Row,
    idx: usize,
    col: &ColumnInfo,
    enc: &mut ResultEncoder,
) -> Result<Vec<u8>, InvalidTypeError> {
    match col.Type {
        // 小整数统一按有符号十进制输出。
        mysql::TypeTiny | mysql::TypeShort | mysql::TypeInt24 | mysql::TypeLong => {
            Ok(format_with_scratch(enc, |scratch| {
                goish::strconv::AppendInt(scratch, row.GetInt64(idx), 10)
            }))
        }
        mysql::TypeYear => {
            let year = row.GetInt64(idx);
            // MySQL YEAR 零值显示为 "0000" 而非 "0"。
            if year == 0 {
                Ok(format_with_scratch(enc, |mut scratch| {
                    scratch.extend_from_slice(b"0000");
                    scratch
                }))
            } else {
                Ok(format_with_scratch(enc, |scratch| {
                    goish::strconv::AppendInt(scratch, year, 10)
                }))
            }
        }
        mysql::TypeLonglong => {
            // UnsignedFlag 决定走 AppendUint 还是 AppendInt。
            if mysql::HasUnsignedFlag(usize::from(col.Flag)) {
                Ok(format_with_scratch(enc, |scratch| {
                    goish::strconv::AppendUint(scratch, row.GetUint64(idx), 10)
                }))
            } else {
                Ok(format_with_scratch(enc, |scratch| {
                    goish::strconv::AppendInt(scratch, row.GetInt64(idx), 10)
                }))
            }
        }
        mysql::TypeFloat => Ok(format_with_scratch(enc, |scratch| {
            AppendFormatFloat(scratch, f64::from(row.GetFloat32(idx)), floatPrec(col), 32)
        })),
        mysql::TypeDouble => Ok(format_with_scratch(enc, |scratch| {
            AppendFormatFloat(scratch, row.GetFloat64(idx), floatPrec(col), 64)
        })),
        mysql::TypeNewDecimal => Ok(row.GetMyDecimal(idx).String().into_bytes()),
        // 字符串/Blob/Bit：先按列 charset 更新编码器再 EncodeData。
        mysql::TypeString
        | mysql::TypeVarString
        | mysql::TypeVarchar
        | mysql::TypeBit
        | mysql::TypeTinyBlob
        | mysql::TypeMediumBlob
        | mysql::TypeLongBlob
        | mysql::TypeBlob => {
            enc.UpdateDataEncoding(col.Charset);
            Ok(enc.EncodeData(&row.GetBytes(idx)))
        }
        mysql::TypeDate | mysql::TypeDatetime | mysql::TypeTimestamp => {
            Ok(row.GetTime(idx).String().into_bytes())
        }
        mysql::TypeDuration => Ok(row
            .GetDuration(idx, i32::from(col.Decimal))
            .String()
            .into_bytes()),
        mysql::TypeEnum => {
            enc.UpdateDataEncoding(col.Charset);
            Ok(enc.EncodeData(row.GetEnum(idx).String().as_bytes()))
        }
        mysql::TypeSet => {
            enc.UpdateDataEncoding(col.Charset);
            Ok(enc.EncodeData(row.GetSet(idx).String().as_bytes()))
        }
        // JSON / 向量固定用默认 collation 做结果字符集转换。
        mysql::TypeJSON => {
            enc.UpdateDataEncoding(mysql::DefaultCollationID);
            Ok(enc.EncodeData(row.GetJSON(idx).String().as_bytes()))
        }
        mysql::TypeTiDBVectorFloat32 => {
            enc.UpdateDataEncoding(mysql::DefaultCollationID);
            Ok(enc.EncodeData(row.GetVectorFloat32(idx).String().as_bytes()))
        }
        _ => Err(ErrInvalidType),
    }
}

/// 计算浮点输出精度：仅当 Table 为空且 Decimal 为有效定点值时覆盖默认。
fn floatPrec(col: &ColumnInfo) -> i32 {
    if col.Decimal > 0 && usize::from(col.Decimal) != mysql::NotFixedDec && col.Table.is_empty() {
        return i32::from(col.Decimal);
    }
    types::UnspecifiedLength as i32
}

/// 绝对值 ≥ 此阈值时改用科学计数法（MySQL 文本协议约定）。
const expFormatBig: f64 = 1e15;
/// 绝对值非零且 < 此阈值时改用科学计数法。
const expFormatSmall: f64 = 1e-15;
/// float32 科学计数法默认有效位数。
const defaultMySQLPrec: i32 = 5;

/// Appends a float using MySQL's text-protocol formatting rules.
/// 按 MySQL 文本协议规则追加浮点字符串（定点或科学计数法，并裁剪多余零与 `e+`）。
pub fn AppendFormatFloat(mut input: Vec<u8>, f_val: f64, mut prec: i32, bit_size: i32) -> Vec<u8> {
    let abs_val = f_val.abs();
    // Inf/NaN 在 MySQL 文本协议中显示为 "0"。
    if abs_val > f64::MAX || abs_val.is_nan() {
        input.push(b'0');
        return input;
    }

    // 按 32/64 位阈值判断是否使用科学计数法。
    let is_e_format = if bit_size == 32 {
        let value = abs_val as f32;
        value >= expFormatBig as f32 || (value != 0.0 && value < expFormatSmall as f32)
    } else {
        abs_val >= expFormatBig || (abs_val != 0.0 && abs_val < expFormatSmall)
    };

    if !is_e_format {
        return goish::strconv::AppendFloat(
            input,
            f_val,
            b'f',
            i64::from(prec),
            i64::from(bit_size),
        );
    }
    if bit_size == 32 {
        prec = defaultMySQLPrec;
    }

    let prefix_len = input.len();
    let mut out =
        goish::strconv::AppendFloat(input, f_val, b'e', i64::from(prec), i64::from(bit_size));

    // MySQL 输出形如 "1e20" 而非 "1e+20"，去掉指数正号。
    if let Some(plus_pos) = out[prefix_len..].iter().position(|byte| *byte == b'+') {
        if plus_pos > 0 {
            out.remove(prefix_len + plus_pos);
        }
    }

    let e_pos = out[prefix_len..]
        .iter()
        .position(|byte| *byte == b'e')
        .map(|pos| prefix_len + pos)
        .unwrap_or(out.len());
    let point_pos = out[prefix_len..e_pos]
        .iter()
        .position(|byte| *byte == b'.')
        .map(|pos| prefix_len + pos)
        .unwrap_or(e_pos);

    // 裁剪尾随零与孤立小数点，得到更紧凑的科学计数法文本。
    let mut valid_pos = e_pos;
    for index in (point_pos..e_pos).rev() {
        if out[index] != b'0' && out[index] != b'.' {
            break;
        }
        valid_pos = index;
    }
    out.drain(valid_pos..e_pos);
    out
}
