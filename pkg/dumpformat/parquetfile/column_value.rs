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

// SQL RawBytes 如何解析为 Parquet 列值，并估算内存、写入列缓冲。
//
// 对应 Go 的 column_value.go：把 database/sql 文本协议字节按物理/逻辑类型
// 转成 `ColumnValue`，支持 DECIMAL 定标整数、TIMESTAMP 微秒、定长二补码等。
// limitations under the License.

// SQL RawBytes 如何转换、估算内存并写入 Parquet column writer。
//
// Go 用 unsafe.Sizeof 估算 slice header；用 size_of 保留“头部 + 数据长度”的估算形状。
// pub const byteArraySliceHeaderBytes: i64 = std::mem::size_of::<parquet::ByteArray>() as i64;
// pub const fixedLenByteArraySliceHeaderBytes: i64 =
//     std::mem::size_of::<parquet::FixedLenByteArray>() as i64;
//
// parseRawColumnValue 对应 Go 的核心转换函数。
// 返回值三元组含义保持为：解析后的值、是否应按 NULL 编码、错误。
// pub fn parseRawColumnValue(rawValue: sql::RawBytes, column: column) -> Result<(any, bool), Error> {
//     match column.Physical {
//         parquet::Types::Boolean => {
//             let s = String::from_utf8_lossy(&rawValue);
//             let v = strconv::ParseBool(&s)?;
//             Ok((any::from(v), false))
//         }
//         parquet::Types::Int32 => {
//             let s = String::from_utf8_lossy(&rawValue);
//             if matches!(column.Logical, schema::LogicalType::DecimalLogicalType(_)) {
//                 let scaled = parseDecimalToScaledInteger(&s, column.columnType.Scale)?;
//                 if !scaled.IsInt64() {
//                     return Err(Error::new(format!("decimal value {:?} does not fit in INT32", s)));
//                 }
//                 let value = scaled.Int64();
//                 if value < math::MinInt32 as i64 || value > math::MaxInt32 as i64 {
//                     return Err(Error::new(format!("decimal value {:?} does not fit in INT32", s)));
//                 }
//                 return Ok((any::from(value as i32), false));
//             }
//             let v = strconv::ParseInt(&s, 10, 32)?;
//             Ok((any::from(v as i32), false))
//         }
//         parquet::Types::Int64 => {
//             let s = String::from_utf8_lossy(&rawValue);
//             if matches!(column.Logical, schema::LogicalType::TimestampLogicalType(_)) {
// Go 的 text protocol 时间值形如 "YYYY-MM-DD HH:MM:SS[.fraction]"。
// Parse 失败且允许空值编码时，原实现把非法 temporal 写成 NULL。
//                 let parsed = time::Parse(time::DateTime, &s);
//                 if let Err(err) = parsed {
//                     if column.allowsNullEncoding {
//                         return Ok((any::nil(), true));
//                     }
//                     return Err(err.into());
//                 }
//                 let t = parsed.unwrap();
//                 return Ok((any::from(unixTimestampByUnit(t, column.timestampUnit)), false));
//             }
//             if matches!(column.Logical, schema::LogicalType::DecimalLogicalType(_)) {
//                 let scaled = parseDecimalToScaledInteger(&s, column.columnType.Scale)?;
//                 if !scaled.IsInt64() {
//                     return Err(Error::new(format!("decimal value {:?} does not fit in INT64", s)));
//                 }
//                 return Ok((any::from(scaled.Int64()), false));
//             }
//             let v = strconv::ParseInt(&s, 10, 64)?;
//             Ok((any::from(v), false))
//         }
//         parquet::Types::Float => {
//             let s = String::from_utf8_lossy(&rawValue);
// 当前 toColumnType 会把 SQL FLOAT 映射到 DOUBLE；保留此分支给外部/custom schema。
//             let v = strconv::ParseFloat(&s, 32)?;
//             Ok((any::from(v as f32), false))
//         }
//         parquet::Types::Double => {
//             let s = String::from_utf8_lossy(&rawValue);
//             let v = strconv::ParseFloat(&s, 64)?;
//             Ok((any::from(v), false))
//         }
//         parquet::Types::ByteArray => {
// Go 显式 clone RawBytes，避免 database/sql 复用底层内存后污染 Parquet 缓冲。
//             let cloned = rawValue.to_vec();
//             Ok((any::from(parquet::ByteArray::from(cloned)), false))
//         }
//         parquet::Types::FixedLenByteArray => {
//             if matches!(column.Logical, schema::LogicalType::DecimalLogicalType(_)) {
//                 let s = String::from_utf8_lossy(&rawValue);
//                 let scaled = parseDecimalToScaledInteger(&s, column.columnType.Scale)?;
//                 let encoded = toFixedLenTwoComplement(&scaled, column.TypeLength)?;
//                 return Ok((any::from(parquet::FixedLenByteArray::from(encoded)), false));
//             }
//             if column.TypeLength <= 0 {
//                 return Err(Error::new(format!("invalid fixed-size byte width {}", column.TypeLength)));
//             }
//             let cloned = rawValue.to_vec();
//             if cloned.len() != column.TypeLength as usize {
//                 return Err(Error::new(format!(
//                     "fixed-len byte array width mismatch: got {}, expected {}",
//                     cloned.len(),
//                     column.TypeLength
//                 )));
//             }
//             Ok((any::from(parquet::FixedLenByteArray::from(cloned)), false))
//         }
//         _ => Err(Error::new(format!(
//             "unsupported parquet physical type {}",
//             column.Physical
//         ))),
//     }
// }
//
// unixTimestampByUnit 对应 Go 的时间单位选择辅助函数。
// 未识别单位默认微秒，保留 Go default 分支。
// pub fn unixTimestampByUnit(t: time::Time, unit: schema::TimeUnitType) -> i64 {
//     match unit {
//         schema::TimeUnitMillis => t.UnixMilli(),
//         schema::TimeUnitMicros => t.UnixMicro(),
//         schema::TimeUnitNanos => t.UnixNano(),
//         _ => t.UnixMicro(),
//     }
// }
//
// appendColumnValue 对应 Go 的类型断言追加。
// 这里仍按 column.Physical 分派，表达“调用方必须保证 value 类型匹配”的 Go 语义。
// pub fn appendColumnValue(buffer: &mut columnBuffer, column: &column, value: any) -> Result<(), Error> {
//     match column.Physical {
//         parquet::Types::Boolean => buffer.boolValues.push(value.downcast::<bool>()),
//         parquet::Types::Int32 => buffer.int32Values.push(value.downcast::<i32>()),
//         parquet::Types::Int64 => buffer.int64Values.push(value.downcast::<i64>()),
//         parquet::Types::Float => buffer.float32Values.push(value.downcast::<f32>()),
//         parquet::Types::Double => buffer.float64Values.push(value.downcast::<f64>()),
//         parquet::Types::ByteArray => buffer.byteArrayValues.push(value.downcast::<parquet::ByteArray>()),
//         parquet::Types::FixedLenByteArray => {
//             buffer.fixedLenByteArrayValues.push(value.downcast::<parquet::FixedLenByteArray>())
//         }
//         _ => {
//             return Err(Error::new(format!(
//                 "unsupported parquet physical type {}",
//                 column.Physical
//             )));
//         }
//     }
//     Ok(())
// }
//
// accountColumnValueMemoryBytes 对应 Go 的内存估算函数。
// 定长数值按字节宽度返回，字节数组额外加 slice header 近似成本。
// pub fn accountColumnValueMemoryBytes(column: &column, value: any) -> i64 {
//     match column.Physical {
//         parquet::Types::Boolean => 1,
//         parquet::Types::Int32 | parquet::Types::Float => 4,
//         parquet::Types::Int64 | parquet::Types::Double => 8,
//         parquet::Types::ByteArray => {
//             byteArraySliceHeaderBytes + value.downcast::<parquet::ByteArray>().len() as i64
//         }
//         parquet::Types::FixedLenByteArray => {
//             fixedLenByteArraySliceHeaderBytes
//                 + value.downcast::<parquet::FixedLenByteArray>().len() as i64
//         }
//         _ => 0,
//     }
// }
//
// writeColumnBatch 对应 Go 的 column writer 类型分派。
// defLevels 只有 allowsNullEncoding 时传入；required 列保持 nil/None。
// pub fn writeColumnBatch(
//     columnWriter: file::ColumnChunkWriter,
//     column: column,
//     buffer: columnBuffer,
// ) -> Result<(), Error> {
//     let defLevels = if column.allowsNullEncoding {
//         Some(buffer.defLevels)
//     } else {
//         None
//     };
//
//     match columnWriter {
//         file::ColumnChunkWriter::Boolean(mut writer) => {
//             writer.WriteBatch(buffer.boolValues, defLevels, None)?;
//         }
//         file::ColumnChunkWriter::Int32(mut writer) => {
//             writer.WriteBatch(buffer.int32Values, defLevels, None)?;
//         }
//         file::ColumnChunkWriter::Int64(mut writer) => {
//             writer.WriteBatch(buffer.int64Values, defLevels, None)?;
//         }
//         file::ColumnChunkWriter::Float32(mut writer) => {
//             writer.WriteBatch(buffer.float32Values, defLevels, None)?;
//         }
//         file::ColumnChunkWriter::Float64(mut writer) => {
//             writer.WriteBatch(buffer.float64Values, defLevels, None)?;
//         }
//         file::ColumnChunkWriter::ByteArray(mut writer) => {
//             writer.WriteBatch(buffer.byteArrayValues, defLevels, None)?;
//         }
//         file::ColumnChunkWriter::FixedLenByteArray(mut writer) => {
//             writer.WriteBatch(buffer.fixedLenByteArrayValues, defLevels, None)?;
//         }
//         _ => {
//             return Err(Error::new(format!(
//                 "unsupported column chunk writer {:?}",
//                 columnWriter
//             )));
//         }
//     }
//     Ok(())
// }
//
// parseDecimalToScaledInteger converts decimal text into an integer scaled by
// 10^scale. This writer layer intentionally only performs conversion/serialization
// and does not enforce original SQL type/domain validity; callers are expected to
// pass already validated values. Extra fractional digits are truncated toward zero.
// parseDecimalToScaledInteger 对应 Go 的 big.Rat 转换：乘以 10^scale 后做有理数整除。
// pub fn parseDecimalToScaledInteger(s: &str, scale: i32) -> Result<big::Int, Error> {
//     if scale < 0 {
//         return Err(Error::new(format!("invalid decimal scale {}", scale)));
//     }
//     let mut rat = big::Rat::new();
//     if !rat.SetString(s) {
//         return Err(Error::new(format!("invalid decimal value {:?}", s)));
//     }
//     let multiplier = big::Int::Exp(big::NewInt(10), big::NewInt(scale as i64), None);
//     rat.Mul(big::Rat::SetInt(multiplier));
//     Ok(big::Int::Quo(rat.Num(), rat.Denom()))
// }
//
// toFixedLenTwoComplement 对应 Go 的 DECIMAL fixed byte array 编码。
// 它先验证 byteWidth 能容纳有符号数范围，再按正数补零、负数二补码写入固定宽度字节。
// pub fn toFixedLenTwoComplement(value: &big::Int, byteWidth: i32) -> Result<Vec<u8>, Error> {
//     if byteWidth <= 0 {
//         return Err(Error::new(format!("invalid fixed-size byte width {}", byteWidth)));
//     }
//
//     let bitWidth = (8 * byteWidth - 1) as u32;
//     let mut maxValue = big::Int::Lsh(big::NewInt(1), bitWidth);
//     maxValue.Sub(big::NewInt(1));
//     let minValue = big::Int::Neg(big::Int::Lsh(big::NewInt(1), bitWidth));
//     if value.Cmp(&minValue) < 0 || value.Cmp(&maxValue) > 0 {
//         return Err(Error::new(format!(
//             "decimal value {} does not fit in {} bytes",
//             value.String(),
//             byteWidth
//         )));
//     }
//
//     let mut encoded = vec![0u8; byteWidth as usize];
//     if value.Sign() >= 0 {
//         let rawBytes = value.Bytes();
//         let start = encoded.len() - rawBytes.len();
//         encoded[start..].copy_from_slice(&rawBytes);
//         return Ok(encoded);
//     }
//
//     let modulus = big::Int::Lsh(big::NewInt(1), (8 * byteWidth) as u32);
//     let twoComplement = big::Int::Add(value, modulus);
//     let rawBytes = twoComplement.Bytes();
//     let start = encoded.len() - rawBytes.len();
//     encoded[start..].copy_from_slice(&rawBytes);
//     Ok(encoded)
// }
// */
use crate::column_buffer::ColumnBuffer;
use crate::column_type::{Column, LogicalType, PhysicalType, TimeUnit};
use crate::{Error, Result};

#[derive(Clone, Debug, PartialEq)]
/// 写入 Parquet 列缓冲的中间值，按物理类型区分。
pub enum ColumnValue {
    /// 布尔列。
    Bool(bool),
    /// 32 位整数（含小精度 DECIMAL）。
    Int32(i32),
    /// 64 位整数 / TIMESTAMP / 中精度 DECIMAL。
    Int64(i64),
    /// 单精度浮点。
    Float32(f32),
    /// 双精度浮点。
    Float64(f64),
    /// 变长字节数组（STRING/BLOB 等）。
    Bytes(Vec<u8>),
    /// 定长字节数组（大精度 DECIMAL 二补码等）。
    FixedBytes(Vec<u8>),
}
/// 计算 10^scale，用于 DECIMAL 定标；scale 非法或溢出则报错。
fn pow10(scale: i32) -> Result<i128> {
    if scale < 0 {
        return Err(Error(format!("invalid decimal scale {scale}")));
    }
    let mut value = 1i128;
    for _ in 0..scale {
        value = value
            .checked_mul(10)
            .ok_or_else(|| Error("decimal scale overflow".into()))?;
    }
    Ok(value)
}
/// 把十进制文本乘以 10^scale 转为整数；多余小数位向零截断。
/// 对应 Go `parseDecimalToScaledInteger`。
pub fn parse_decimal_to_scaled_integer(input: &str, scale: i32) -> Result<i128> {
    // `big.Rat.SetString` does not trim surrounding whitespace.
    let text = input;
    if text.is_empty() {
        return Err(Error(format!("invalid decimal value {input:?}")));
    }
    // 解析可选正负号，再按整数部分与小数部分拆分。
    let (negative, body) = if let Some(v) = text.strip_prefix('-') {
        (true, v)
    } else if let Some(v) = text.strip_prefix('+') {
        (false, v)
    } else {
        (false, text)
    };
    let mut parts = body.split('.');
    let whole = parts.next().unwrap();
    let fraction = parts.next().unwrap_or("");
    if parts.next().is_some()
        || whole.is_empty() && fraction.is_empty()
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return Err(Error(format!("invalid decimal value {input:?}")));
    }
    let multiplier = pow10(scale)?;
    let whole_value = if whole.is_empty() {
        0
    } else {
        whole
            .parse::<i128>()
            .map_err(|_| Error(format!("decimal value {input:?} overflows")))?
    };
    let mut fraction_value = 0i128;
    for byte in fraction.bytes().take(scale as usize) {
        fraction_value = fraction_value * 10 + (byte - b'0') as i128;
    }
    // 小数位数不足 scale 时右侧补零，对齐定标位数。
    for _ in fraction.len().min(scale as usize)..scale as usize {
        fraction_value *= 10;
    }
    let value = whole_value
        .checked_mul(multiplier)
        .and_then(|v| v.checked_add(fraction_value))
        .ok_or_else(|| Error(format!("decimal value {input:?} overflows")))?;
    Ok(if negative { -value } else { value })
}
/// 将有符号整数写成固定宽度大端二补码，供 FIXED_LEN_BYTE_ARRAY DECIMAL 使用。
pub fn to_fixed_len_two_complement(value: i128, width: i32) -> Result<Vec<u8>> {
    if width <= 0 || width > 16 {
        return Err(Error(format!("invalid fixed-size byte width {width}")));
    }
    let bits = width as u32 * 8;
    let (min, max) = if bits == 128 {
        (i128::MIN, i128::MAX)
    } else {
        (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
    };
    if value < min || value > max {
        return Err(Error(format!(
            "decimal value {value} does not fit in {width} bytes"
        )));
    }
    let bytes = value.to_be_bytes();
    Ok(bytes[16 - width as usize..].to_vec())
}
/// 公历年月日到 Unix epoch 日数（Howard Hinnant 算法）。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = year - if month <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}
/// 给定年月的天数，含闰年二月。
fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}
/// 解析 `YYYY-MM-DD HH:MM:SS[.fraction]` 文本为 Unix 微秒。
/// 对应 Go text protocol 时间格式。
fn parse_datetime_micros(input: &str) -> Result<i64> {
    let (date, time) = input
        .split_once(' ')
        .ok_or_else(|| Error(format!("invalid datetime {input:?}")))?;
    let mut d = date.split('-');
    let (y, m, day) = (
        d.next().and_then(|v| v.parse::<i64>().ok()),
        d.next().and_then(|v| v.parse::<i64>().ok()),
        d.next().and_then(|v| v.parse::<i64>().ok()),
    );
    if d.next().is_some() {
        return Err(Error(format!("invalid datetime {input:?}")));
    }
    let (y, m, day) = (
        y.ok_or_else(|| Error(format!("invalid datetime {input:?}")))?,
        m.unwrap_or(0),
        day.unwrap_or(0),
    );
    if !(1..=12).contains(&m) || day < 1 || day > days_in_month(y, m) {
        return Err(Error(format!("invalid datetime {input:?}")));
    }
    let (main, fraction) = time.split_once('.').unwrap_or((time, ""));
    let mut t = main.split(':');
    let (h, min, sec) = (
        t.next().and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1),
        t.next().and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1),
        t.next().and_then(|v| v.parse::<i64>().ok()).unwrap_or(-1),
    );
    if t.next().is_some()
        || !(0..24).contains(&h)
        || !(0..60).contains(&min)
        || !(0..60).contains(&sec)
        || time.contains('.') && fraction.is_empty()
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(Error(format!("invalid datetime {input:?}")));
    }
    // 小数秒最多取 6 位微秒，不足右侧补零。
    let mut micros = 0i64;
    for b in fraction.bytes().take(6) {
        micros = micros * 10 + (b - b'0') as i64;
    }
    for _ in fraction.len().min(6)..6 {
        micros *= 10;
    }
    Ok((days_from_civil(y, m, day) * 86400 + h * 3600 + min * 60 + sec) * 1_000_000 + micros)
}
/// 按列的物理/逻辑类型解析 RawBytes。
/// 返回 `(值, is_null)`；TIMESTAMP 非法且允许空值编码时写 NULL。
pub fn parse_raw_column_value(raw: &[u8], column: &Column) -> Result<(Option<ColumnValue>, bool)> {
    let text = std::str::from_utf8(raw).unwrap_or("");
    let value = match column.column_type.physical {
        PhysicalType::Boolean => ColumnValue::Bool(match text {
            "1" | "t" | "T" | "true" | "TRUE" | "True" => true,
            "0" | "f" | "F" | "false" | "FALSE" | "False" => false,
            _ => return Err(Error(format!("invalid boolean value {text:?}"))),
        }),
        PhysicalType::Int32 => {
            let v = if matches!(column.column_type.logical, LogicalType::Decimal { .. }) {
                parse_decimal_to_scaled_integer(text, column.column_type.scale)?
            } else {
                text.parse::<i128>().map_err(|e| Error(e.to_string()))?
            };
            ColumnValue::Int32(
                i32::try_from(v)
                    .map_err(|_| Error(format!("value {text:?} does not fit in INT32")))?,
            )
        }
        PhysicalType::Int64 => {
            // TIMESTAMP：解析失败且 allows_null_encoding 时编码为 NULL。
            if matches!(column.column_type.logical, LogicalType::Timestamp { .. }) {
                match parse_datetime_micros(text) {
                    Ok(mut v) => {
                        v = match column.timestamp_unit {
                            TimeUnit::Millis => v / 1000,
                            TimeUnit::Micros => v,
                            TimeUnit::Nanos => v
                                .checked_mul(1000)
                                .ok_or_else(|| Error("timestamp overflow".into()))?,
                        };
                        ColumnValue::Int64(v)
                    }
                    Err(_) if column.allows_null_encoding => return Ok((None, true)),
                    Err(error) => return Err(error),
                }
            } else {
                let v = if matches!(column.column_type.logical, LogicalType::Decimal { .. }) {
                    parse_decimal_to_scaled_integer(text, column.column_type.scale)?
                } else {
                    text.parse::<i128>().map_err(|e| Error(e.to_string()))?
                };
                ColumnValue::Int64(
                    i64::try_from(v)
                        .map_err(|_| Error(format!("value {text:?} does not fit in INT64")))?,
                )
            }
        }
        PhysicalType::Float => ColumnValue::Float32(
            text.parse()
                .map_err(|e: std::num::ParseFloatError| Error(e.to_string()))?,
        ),
        PhysicalType::Double => ColumnValue::Float64(
            text.parse()
                .map_err(|e: std::num::ParseFloatError| Error(e.to_string()))?,
        ),
        // 显式 clone，避免底层缓冲复用污染 Parquet 列缓冲。
        PhysicalType::ByteArray => ColumnValue::Bytes(raw.to_vec()),
        PhysicalType::FixedLenByteArray => {
            if column.column_type.type_length <= 0 {
                return Err(Error(format!(
                    "invalid fixed-size byte width {}",
                    column.column_type.type_length
                )));
            }
            let bytes = if matches!(column.column_type.logical, LogicalType::Decimal { .. }) {
                to_fixed_len_two_complement(
                    parse_decimal_to_scaled_integer(text, column.column_type.scale)?,
                    column.column_type.type_length,
                )?
            } else {
                if raw.len() != column.column_type.type_length as usize {
                    return Err(Error(format!(
                        "fixed-len byte array width mismatch: got {}, expected {}",
                        raw.len(),
                        column.column_type.type_length
                    )));
                }
                raw.to_vec()
            };
            ColumnValue::FixedBytes(bytes)
        }
        PhysicalType::Int96 => return Err(Error("unsupported parquet physical type Int96".into())),
    };
    Ok((Some(value), false))
}
/// 按物理类型把 `ColumnValue` 追加到对应列缓冲；类型不匹配则报错。
pub fn append_column_value(
    buffer: &mut ColumnBuffer,
    column: &Column,
    value: ColumnValue,
) -> Result<()> {
    match (column.column_type.physical, value) {
        (PhysicalType::Boolean, ColumnValue::Bool(v)) => buffer.bool_values.push(v),
        (PhysicalType::Int32, ColumnValue::Int32(v)) => buffer.int32_values.push(v),
        (PhysicalType::Int64, ColumnValue::Int64(v)) => buffer.int64_values.push(v),
        (PhysicalType::Float, ColumnValue::Float32(v)) => buffer.float32_values.push(v),
        (PhysicalType::Double, ColumnValue::Float64(v)) => buffer.float64_values.push(v),
        (PhysicalType::ByteArray, ColumnValue::Bytes(v)) => buffer.byte_array_values.push(v),
        (PhysicalType::FixedLenByteArray, ColumnValue::FixedBytes(v)) => {
            buffer.fixed_len_byte_array_values.push(v)
        }
        _ => return Err(Error("column value type mismatch".into())),
    }
    Ok(())
}
/// 估算单个列值占用字节（定长按宽度，变长含 Vec 头部近似）。
pub fn account_column_value_memory_bytes(value: &ColumnValue) -> i64 {
    match value {
        ColumnValue::Bool(_) => 1,
        ColumnValue::Int32(_) | ColumnValue::Float32(_) => 4,
        ColumnValue::Int64(_) | ColumnValue::Float64(_) => 8,
        ColumnValue::Bytes(v) | ColumnValue::FixedBytes(v) => {
            std::mem::size_of::<Vec<u8>>() as i64 + v.len() as i64
        }
    }
}
/// 从列缓冲按物理类型取出一批值（简化版 WriteBatch 分派）。
pub fn write_column_batch(buffer: &ColumnBuffer, column: &Column) -> Result<Vec<ColumnValue>> {
    let values = match column.column_type.physical {
        PhysicalType::Boolean => buffer
            .bool_values
            .iter()
            .copied()
            .map(ColumnValue::Bool)
            .collect(),
        PhysicalType::Int32 => buffer
            .int32_values
            .iter()
            .copied()
            .map(ColumnValue::Int32)
            .collect(),
        PhysicalType::Int64 => buffer
            .int64_values
            .iter()
            .copied()
            .map(ColumnValue::Int64)
            .collect(),
        PhysicalType::Float => buffer
            .float32_values
            .iter()
            .copied()
            .map(ColumnValue::Float32)
            .collect(),
        PhysicalType::Double => buffer
            .float64_values
            .iter()
            .copied()
            .map(ColumnValue::Float64)
            .collect(),
        PhysicalType::ByteArray => buffer
            .byte_array_values
            .iter()
            .cloned()
            .map(ColumnValue::Bytes)
            .collect(),
        PhysicalType::FixedLenByteArray => buffer
            .fixed_len_byte_array_values
            .iter()
            .cloned()
            .map(ColumnValue::FixedBytes)
            .collect(),
        PhysicalType::Int96 => return Err(Error("unsupported column chunk writer Int96".into())),
    };
    Ok(values)
}
/// Go 风格别名：`parse_raw_column_value`。
pub fn parseRawColumnValue(r: &[u8], c: &Column) -> Result<(Option<ColumnValue>, bool)> {
    parse_raw_column_value(r, c)
}
/// Go 风格别名：`append_column_value`。
pub fn appendColumnValue(b: &mut ColumnBuffer, c: &Column, v: ColumnValue) -> Result<()> {
    append_column_value(b, c, v)
}
/// Go 风格别名：`parse_decimal_to_scaled_integer`。
pub fn parseDecimalToScaledInteger(s: &str, n: i32) -> Result<i128> {
    parse_decimal_to_scaled_integer(s, n)
}
/// Go 风格别名：`to_fixed_len_two_complement`。
pub fn toFixedLenTwoComplement(v: i128, n: i32) -> Result<Vec<u8>> {
    to_fixed_len_two_complement(v, n)
}
