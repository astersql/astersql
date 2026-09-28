// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Parquet 原始列值到内部 Datum 的转换（DECIMAL、时间、INT96、字节数组）。
//
// 可编译路径提供简化 Datum；块注释内保留更完整的 Go setter 工厂草稿。
// limitations under the License.

// Parquet 原始列值如何转换为 TiDB Datum，包括 DECIMAL、时间、INT96 和 byte array。
//
// setter 对应 Go 泛型函数类型：把 Parquet primitive value 写入 TiDB Datum。
// pub type setter<T> = fn(T, &mut types::Datum) -> Result<(), Error>;
//
// pub static zeroMyDecimal: types::MyDecimal = types::MyDecimal {};
//
// maximumDecimalBytes 对应 Go 的最大直接解析字节数，避免 MyDecimal wordbuf 溢出。
// pub const maximumDecimalBytes: usize = 33;
//
// initializeMyDecimal 对应 Go 的 Datum decimal 复用逻辑。
// 已经是 MysqlDecimal 时清零并复用，否则新建 MyDecimal 放回 Datum。
// pub fn initializeMyDecimal(d: &mut types::Datum) -> &mut types::MyDecimal {
//     if d.Kind() == types::KindMysqlDecimal {
//         let dec = d.GetMysqlDecimal();
//         *dec = zeroMyDecimal;
//         return dec;
//     }
//
//     let dec = Box::new(types::MyDecimal {});
//     d.SetMysqlDecimal(dec);
//     d.GetMysqlDecimal()
// }
//
// setDatumFromDecimalByte 对应 Go 的 BYTE_ARRAY/FIXED_LEN_BYTE_ARRAY DECIMAL 解析。
// 它先裁剪二补码符号扩展，再决定走 MyDecimal 直接解析还是字符串 fallback。
// pub fn setDatumFromDecimalByte(
//     d: &mut types::Datum,
//     mut val: Vec<u8>,
//     scale: i32,
// ) -> Result<(), Error> {
//     if val.is_empty() {
//         return Err(Error::new("invalid parquet decimal byte array"));
//     }
//
//     let negative = (val[0] & 0x80) != 0;
//     let mut start = 0usize;
//     while start < val.len() {
//         if (negative && val[start] != 0xff) || (!negative && val[start] != 0x00) {
//             break;
//         }
//         start += 1;
//     }
// Go 保留至少一个字节；这里保留同样的 start-1 下界处理。
//     start = start.saturating_sub(1);
//     val = val[start..].to_vec();
//
//     if val.len() >= maximumDecimalBytes || scale > 81 {
//         let s = getStringFromParquetByte(val, scale);
//         d.SetBytesAsString(s, "utf8mb4_bin", 0);
//         return Ok(());
//     }
//
//     let dec = initializeMyDecimal(d);
//     dec.FromParquetArray(val, scale)
// }
//
// getStringFromParquetByte 对应 Go 的大 DECIMAL 字符串 fallback。
// 该逻辑把 base-256 二补码整数原地转换为 base-10 字符串，并在 scale 位置插入小数点。
// pub fn getStringFromParquetByte(mut rawBytes: Vec<u8>, scale: i32) -> Vec<u8> {
//     let base: u64 = 1_000_000_000;
//     let baseDigits = 9;
//
//     let negative = (rawBytes[0] & 0x80) != 0;
//     if negative {
//         for b in rawBytes.iter_mut() {
//             *b = !*b;
//         }
//         for i in (0..rawBytes.len()).rev() {
//             rawBytes[i] = rawBytes[i].wrapping_add(1);
//             if rawBytes[i] != 0 {
//                 break;
//             }
//         }
//     }
//
//     let mut s: Vec<u8> = Vec::with_capacity(64);
//     let mut n = 0;
//     let mut nDigits = 0;
//     let mut startIndex = 0usize;
//     let endIndex = rawBytes.len();
//
//     while startIndex < endIndex && rawBytes[startIndex] == 0 {
//         startIndex += 1;
//     }
//
//     while startIndex < endIndex {
//         let mut rem: u64 = 0;
//         for i in startIndex..endIndex {
//             let v = (rem << 8) | rawBytes[i] as u64;
//             let q = v / base;
//             rem = v % base;
//             rawBytes[i] = q as u8;
//             if q == 0 && i == startIndex {
//                 startIndex += 1;
//             }
//         }
//
// Go 写作 for range baseDigits；这里显式循环 9 次。
//         for _ in 0..baseDigits {
//             s.push((48 + rem % 10) as u8);
//             n += 1;
//             nDigits += 1;
//             rem /= 10;
//             if nDigits == scale {
//                 s.push(b'.');
//                 n += 1;
//             }
//             if startIndex == endIndex && rem == 0 {
//                 break;
//             }
//         }
//     }
//
//     while nDigits < scale + 1 {
//         s.push(b'0');
//         n += 1;
//         nDigits += 1;
//         if nDigits == scale {
//             s.push(b'.');
//             n += 1;
//         }
//     }
//
//     if negative {
//         s.push(b'-');
//     }
//
// Go 最后反转字符串，因为前面按低位到高位追加。
//     for i in 0..(s.len() / 2) {
//         let j = s.len() - 1 - i;
//         s.swap(i, j);
//     }
//
//     s
// }
//
// setParquetDecimalFromInt64 对应 Go 的 int32/int64 DECIMAL 转换。
// 先按 unscaled 整数初始化，再按 scale shift 并用 truncate 模式 round。
// pub fn setParquetDecimalFromInt64(
//     unscaled: i64,
//     dec: &mut types::MyDecimal,
//     decimalMeta: schema::DecimalMetadata,
// ) -> Result<(), Error> {
//     dec.FromInt(unscaled);
//
//     let scale = decimalMeta.Scale as i32;
//     dec.Shift(-scale)?;
//     dec.Round(dec, scale, types::ModeTruncate)
// }
//
// getBoolDataSetter 对应 Go 的 bool setter：true 写 1，false 写 0。
// pub fn getBoolDataSetter(val: bool, d: &mut types::Datum) -> Result<(), Error> {
//     if val {
//         d.SetUint64(1);
//     } else {
//         d.SetUint64(0);
//     }
//     Ok(())
// }
//
// getInt32Setter 对应 Go 的 int32 setter 工厂。
// TIME/TIMESTAMP 的 IsAdjustedToUTC 语义保留在各分支注释中。
// pub fn getInt32Setter(converted: &convertedType, loc: &time::Location) -> Option<setter<i32>> {
//     match converted.converted {
//         schema::ConvertedTypes::Decimal => Some(|val, d| {
//             let dec = initializeMyDecimal(d);
//             setParquetDecimalFromInt64(val as i64, dec, converted.decimalMeta)
//         }),
//         schema::ConvertedTypes::Date => Some(|mut val, d| {
//             if !converted.sparkRebaseMicros.timeZoneID.is_empty() {
//                 val = rebaseJulianToGregorianDays(val);
//             }
//             let t = arrow::Date32(val).ToTime();
//             let mysqlTime = types::NewTime(types::FromGoTime(t), mysql::TypeDate, 0);
//             d.SetMysqlTime(mysqlTime);
//             Ok(())
//         }),
//         schema::ConvertedTypes::TimeMillis => Some(|val, d| {
//             let mut t = time::UnixMilli(val as i64).In(time::UTC);
//             if converted.IsAdjustedToUTC {
//                 t = t.In(loc);
//             }
//             let mysqlTime = types::NewTime(types::FromGoTime(t), mysql::TypeTimestamp, 6);
//             d.SetMysqlTime(mysqlTime);
//             Ok(())
//         }),
//         schema::ConvertedTypes::Int32
//         | schema::ConvertedTypes::Uint32
//         | schema::ConvertedTypes::Int16
//         | schema::ConvertedTypes::Uint16
//         | schema::ConvertedTypes::Int8
//         | schema::ConvertedTypes::Uint8
//         | schema::ConvertedTypes::None => Some(|val, d| {
//             d.SetInt64(val as i64);
//             Ok(())
//         }),
//         _ => None,
//     }
// }
// */
use crate::parser::ConvertedType;
use crate::spark_rebase::SparkRebaseMicrosLookup;
use crate::{Error, Result};
/// 直接解析 DECIMAL 字节的最大长度，避免 MyDecimal 溢出。
pub const MAXIMUM_DECIMAL_BYTES: usize = 33;
/// Unix epoch 对应的儒略日。
pub const JULIAN_DAY_OF_UNIX_EPOCH: i64 = 2_440_588;
/// 一天的微秒数。
pub const MICROS_PER_DAY: i64 = 86_400_000_000;
#[derive(Clone, Debug, PartialEq)]
/// 简化的内部值载体（对齐 TiDB Datum 常用形态）。
pub enum Datum {
    /// 空值。
    Null,
    /// 无符号整数。
    UInt(u64),
    /// 有符号整数。
    Int(i64),
    /// 十进制文本（大数或带 scale）。
    Decimal(String),
    /// 时间/时间戳，存 Unix 微秒。
    TimeMicros(i64),
    /// 单精度浮点。
    Float32(f32),
    /// 双精度浮点。
    Float64(f64),
    /// 原始/字符串字节。
    Bytes(Vec<u8>),
}
#[derive(Clone, Debug)]
/// 列转换上下文：逻辑类型、scale、UTC 调整与可选 Spark rebase。
pub struct ConvertedInfo {
    pub converted: ConvertedType,
    pub scale: i32,
    pub adjusted_to_utc: bool,
    pub timezone_offset_seconds: i32,
    pub spark_rebase: Option<SparkRebaseMicrosLookup>,
}
/// 把无符号大端整数原地转为十进制数字串。
fn magnitude_to_decimal(mut bytes: Vec<u8>) -> String {
    let mut digits = Vec::<u8>::new();
    while bytes.iter().any(|b| *b != 0) {
        let mut remainder = 0u16;
        for byte in &mut bytes {
            let value = (remainder << 8) | *byte as u16;
            *byte = (value / 10) as u8;
            remainder = value % 10;
        }
        digits.push(b'0' + remainder as u8);
    }
    if digits.is_empty() {
        digits.push(b'0');
    }
    digits.reverse();
    String::from_utf8(digits).unwrap()
}
/// 二补码 DECIMAL 字节 → 带小数点的十进制字符串。
pub fn decimal_bytes_to_string(input: &[u8], scale: i32) -> Result<String> {
    if input.is_empty() {
        return Err(Error("invalid parquet decimal byte array".into()));
    }
    if scale < 0 {
        return Err(Error(format!("invalid decimal scale {scale}")));
    }
    // 最高位为符号；负数先取反加一得到绝对值。
    let negative = input[0] & 0x80 != 0;
    let mut magnitude = input.to_vec();
    if negative {
        for byte in &mut magnitude {
            *byte = !*byte;
        }
        for byte in magnitude.iter_mut().rev() {
            let (next, carry) = byte.overflowing_add(1);
            *byte = next;
            if !carry {
                break;
            }
        }
    }
    while magnitude.len() > 1 && magnitude[0] == 0 {
        magnitude.remove(0);
    }
    let mut text = magnitude_to_decimal(magnitude);
    let scale = scale as usize;
    if scale > 0 {
        if text.len() <= scale {
            text = format!("{}{}", "0".repeat(scale + 1 - text.len()), text);
        }
        text.insert(text.len() - scale, '.');
    }
    if negative && text.chars().any(|c| c.is_ascii_digit() && c != '0') {
        text.insert(0, '-');
    }
    Ok(text)
}
/// DECIMAL 字节解析为 Datum::Decimal。
pub fn set_datum_from_decimal_bytes(bytes: &[u8], scale: i32) -> Result<Datum> {
    Ok(Datum::Decimal(decimal_bytes_to_string(bytes, scale)?))
}
/// 把整数按 scale 插入小数点，得到 Decimal 文本。
fn scaled_decimal(value: i64, scale: i32) -> Result<Datum> {
    let negative = value < 0;
    let magnitude = (value as i128).abs().to_string();
    let mut text = magnitude;
    let scale = scale.max(0) as usize;
    if scale > 0 {
        if text.len() <= scale {
            text = format!("{}{}", "0".repeat(scale + 1 - text.len()), text);
        }
        text.insert(text.len() - scale, '.');
    }
    if negative {
        text.insert(0, '-');
    }
    Ok(Datum::Decimal(text))
}
/// INT32：按 ConvertedType 分派 DECIMAL/DATE/TIME_MILLIS/整型。
pub fn convert_int32(value: i32, info: &ConvertedInfo) -> Result<Datum> {
    Ok(match info.converted {
        ConvertedType::Decimal => scaled_decimal(value as i64, info.scale)?,
        ConvertedType::Date => {
            let days = if info.spark_rebase.is_some() {
                crate::spark_rebase::rebase_julian_to_gregorian_days(value)
            } else {
                value
            };
            Datum::TimeMicros(days as i64 * MICROS_PER_DAY)
        }
        ConvertedType::TimeMillis => Datum::TimeMicros(
            value as i64 * 1000
                + if info.adjusted_to_utc {
                    info.timezone_offset_seconds as i64 * 1_000_000
                } else {
                    0
                },
        ),
        _ => Datum::Int(value as i64),
    })
}
/// INT64：DECIMAL/TIME/TIMESTAMP（可经 Spark rebase）/整型。
pub fn convert_int64(mut value: i64, info: &ConvertedInfo) -> Result<Datum> {
    Ok(match info.converted {
        ConvertedType::Decimal => scaled_decimal(value, info.scale)?,
        ConvertedType::TimeMicros => Datum::TimeMicros(
            value
                + if info.adjusted_to_utc {
                    info.timezone_offset_seconds as i64 * 1_000_000
                } else {
                    0
                },
        ),
        // 毫秒时间戳：先 ×1000 做微秒 rebase，再除回毫秒后转微秒 Datum。
        ConvertedType::TimestampMillis => {
            if let Some(lookup) = &info.spark_rebase {
                value = lookup.rebase(
                    value
                        .checked_mul(1000)
                        .ok_or_else(|| Error("timestamp overflow".into()))?,
                )? / 1000;
            }
            Datum::TimeMicros(
                value * 1000
                    + if info.adjusted_to_utc {
                        info.timezone_offset_seconds as i64 * 1_000_000
                    } else {
                        0
                    },
            )
        }
        // 微秒时间戳：可选 Spark rebase，再按需加本地时区偏移。
        ConvertedType::TimestampMicros => {
            if let Some(lookup) = &info.spark_rebase {
                value = lookup.rebase(value)?;
            }
            Datum::TimeMicros(
                value
                    + if info.adjusted_to_utc {
                        info.timezone_offset_seconds as i64 * 1_000_000
                    } else {
                        0
                    },
            )
        }
        ConvertedType::None => Datum::Int(value),
        _ => Datum::UInt(value as u64),
    })
}
/// 由 Unix 微秒构造 INT96（日纳秒 + 儒略日），测试辅助。
pub fn new_int96(microseconds: i64) -> [u8; 12] {
    let day = microseconds.div_euclid(MICROS_PER_DAY) + JULIAN_DAY_OF_UNIX_EPOCH;
    let nanos = microseconds.rem_euclid(MICROS_PER_DAY) as u64 * 1000;
    let mut out = [0; 12];
    out[..8].copy_from_slice(&nanos.to_le_bytes());
    out[8..].copy_from_slice(&(day as u32).to_le_bytes());
    out
}
/// INT96 → Unix 微秒，供 Spark rebasing 使用。
pub fn int96_to_unix_micros(value: [u8; 12]) -> i64 {
    let nanos = u64::from_le_bytes(value[..8].try_into().unwrap()) as i64;
    let day = u32::from_le_bytes(value[8..].try_into().unwrap()) as i64;
    (day - JULIAN_DAY_OF_UNIX_EPOCH) * MICROS_PER_DAY + nanos / 1000
}

/// INT96 → Unix 微秒，并对 Go `types.FromGoTime` 保留的亚微秒精度做同等舍入。
fn int96_to_unix_micros_rounded(value: [u8; 12]) -> i64 {
    let nanos_of_day = u64::from_le_bytes(value[..8].try_into().unwrap());
    int96_to_unix_micros(value) + i64::from(nanos_of_day % 1000 >= 500)
}
/// INT96 列：转微秒、可选 rebase、可选 UTC 调整。
pub fn convert_int96(value: [u8; 12], info: &ConvertedInfo) -> Result<Datum> {
    let mut micros = if let Some(lookup) = &info.spark_rebase {
        lookup.rebase(int96_to_unix_micros(value))?
    } else {
        int96_to_unix_micros_rounded(value)
    };
    if info.adjusted_to_utc {
        micros += info.timezone_offset_seconds as i64 * 1_000_000;
    }
    Ok(Datum::TimeMicros(micros))
}
/// BYTE_ARRAY / FIXED_LEN：DECIMAL 走定标解析，否则原样字节。
pub fn convert_bytes(value: &[u8], info: &ConvertedInfo) -> Result<Datum> {
    if info.converted == ConvertedType::Decimal {
        set_datum_from_decimal_bytes(value, info.scale)
    } else {
        Ok(Datum::Bytes(value.to_vec()))
    }
}
/// Go 风格别名。
pub fn setDatumFromDecimalByte(v: &[u8], s: i32) -> Result<Datum> {
    set_datum_from_decimal_bytes(v, s)
}
/// Go 风格别名。
pub fn getStringFromParquetByte(v: &[u8], s: i32) -> Result<String> {
    decimal_bytes_to_string(v, s)
}
/// Go 风格别名。
pub fn newInt96(v: i64) -> [u8; 12] {
    new_int96(v)
}
/// Go 风格别名。
pub fn int96ToUnixMicros(v: [u8; 12]) -> i64 {
    int96_to_unix_micros(v)
}
