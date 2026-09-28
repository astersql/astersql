// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Vector string builtin semantics ported from `builtin_string_vec.go`.
//
// The surrounding expression signature types are migrated by separate tasks, so this file
// exposes the row-independent vector kernel they can call. `Value::Null` models chunk nulls;
// binary operations always work on bytes, while UTF-8 operations use Go-compatible lossy rune
// iteration. Every input row produces exactly one output row.

// 字符串内建向量化语义，移植自 `builtin_string_vec.go`。
//
// 表达式签名类型由其他任务迁移；本文件暴露行无关的向量内核供其调用。
// `Value::Null` 建模 chunk 空值；二进制按字节，UTF-8 使用与 Go 兼容的有损
// rune 迭代。每行输入恰好产生一行输出。

use std::collections::HashMap;
use std::fmt;

use crate::{collate, mysql};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;

/// MySQL mediumblob 上限，用作默认结果 flen。
const MAX_BLOB_WIDTH: usize = 16_777_215;
/// FORMAT 小数位上限。
const FORMAT_MAX_DECIMALS: i64 = 30;

#[derive(Clone, Debug, PartialEq)]
/// 向量化求值的单元格：Null 建模 chunk 空值，Bytes 承载字符串/二进制。
pub enum Value {
    Null,
    Bytes(Vec<u8>),
    Int(i64),
    Real(f64),
    Decimal(String),
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::Bytes(value.as_bytes().to_vec())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 求值过程中可收集的告警（超包、未知 locale、非法编码等）。
pub enum EvalWarning {
    AllowedPacketOverflow { function: String, limit: u64 },
    UnknownLocale(String),
    InvalidEncoding(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 向量化字符串求值的致命错误。
pub enum EvalError {
    MissingArgument {
        index: usize,
    },
    TypeMismatch {
        index: usize,
        expected: &'static str,
    },
    AllowedPacketOverflow {
        function: String,
        limit: u64,
    },
    UnknownCharset(String),
    Format(String),
}

impl fmt::Display for EvalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingArgument { index } => write!(formatter, "missing argument {index}"),
            Self::TypeMismatch { index, expected } => {
                write!(formatter, "argument {index} must be {expected}")
            }
            Self::AllowedPacketOverflow { function, limit } => {
                write!(formatter, "{function} exceeds max_allowed_packet ({limit})")
            }
            Self::UnknownCharset(charset) => write!(formatter, "unknown charset {charset}"),
            Self::Format(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for EvalError {}

#[derive(Clone, Debug)]
/// 行无关求值配置：max_allowed_packet、结果宽度与截断策略。
pub struct EvalConfig {
    pub max_allowed_packet: u64,
    pub result_flen: usize,
    pub truncate_as_warning: bool,
    pub ignore_truncate_error: bool,
    pub strict_mode: bool,
}

impl Default for EvalConfig {
    fn default() -> Self {
        Self {
            max_allowed_packet: 64 * 1024 * 1024,
            result_flen: MAX_BLOB_WIDTH,
            truncate_as_warning: false,
            ignore_truncate_error: false,
            strict_mode: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
/// 一批行的求值输出：结果列与告警列表。
pub struct EvalOutput {
    pub values: Vec<Value>,
    pub warnings: Vec<EvalWarning>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// TRIM 方向：双侧 / 仅前 / 仅后。
pub enum TrimDirection {
    Both,
    Leading,
    Trailing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 可向量化的字符串内建变体；部分携带 collation 或字符集状态。
pub enum StringBuiltin {
    LowerBinary,
    LowerUtf8,
    Repeat,
    StringIsNull,
    UpperUtf8,
    UpperBinary,
    LeftUtf8,
    RightUtf8,
    Space,
    ReverseUtf8,
    Concat,
    Locate3Utf8 {
        collation: String,
    },
    HexStr,
    LTrim,
    Quote,
    InsertBinary,
    ConcatWs,
    Convert {
        source_charset: String,
        target_charset: String,
    },
    SubstringIndex {
        count_unsigned: bool,
    },
    Unhex,
    ExportSet3,
    Ascii,
    LpadBinary,
    LpadUtf8,
    FindInSet {
        collation: String,
    },
    LeftBinary,
    ReverseBinary,
    RTrim,
    Strcmp {
        collation: String,
    },
    Locate2Binary,
    Locate3Binary,
    ExportSet4,
    RpadBinary,
    FormatWithLocale,
    Substring2Binary,
    Substring2Utf8,
    Trim2,
    InstrUtf8 {
        collation: String,
    },
    OctString,
    Elt,
    InsertUtf8,
    ExportSet5,
    Substring3Utf8,
    Trim3,
    Ord,
    InstrBinary,
    Length,
    Locate2Utf8 {
        collation: String,
    },
    BitLength,
    Char {
        charset: String,
    },
    Replace,
    MakeSet,
    OctInt,
    ToBase64,
    Trim1,
    RpadUtf8,
    CharLengthBinary,
    Bin,
    Format,
    RightBinary,
    Substring3Binary,
    HexInt,
    FromBase64,
    CharLengthUtf8,
    TranslateBinary,
    TranslateUtf8,
}

/// 对多行输入逐行调用内核，组装 `EvalOutput`。
/// 每行恰好产生一个输出值。
pub fn eval_rows(
    builtin: &StringBuiltin,
    rows: &[Vec<Value>],
    config: &EvalConfig,
) -> Result<EvalOutput, EvalError> {
    let mut warnings = Vec::new();
    let mut values = Vec::with_capacity(rows.len());
    for row in rows {
        values.push(eval_row(builtin, row, config, &mut warnings)?);
    }
    Ok(EvalOutput { values, warnings })
}

/// 单行分派：按 `StringBuiltin` 变体选择具体实现。
fn eval_row(
    builtin: &StringBuiltin,
    row: &[Value],
    config: &EvalConfig,
    warnings: &mut Vec<EvalWarning>,
) -> Result<Value, EvalError> {
    use StringBuiltin::*;
    match builtin {
        // binary 大小写恒等：直接透传字节；UTF-8 路径做 Unicode 大小写转换。
        LowerBinary | UpperBinary => clone_nullable_bytes(row, 0),
        LowerUtf8 => map_utf8(row, 0, go_simple_lowercase),
        UpperUtf8 => map_utf8(row, 0, go_simple_uppercase),
        Repeat => repeat(row, config, warnings),
        StringIsNull => Ok(Value::Int(i64::from(matches!(arg(row, 0)?, Value::Null)))),
        LeftUtf8 => left_utf8(row),
        RightUtf8 => right_utf8(row),
        Space => space(row, config, warnings),
        ReverseUtf8 => map_utf8(row, 0, |value| value.chars().rev().collect()),
        Concat => concat(row, config, warnings),
        Locate3Utf8 { collation } => locate3_utf8(row, collate::IsCICollation(collation)),
        HexStr => map_bytes(row, 0, |value| hex::encode_upper(value).into_bytes()),
        LTrim => map_bytes(row, 0, |value| trim_left_spaces(value).to_vec()),
        Quote => quote(row),
        InsertBinary => insert_binary(row, config, warnings),
        ConcatWs => concat_ws(row, config, warnings),
        Convert {
            source_charset,
            target_charset,
        } => convert(row, source_charset, target_charset, warnings),
        SubstringIndex { count_unsigned } => substring_index(row, *count_unsigned),
        Unhex => unhex(row),
        ExportSet3 => export_set_row(row, 3),
        Ascii => ascii(row),
        LpadBinary => pad_binary(row, true, config, warnings),
        LpadUtf8 => pad_utf8(row, true, config, warnings),
        FindInSet { collation } => find_in_set(row, collation),
        LeftBinary => left_binary(row),
        ReverseBinary => map_bytes(row, 0, |value| value.iter().rev().copied().collect()),
        RTrim => map_bytes(row, 0, |value| trim_right_spaces(value).to_vec()),
        Strcmp { collation } => strcmp(row, collation),
        Locate2Binary => locate2_binary(row),
        Locate3Binary => locate3_binary(row),
        ExportSet4 => export_set_row(row, 4),
        RpadBinary => pad_binary(row, false, config, warnings),
        FormatWithLocale => format_value(row, true, warnings),
        Substring2Binary => substring2_binary(row),
        Substring2Utf8 => substring2_utf8(row),
        Trim2 => trim2(row),
        InstrUtf8 { collation } => instr_utf8(row, collate::IsCICollation(collation)),
        OctString => oct_string(row),
        Elt => elt(row),
        InsertUtf8 => insert_utf8(row, config, warnings),
        ExportSet5 => export_set_row(row, 5),
        Substring3Utf8 => substring3_utf8(row),
        Trim3 => trim3(row),
        Ord => ord(row),
        InstrBinary => instr_binary(row),
        // CHAR_LENGTH(binary) 与 LENGTH 同为字节长度。
        Length | CharLengthBinary => length(row),
        Locate2Utf8 { collation } => locate2_utf8(row, collate::IsCICollation(collation)),
        BitLength => bit_length(row),
        Char { charset } => char_value(row, charset, config, warnings),
        Replace => replace(row),
        MakeSet => make_set(row),
        OctInt => radix_int(row, 8, false),
        ToBase64 => to_base64(row, config, warnings),
        Trim1 => map_bytes(row, 0, |value| trim_spaces(value).to_vec()),
        RpadUtf8 => pad_utf8(row, false, config, warnings),
        Bin => radix_int(row, 2, false),
        Format => format_value(row, false, warnings),
        RightBinary => right_binary(row),
        Substring3Binary => substring3_binary(row),
        HexInt => radix_int(row, 16, true),
        FromBase64 => from_base64(row, config, warnings),
        CharLengthUtf8 => char_length_utf8(row),
        TranslateBinary => translate_binary(row),
        TranslateUtf8 => translate_utf8(row),
    }
}

/// 取第 index 个参数；缺失则报错。
fn arg(row: &[Value], index: usize) -> Result<&Value, EvalError> {
    row.get(index).ok_or(EvalError::MissingArgument { index })
}

/// 将参数解释为可选字节切片；类型不符报错。
fn bytes(row: &[Value], index: usize) -> Result<Option<&[u8]>, EvalError> {
    match arg(row, index)? {
        Value::Null => Ok(None),
        Value::Bytes(value) => Ok(Some(value)),
        _ => Err(EvalError::TypeMismatch {
            index,
            expected: "bytes",
        }),
    }
}

/// 将参数解释为可选整数。
fn int(row: &[Value], index: usize) -> Result<Option<i64>, EvalError> {
    match arg(row, index)? {
        Value::Null => Ok(None),
        Value::Int(value) => Ok(Some(*value)),
        _ => Err(EvalError::TypeMismatch {
            index,
            expected: "integer",
        }),
    }
}

/// 透传可空字节参数（如 binary 大小写恒等）。
fn clone_nullable_bytes(row: &[Value], index: usize) -> Result<Value, EvalError> {
    Ok(bytes(row, index)?.map_or(Value::Null, |value| Value::Bytes(value.to_vec())))
}

/// 对非 NULL 字节参数应用变换。
fn map_bytes(
    row: &[Value],
    index: usize,
    map: impl FnOnce(&[u8]) -> Vec<u8>,
) -> Result<Value, EvalError> {
    Ok(bytes(row, index)?.map_or(Value::Null, |value| Value::Bytes(map(value))))
}

/// 对非 NULL 参数按 UTF-8 有损解码后变换。
fn map_utf8(
    row: &[Value],
    index: usize,
    map: impl FnOnce(&str) -> String,
) -> Result<Value, EvalError> {
    Ok(bytes(row, index)?.map_or(Value::Null, |value| {
        Value::Bytes(map(&String::from_utf8_lossy(value)).into_bytes())
    }))
}

/// Apply Go's simple, one-rune Unicode uppercase mapping. Rust's standard
/// conversion uses full mappings and can expand one scalar into several.
fn go_simple_uppercase(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            let mut mapped = character.to_uppercase();
            let first = mapped.next().expect("case mapping is never empty");
            if mapped.next().is_none() {
                first
            } else {
                character
            }
        })
        .collect()
}

/// Apply Go's simple, one-rune Unicode lowercase mapping.
fn go_simple_lowercase(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            character
                .to_lowercase()
                .next()
                .expect("case mapping is never empty")
        })
        .collect()
}

/// 处理超过 max_allowed_packet：告警或错误，结果为 NULL。
fn packet_overflow(
    function: &str,
    size: u128,
    config: &EvalConfig,
    warnings: &mut Vec<EvalWarning>,
) -> Result<bool, EvalError> {
    if size <= u128::from(config.max_allowed_packet) {
        return Ok(false);
    }
    if config.truncate_as_warning || config.ignore_truncate_error {
        warnings.push(EvalWarning::AllowedPacketOverflow {
            function: function.to_owned(),
            limit: config.max_allowed_packet,
        });
        Ok(true)
    } else {
        Err(EvalError::AllowedPacketOverflow {
            function: function.to_owned(),
            limit: config.max_allowed_packet,
        })
    }
}

/// REPEAT 向量内核。
fn repeat(
    row: &[Value],
    config: &EvalConfig,
    warnings: &mut Vec<EvalWarning>,
) -> Result<Value, EvalError> {
    let (Some(value), Some(mut count)) = (bytes(row, 0)?, int(row, 1)?) else {
        return Ok(Value::Null);
    };
    if count < 1 {
        return Ok(Value::Bytes(Vec::new()));
    }
    count = count.min(i64::from(i32::MAX));
    let size = value.len() as u128 * count as u128;
    if packet_overflow("repeat", size, config, warnings)?
        || value.len() as u128 > config.result_flen as u128 / count as u128
    {
        return Ok(Value::Null);
    }
    Ok(Value::Bytes(value.repeat(count as usize)))
}

/// LEFT UTF-8 向量内核。
fn left_utf8(row: &[Value]) -> Result<Value, EvalError> {
    let (Some(value), Some(count)) = (bytes(row, 0)?, int(row, 1)?) else {
        return Ok(Value::Null);
    };
    let chars: Vec<char> = String::from_utf8_lossy(value).chars().collect();
    let end = count.max(0).min(chars.len() as i64) as usize;
    Ok(Value::Bytes(
        chars[..end].iter().collect::<String>().into_bytes(),
    ))
}

/// RIGHT UTF-8 向量内核。
fn right_utf8(row: &[Value]) -> Result<Value, EvalError> {
    let (Some(value), Some(count)) = (bytes(row, 0)?, int(row, 1)?) else {
        return Ok(Value::Null);
    };
    let chars: Vec<char> = String::from_utf8_lossy(value).chars().collect();
    let count = count.max(0).min(chars.len() as i64) as usize;
    Ok(Value::Bytes(
        chars[chars.len() - count..]
            .iter()
            .collect::<String>()
            .into_bytes(),
    ))
}

/// SPACE 向量内核。
fn space(
    row: &[Value],
    config: &EvalConfig,
    warnings: &mut Vec<EvalWarning>,
) -> Result<Value, EvalError> {
    let Some(count) = int(row, 0)? else {
        return Ok(Value::Null);
    };
    let count = count.max(0) as u128;
    if packet_overflow("space", count, config, warnings)? || count > MAX_BLOB_WIDTH as u128 {
        return Ok(Value::Null);
    }
    Ok(Value::Bytes(vec![b' '; count as usize]))
}

/// CONCAT 向量内核；任一 NULL 则结果 NULL。
fn concat(
    row: &[Value],
    config: &EvalConfig,
    warnings: &mut Vec<EvalWarning>,
) -> Result<Value, EvalError> {
    let mut output = Vec::new();
    for index in 0..row.len() {
        let Some(value) = bytes(row, index)? else {
            return Ok(Value::Null);
        };
        if packet_overflow(
            "concat",
            output.len() as u128 + value.len() as u128,
            config,
            warnings,
        )? {
            return Ok(Value::Null);
        }
        output.extend_from_slice(value);
    }
    Ok(Value::Bytes(output))
}

/// LOCATE 三参数 UTF-8 路径。
fn locate3_utf8(row: &[Value], case_insensitive: bool) -> Result<Value, EvalError> {
    let (Some(needle), Some(haystack), Some(position)) =
        (bytes(row, 0)?, bytes(row, 1)?, int(row, 2)?)
    else {
        return Ok(Value::Null);
    };
    let mut needle = String::from_utf8_lossy(needle).into_owned();
    let mut haystack = String::from_utf8_lossy(haystack).into_owned();
    let haystack_chars: Vec<char> = haystack.chars().collect();
    let needle_len = needle.chars().count();
    let position = position - 1;
    if position < 0 || position as usize > haystack_chars.len().saturating_sub(needle_len) {
        return Ok(Value::Int(0));
    }
    if needle_len == 0 {
        return Ok(Value::Int(position + 1));
    }
    haystack = haystack_chars[position as usize..].iter().collect();
    if case_insensitive {
        needle = needle.to_lowercase();
        haystack = haystack.to_lowercase();
    }
    Ok(Value::Int(haystack.find(&needle).map_or(0, |offset| {
        position + haystack[..offset].chars().count() as i64 + 1
    })))
}

/// QUOTE 向量内核。
fn quote(row: &[Value]) -> Result<Value, EvalError> {
    let Some(value) = bytes(row, 0)? else {
        return Ok(Value::from("NULL"));
    };
    let mut output = String::from("'");
    for character in String::from_utf8_lossy(value).chars() {
        match character {
            '\\' | '\'' => {
                output.push('\\');
                output.push(character);
            }
            '\0' => output.push_str("\\0"),
            '\u{1a}' => output.push_str("\\Z"),
            _ => output.push(character),
        }
    }
    output.push('\'');
    Ok(Value::Bytes(output.into_bytes()))
}

/// INSERT 二进制向量内核。
fn insert_binary(
    row: &[Value],
    config: &EvalConfig,
    warnings: &mut Vec<EvalWarning>,
) -> Result<Value, EvalError> {
    let (Some(value), Some(position), Some(length), Some(replacement)) =
        (bytes(row, 0)?, int(row, 1)?, int(row, 2)?, bytes(row, 3)?)
    else {
        return Ok(Value::Null);
    };
    if position < 1 || position as usize > value.len() {
        return Ok(Value::Bytes(value.to_vec()));
    }
    let start = position as usize - 1;
    let length = if length < 0 {
        value.len() - start
    } else {
        (length as usize).min(value.len() - start)
    };
    let size = value.len() - length + replacement.len();
    if packet_overflow("insert", size as u128, config, warnings)? {
        return Ok(Value::Null);
    }
    let mut output = Vec::with_capacity(size);
    output.extend_from_slice(&value[..start]);
    output.extend_from_slice(replacement);
    output.extend_from_slice(&value[start + length..]);
    Ok(Value::Bytes(output))
}

/// CONCAT_WS 向量内核；分隔符 NULL 则整行 NULL。
fn concat_ws(
    row: &[Value],
    config: &EvalConfig,
    warnings: &mut Vec<EvalWarning>,
) -> Result<Value, EvalError> {
    let Some(separator) = bytes(row, 0)? else {
        return Ok(Value::Null);
    };
    let mut parts = Vec::new();
    let mut size = 0_u128;
    for index in 1..row.len() {
        let Some(value) = bytes(row, index)? else {
            continue;
        };
        size += value.len() as u128;
        if !parts.is_empty() {
            size += separator.len() as u128;
        }
        if packet_overflow("concat_ws", size, config, warnings)? {
            return Ok(Value::Null);
        }
        parts.push(value);
    }
    let mut output = Vec::with_capacity(size as usize);
    for (index, part) in parts.iter().enumerate() {
        if index != 0 {
            output.extend_from_slice(separator);
        }
        output.extend_from_slice(part);
    }
    Ok(Value::Bytes(output))
}

/// 解析字符集标签。
fn encoding(label: &str) -> Result<&'static encoding_rs::Encoding, EvalError> {
    let lowercase = label.to_ascii_lowercase();
    let normalized = match lowercase.as_str() {
        "utf8" | "utf8mb4" => "utf-8",
        "ascii" => "us-ascii",
        other => other,
    };
    encoding_rs::Encoding::for_label(normalized.as_bytes())
        .ok_or_else(|| EvalError::UnknownCharset(label.to_owned()))
}

/// CONVERT ... USING 向量内核。
fn convert(
    row: &[Value],
    source: &str,
    target: &str,
    warnings: &mut Vec<EvalWarning>,
) -> Result<Value, EvalError> {
    let Some(value) = bytes(row, 0)? else {
        return Ok(Value::Null);
    };
    if target.eq_ignore_ascii_case("binary") {
        return Ok(Value::Bytes(value.to_vec()));
    }
    let target_encoding = encoding(target)?;
    let decoded = if source.eq_ignore_ascii_case("binary") {
        let (decoded, _, had_errors) = target_encoding.decode(value);
        if had_errors {
            warnings.push(EvalWarning::InvalidEncoding(target.to_owned()));
        }
        decoded
    } else {
        let source_encoding = encoding(source)?;
        let (decoded, _, had_errors) = source_encoding.decode(value);
        if had_errors {
            warnings.push(EvalWarning::InvalidEncoding(source.to_owned()));
        }
        decoded
    };
    let (encoded, _, had_errors) = target_encoding.encode(&decoded);
    if had_errors {
        warnings.push(EvalWarning::InvalidEncoding(target.to_owned()));
    }
    Ok(Value::Bytes(encoded.into_owned()))
}

/// SUBSTRING_INDEX 向量内核。
fn substring_index(row: &[Value], count_unsigned: bool) -> Result<Value, EvalError> {
    let (Some(value), Some(delimiter), Some(mut count)) =
        (bytes(row, 0)?, bytes(row, 1)?, int(row, 2)?)
    else {
        return Ok(Value::Null);
    };
    if delimiter.is_empty() {
        return Ok(Value::Bytes(Vec::new()));
    }
    if count < 0 && count_unsigned {
        return Ok(Value::Bytes(value.to_vec()));
    }
    let parts: Vec<&[u8]> = value
        .split(|byte| delimiter.len() == 1 && *byte == delimiter[0])
        .collect();
    // `slice::split` only handles one-byte delimiters; preserve arbitrary binary delimiters below.
    let parts = if delimiter.len() == 1 {
        parts
    } else {
        split_bytes(value, delimiter)
    };
    let end = parts.len() as i64;
    let (start, stop) = if count > 0 {
        (0, count.min(end))
    } else {
        count = count.wrapping_neg();
        if count < 0 {
            return Ok(Value::Bytes(value.to_vec()));
        }
        ((end - count).max(0), end)
    };
    Ok(Value::Bytes(join_bytes(
        &parts[start as usize..stop as usize],
        delimiter,
    )))
}

/// 按分隔符切分字节切片。
fn split_bytes<'a>(value: &'a [u8], delimiter: &[u8]) -> Vec<&'a [u8]> {
    let mut output = Vec::new();
    let mut start = 0;
    while let Some(offset) = find_bytes(&value[start..], delimiter) {
        output.push(&value[start..start + offset]);
        start += offset + delimiter.len();
    }
    output.push(&value[start..]);
    output
}

/// 用分隔符拼接字节片段。
fn join_bytes(parts: &[&[u8]], delimiter: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if index != 0 {
            output.extend_from_slice(delimiter);
        }
        output.extend_from_slice(part);
    }
    output
}

/// UNHEX 向量内核。
fn unhex(row: &[Value]) -> Result<Value, EvalError> {
    let Some(value) = bytes(row, 0)? else {
        return Ok(Value::Null);
    };
    let mut input = value.to_vec();
    if input.len() % 2 != 0 {
        input.insert(0, b'0');
    }
    Ok(hex::decode(input).map_or(Value::Null, Value::Bytes))
}

/// EXPORT_SET 按实际参数个数解释可选参数。
fn export_set_row(row: &[Value], arity: usize) -> Result<Value, EvalError> {
    let (Some(bits), Some(on), Some(off)) = (int(row, 0)?, bytes(row, 1)?, bytes(row, 2)?) else {
        return Ok(Value::Null);
    };
    let separator = if arity >= 4 {
        let Some(value) = bytes(row, 3)? else {
            return Ok(Value::Null);
        };
        value
    } else {
        b","
    };
    let count = if arity == 5 {
        let Some(value) = int(row, 4)? else {
            return Ok(Value::Null);
        };
        if !(0..=64).contains(&value) {
            64
        } else {
            value as usize
        }
    } else {
        64
    };
    let bits = bits as u64;
    let mut parts = Vec::with_capacity(count);
    for index in 0..count {
        parts.push(if bits & (1_u64 << index) != 0 {
            on
        } else {
            off
        });
    }
    Ok(Value::Bytes(join_bytes(&parts, separator)))
}

/// ASCII 向量内核。
fn ascii(row: &[Value]) -> Result<Value, EvalError> {
    Ok(bytes(row, 0)?.map_or(Value::Null, |value| {
        Value::Int(value.first().copied().unwrap_or(0) as i64)
    }))
}

/// LPAD/RPAD 二进制路径。
fn pad_binary(
    row: &[Value],
    left: bool,
    config: &EvalConfig,
    warnings: &mut Vec<EvalWarning>,
) -> Result<Value, EvalError> {
    let (Some(value), Some(target), Some(pad)) = (bytes(row, 0)?, int(row, 1)?, bytes(row, 2)?)
    else {
        return Ok(Value::Null);
    };
    if packet_overflow(
        if left { "lpad" } else { "rpad" },
        target as u64 as u128,
        config,
        warnings,
    )? {
        return Ok(Value::Null);
    }
    if target < 0 || target as usize > config.result_flen {
        return Ok(Value::Null);
    }
    let target = target as usize;
    if value.len() < target && pad.is_empty() {
        return Ok(Value::Bytes(Vec::new()));
    }
    if value.len() >= target {
        return Ok(Value::Bytes(value[..target].to_vec()));
    }
    let missing = target - value.len();
    let repeated = repeat_prefix(pad, missing);
    let mut output = Vec::with_capacity(target);
    if left {
        output.extend_from_slice(&repeated);
        output.extend_from_slice(value);
    } else {
        output.extend_from_slice(value);
        output.extend_from_slice(&repeated);
    }
    Ok(Value::Bytes(output))
}

/// LPAD/RPAD UTF-8 路径。
fn pad_utf8(
    row: &[Value],
    left: bool,
    config: &EvalConfig,
    warnings: &mut Vec<EvalWarning>,
) -> Result<Value, EvalError> {
    let (Some(value), Some(target), Some(pad)) = (bytes(row, 0)?, int(row, 1)?, bytes(row, 2)?)
    else {
        return Ok(Value::Null);
    };
    if packet_overflow(
        if left { "lpad" } else { "rpad" },
        (target as u64 as u128) * 4,
        config,
        warnings,
    )? {
        return Ok(Value::Null);
    }
    if target < 0
        || target as usize > MAX_BLOB_WIDTH
        || (target as u128) * 4 > config.result_flen as u128
    {
        return Ok(Value::Null);
    }
    let value: Vec<char> = String::from_utf8_lossy(value).chars().collect();
    let pad: Vec<char> = String::from_utf8_lossy(pad).chars().collect();
    let target = target as usize;
    if value.len() < target && pad.is_empty() {
        return Ok(Value::Bytes(Vec::new()));
    }
    let output: String = if value.len() >= target {
        value[..target].iter().collect()
    } else {
        let missing = target - value.len();
        let fill = pad.iter().cycle().take(missing).copied();
        if left {
            fill.chain(value).collect()
        } else {
            value.into_iter().chain(fill).collect()
        }
    };
    Ok(Value::Bytes(output.into_bytes()))
}

/// 循环复制填充串直至达到目标长度。
fn repeat_prefix(value: &[u8], length: usize) -> Vec<u8> {
    value.iter().cycle().take(length).copied().collect()
}

/// FIND_IN_SET 向量内核。
fn find_in_set(row: &[Value], collation: &str) -> Result<Value, EvalError> {
    let (Some(needle), Some(list)) = (bytes(row, 0)?, bytes(row, 1)?) else {
        return Ok(Value::Null);
    };
    if list.is_empty() {
        return Ok(Value::Int(0));
    }
    let collator = collate::GetCollator(collation);
    let needle = String::from_utf8_lossy(needle);
    let needle_key = collator.KeyWithoutTrimRightSpace(&needle);
    for (index, candidate) in list.split(|byte| *byte == b',').enumerate() {
        let candidate = String::from_utf8_lossy(candidate);
        if needle_key == collator.KeyWithoutTrimRightSpace(&candidate) {
            return Ok(Value::Int(index as i64 + 1));
        }
    }
    Ok(Value::Int(0))
}

/// LEFT 二进制路径。
fn left_binary(row: &[Value]) -> Result<Value, EvalError> {
    let (Some(value), Some(count)) = (bytes(row, 0)?, int(row, 1)?) else {
        return Ok(Value::Null);
    };
    let end = count.max(0).min(value.len() as i64) as usize;
    Ok(Value::Bytes(value[..end].to_vec()))
}

/// RIGHT 二进制路径。
fn right_binary(row: &[Value]) -> Result<Value, EvalError> {
    let (Some(value), Some(count)) = (bytes(row, 0)?, int(row, 1)?) else {
        return Ok(Value::Null);
    };
    let count = count.max(0).min(value.len() as i64) as usize;
    Ok(Value::Bytes(value[value.len() - count..].to_vec()))
}

/// 剥离左侧 ASCII 空白。
fn trim_left_spaces(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(|byte| *byte == b' ') {
        value = &value[1..];
    }
    value
}

/// 剥离右侧 ASCII 空白。
fn trim_right_spaces(mut value: &[u8]) -> &[u8] {
    while value.last().is_some_and(|byte| *byte == b' ') {
        value = &value[..value.len() - 1];
    }
    value
}

/// 双侧剥离 ASCII 空白。
fn trim_spaces(value: &[u8]) -> &[u8] {
    trim_right_spaces(trim_left_spaces(value))
}

/// STRCMP 向量内核。
fn strcmp(row: &[Value], collation: &str) -> Result<Value, EvalError> {
    let (Some(left), Some(right)) = (bytes(row, 0)?, bytes(row, 1)?) else {
        return Ok(Value::Null);
    };
    let collator = collate::GetCollator(collation);
    Ok(Value::Int(i64::from(collator.Compare(
        &String::from_utf8_lossy(left),
        &String::from_utf8_lossy(right),
    ))))
}

/// LOCATE 两参数二进制路径。
fn locate2_binary(row: &[Value]) -> Result<Value, EvalError> {
    let (Some(needle), Some(haystack)) = (bytes(row, 0)?, bytes(row, 1)?) else {
        return Ok(Value::Null);
    };
    Ok(Value::Int(if needle.is_empty() {
        1
    } else {
        find_bytes(haystack, needle).map_or(0, |index| index as i64 + 1)
    }))
}

/// LOCATE 三参数二进制路径。
fn locate3_binary(row: &[Value]) -> Result<Value, EvalError> {
    let (Some(needle), Some(haystack), Some(position)) =
        (bytes(row, 0)?, bytes(row, 1)?, int(row, 2)?)
    else {
        return Ok(Value::Null);
    };
    let position = position - 1;
    if position < 0 || position as usize > haystack.len().saturating_sub(needle.len()) {
        return Ok(Value::Int(0));
    }
    if needle.is_empty() {
        return Ok(Value::Int(position + 1));
    }
    Ok(Value::Int(
        find_bytes(&haystack[position as usize..], needle)
            .map_or(0, |index| position + index as i64 + 1),
    ))
}

/// FORMAT / FORMAT_WITH_LOCALE 向量内核。
fn format_value(
    row: &[Value],
    with_locale: bool,
    warnings: &mut Vec<EvalWarning>,
) -> Result<Value, EvalError> {
    let Some(decimals) = int(row, 1)? else {
        return Ok(Value::Null);
    };
    let decimals = decimals.clamp(0, FORMAT_MAX_DECIMALS) as usize;
    let number = match arg(row, 0)? {
        Value::Null => return Ok(Value::Null),
        Value::Real(value) => format!("{value:.decimals$}"),
        Value::Decimal(value) => round_decimal(value, decimals),
        _ => {
            return Err(EvalError::TypeMismatch {
                index: 0,
                expected: "real or decimal",
            });
        }
    };
    let locale = if with_locale {
        match bytes(row, 2)? {
            Some(value) => String::from_utf8_lossy(value).into_owned(),
            None => {
                warnings.push(EvalWarning::UnknownLocale("NULL".into()));
                "en_US".into()
            }
        }
    } else {
        "en_US".into()
    };
    let (formatted, found, error) =
        mysql::locale_format::FormatByLocale(&number, &decimals.to_string(), &locale);
    error.map_err(|error| EvalError::Format(error.to_string()))?;
    if !found && !(with_locale && matches!(arg(row, 2)?, Value::Null)) {
        warnings.push(EvalWarning::UnknownLocale(locale));
    }
    Ok(Value::Bytes(formatted.into_bytes()))
}

/// FORMAT 用十进制字符串舍入。
fn round_decimal(value: &str, decimals: usize) -> String {
    let (sign, unsigned) = value
        .strip_prefix('-')
        .map_or(("", value), |value| ("-", value));
    let (integer, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let mut digits = format!("{integer}{fraction:0<decimals$}");
    digits.truncate(integer.len() + decimals);
    if fraction
        .as_bytes()
        .get(decimals)
        .is_some_and(|digit| *digit >= b'5')
    {
        let mut carry = true;
        let mut bytes = digits.into_bytes();
        for digit in bytes.iter_mut().rev() {
            if !carry {
                break;
            }
            if *digit == b'9' {
                *digit = b'0';
            } else {
                *digit += 1;
                carry = false;
            }
        }
        if carry {
            bytes.insert(0, b'1');
        }
        digits = String::from_utf8(bytes).expect("decimal digits are ASCII");
    }
    if decimals == 0 {
        format!("{sign}{digits}")
    } else {
        let split = digits.len() - decimals;
        format!("{sign}{}.{}", &digits[..split], &digits[split..])
    }
}

/// 将一基 position 转为字节/字符起始下标。
fn substring_start(length: usize, position: i64) -> usize {
    let length = length as i64;
    let position = if position < 0 {
        position.wrapping_add(length)
    } else {
        position - 1
    };
    if position < 0 || position > length {
        length as usize
    } else {
        position as usize
    }
}

/// SUBSTRING 两参数二进制路径。
fn substring2_binary(row: &[Value]) -> Result<Value, EvalError> {
    let (Some(value), Some(position)) = (bytes(row, 0)?, int(row, 1)?) else {
        return Ok(Value::Null);
    };
    Ok(Value::Bytes(
        value[substring_start(value.len(), position)..].to_vec(),
    ))
}

/// SUBSTRING 两参数 UTF-8 路径。
fn substring2_utf8(row: &[Value]) -> Result<Value, EvalError> {
    let (Some(value), Some(position)) = (bytes(row, 0)?, int(row, 1)?) else {
        return Ok(Value::Null);
    };
    let chars: Vec<char> = String::from_utf8_lossy(value).chars().collect();
    Ok(Value::Bytes(
        chars[substring_start(chars.len(), position)..]
            .iter()
            .collect::<String>()
            .into_bytes(),
    ))
}

/// TRIM 两参数形式。
fn trim2(row: &[Value]) -> Result<Value, EvalError> {
    let (Some(value), Some(remove)) = (bytes(row, 0)?, bytes(row, 1)?) else {
        return Ok(Value::Null);
    };
    Ok(Value::Bytes(
        trim_pattern(trim_pattern(value, remove, true), remove, false).to_vec(),
    ))
}

/// 按指定模式从左或右反复剥离。
fn trim_pattern<'a>(mut value: &'a [u8], remove: &[u8], leading: bool) -> &'a [u8] {
    if remove.is_empty() {
        return value;
    }
    if leading {
        while value.starts_with(remove) {
            value = &value[remove.len()..];
        }
    } else {
        while value.ends_with(remove) {
            value = &value[..value.len() - remove.len()];
        }
    }
    value
}

/// INSTR UTF-8 路径。
fn instr_utf8(row: &[Value], case_insensitive: bool) -> Result<Value, EvalError> {
    let (Some(haystack), Some(needle)) = (bytes(row, 0)?, bytes(row, 1)?) else {
        return Ok(Value::Null);
    };
    let mut haystack = String::from_utf8_lossy(haystack).into_owned();
    let mut needle = String::from_utf8_lossy(needle).into_owned();
    if case_insensitive {
        haystack = haystack.to_lowercase();
        needle = needle.to_lowercase();
    }
    Ok(Value::Int(haystack.find(&needle).map_or(0, |index| {
        haystack[..index].chars().count() as i64 + 1
    })))
}

/// 截取可作为十进制前缀的合法数字部分。
fn valid_decimal_prefix(value: &str) -> &str {
    let bytes = value.as_bytes();
    let mut end = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if index == 0 && matches!(byte, b'+' | b'-') {
            continue;
        }
        if !byte.is_ascii_digit() {
            break;
        }
        end = index + 1;
    }
    if end > 1 && value.starts_with('+') {
        &value[1..end]
    } else {
        &value[..end]
    }
}

/// OCT 字符串参数路径。
fn oct_string(row: &[Value]) -> Result<Value, EvalError> {
    let Some(value) = bytes(row, 0)? else {
        return Ok(Value::Null);
    };
    if value.is_empty() {
        return Ok(Value::Null);
    }
    let text = String::from_utf8_lossy(value);
    let prefix = valid_decimal_prefix(text.trim());
    if prefix.is_empty() {
        return Ok(Value::from("0"));
    }
    let (negative, digits) = prefix
        .strip_prefix('-')
        .map_or((false, prefix), |value| (true, value));
    let mut number = digits.parse::<u64>().unwrap_or(u64::MAX);
    if negative && number != u64::MAX {
        number = number.wrapping_neg();
    }
    Ok(Value::Bytes(format!("{number:o}").into_bytes()))
}

/// ELT 向量内核。
fn elt(row: &[Value]) -> Result<Value, EvalError> {
    let Some(index) = int(row, 0)? else {
        return Ok(Value::Null);
    };
    if index < 1 || index as usize >= row.len() {
        return Ok(Value::Null);
    }
    clone_nullable_bytes(row, index as usize)
}

/// INSERT UTF-8 向量内核。
fn insert_utf8(
    row: &[Value],
    config: &EvalConfig,
    warnings: &mut Vec<EvalWarning>,
) -> Result<Value, EvalError> {
    let (Some(value), Some(position), Some(length), Some(replacement)) =
        (bytes(row, 0)?, int(row, 1)?, int(row, 2)?, bytes(row, 3)?)
    else {
        return Ok(Value::Null);
    };
    let chars: Vec<char> = String::from_utf8_lossy(value).chars().collect();
    if position < 1 || position as usize > chars.len() {
        return Ok(Value::Bytes(value.to_vec()));
    }
    let start = position as usize - 1;
    let length = if length < 0 {
        chars.len() - start
    } else {
        (length as usize).min(chars.len() - start)
    };
    let head: String = chars[..start].iter().collect();
    let tail: String = chars[start + length..].iter().collect();
    let size = head.len() + replacement.len() + tail.len();
    if packet_overflow("insert", size as u128, config, warnings)? {
        return Ok(Value::Null);
    }
    let mut output = head.into_bytes();
    output.extend_from_slice(replacement);
    output.extend_from_slice(tail.as_bytes());
    Ok(Value::Bytes(output))
}

/// SUBSTRING 三参数 UTF-8 路径。
fn substring3_utf8(row: &[Value]) -> Result<Value, EvalError> {
    let (Some(value), Some(position), Some(length)) = (bytes(row, 0)?, int(row, 1)?, int(row, 2)?)
    else {
        return Ok(Value::Null);
    };
    let chars: Vec<char> = String::from_utf8_lossy(value).chars().collect();
    let start = substring_start(chars.len(), position);
    let end = (start as i64).wrapping_add(length);
    if end < start as i64 {
        return Ok(Value::Bytes(Vec::new()));
    }
    let end = (end as usize).min(chars.len());
    Ok(Value::Bytes(
        chars[start..end].iter().collect::<String>().into_bytes(),
    ))
}

/// SUBSTRING 三参数二进制路径。
fn substring3_binary(row: &[Value]) -> Result<Value, EvalError> {
    let (Some(value), Some(position), Some(length)) = (bytes(row, 0)?, int(row, 1)?, int(row, 2)?)
    else {
        return Ok(Value::Null);
    };
    let start = substring_start(value.len(), position);
    let end = (start as i64).wrapping_add(length);
    if end < start as i64 {
        return Ok(Value::Bytes(Vec::new()));
    }
    Ok(Value::Bytes(
        value[start..(end as usize).min(value.len())].to_vec(),
    ))
}

/// TRIM 三参数形式（含方向）。
fn trim3(row: &[Value]) -> Result<Value, EvalError> {
    let (Some(value), Some(remove), Some(direction)) =
        (bytes(row, 0)?, bytes(row, 1)?, int(row, 2)?)
    else {
        return Ok(Value::Null);
    };
    let output = match direction {
        2 => trim_pattern(value, remove, true),
        3 => trim_pattern(value, remove, false),
        _ => trim_pattern(trim_pattern(value, remove, true), remove, false),
    };
    Ok(Value::Bytes(output.to_vec()))
}

/// ORD 向量内核。
fn ord(row: &[Value]) -> Result<Value, EvalError> {
    let Some(value) = bytes(row, 0)? else {
        return Ok(Value::Null);
    };
    let width = (1..=value.len().min(4))
        .find(|width| std::str::from_utf8(&value[..*width]).is_ok())
        .unwrap_or_else(|| usize::from(!value.is_empty()));
    let number = value[..width]
        .iter()
        .fold(0_i64, |number, byte| (number << 8) | i64::from(*byte));
    Ok(Value::Int(number))
}

/// INSTR 二进制路径。
fn instr_binary(row: &[Value]) -> Result<Value, EvalError> {
    let (Some(haystack), Some(needle)) = (bytes(row, 0)?, bytes(row, 1)?) else {
        return Ok(Value::Null);
    };
    Ok(Value::Int(
        find_bytes(haystack, needle).map_or(0, |index| index as i64 + 1),
    ))
}

/// LENGTH / CHAR_LENGTH(binary) 向量内核。
fn length(row: &[Value]) -> Result<Value, EvalError> {
    Ok(bytes(row, 0)?.map_or(Value::Null, |value| Value::Int(value.len() as i64)))
}

/// LOCATE 两参数 UTF-8 路径。
fn locate2_utf8(row: &[Value], case_insensitive: bool) -> Result<Value, EvalError> {
    let (Some(needle), Some(haystack)) = (bytes(row, 0)?, bytes(row, 1)?) else {
        return Ok(Value::Null);
    };
    let mut needle = String::from_utf8_lossy(needle).into_owned();
    let mut haystack = String::from_utf8_lossy(haystack).into_owned();
    if needle.is_empty() {
        return Ok(Value::Int(1));
    }
    if case_insensitive {
        needle = needle.to_lowercase();
        haystack = haystack.to_lowercase();
    }
    Ok(Value::Int(haystack.find(&needle).map_or(0, |index| {
        haystack[..index].chars().count() as i64 + 1
    })))
}

/// BIT_LENGTH 向量内核。
fn bit_length(row: &[Value]) -> Result<Value, EvalError> {
    Ok(bytes(row, 0)?.map_or(Value::Null, |value| Value::Int((value.len() * 8) as i64)))
}

/// CHAR(...) 向量内核。
fn char_value(
    row: &[Value],
    charset: &str,
    config: &EvalConfig,
    warnings: &mut Vec<EvalWarning>,
) -> Result<Value, EvalError> {
    let mut raw = Vec::new();
    for index in 0..row.len() {
        let Some(number) = int(row, index)? else {
            continue;
        };
        let mut current = number;
        let mut bytes = Vec::new();
        for _ in 0..4 {
            bytes.push((current & 0xff) as u8);
            current >>= 8;
            if current == 0 {
                break;
            }
        }
        bytes.reverse();
        raw.extend(bytes);
    }
    if charset.eq_ignore_ascii_case("binary") {
        return Ok(Value::Bytes(raw));
    }
    let encoding = encoding(charset)?;
    let (decoded, _, had_errors) = encoding.decode(&raw);
    if had_errors {
        warnings.push(EvalWarning::InvalidEncoding(charset.to_owned()));
        if config.strict_mode {
            return Ok(Value::Null);
        }
    }
    Ok(Value::Bytes(decoded.into_owned().into_bytes()))
}

/// REPLACE 向量内核。
fn replace(row: &[Value]) -> Result<Value, EvalError> {
    let (Some(value), Some(old), Some(new)) = (bytes(row, 0)?, bytes(row, 1)?, bytes(row, 2)?)
    else {
        return Ok(Value::Null);
    };
    if old.is_empty() {
        return Ok(Value::Bytes(value.to_vec()));
    }
    let mut output = Vec::new();
    let mut start = 0;
    while let Some(index) = find_bytes(&value[start..], old) {
        output.extend_from_slice(&value[start..start + index]);
        output.extend_from_slice(new);
        start += index + old.len();
    }
    output.extend_from_slice(&value[start..]);
    Ok(Value::Bytes(output))
}

/// MAKE_SET 向量内核。
fn make_set(row: &[Value]) -> Result<Value, EvalError> {
    let Some(bits) = int(row, 0)? else {
        return Ok(Value::Null);
    };
    let mut values = Vec::new();
    for index in 1..row.len() {
        if index - 1 >= 64 || bits as u64 & (1_u64 << (index - 1)) == 0 {
            continue;
        }
        if let Some(value) = bytes(row, index)? {
            values.push(value);
        }
    }
    Ok(Value::Bytes(join_bytes(&values, b",")))
}

/// BIN/OCT/HEX(整数) 共用进制转换。
fn radix_int(row: &[Value], radix: u32, uppercase: bool) -> Result<Value, EvalError> {
    let Some(value) = int(row, 0)? else {
        return Ok(Value::Null);
    };
    let mut output = match radix {
        2 => format!("{:b}", value as u64),
        8 => format!("{:o}", value as u64),
        16 => format!("{:x}", value as u64),
        _ => unreachable!(),
    };
    if uppercase {
        output.make_ascii_uppercase();
    }
    Ok(Value::Bytes(output.into_bytes()))
}

/// TO_BASE64 向量内核。
fn to_base64(
    row: &[Value],
    config: &EvalConfig,
    warnings: &mut Vec<EvalWarning>,
) -> Result<Value, EvalError> {
    let Some(value) = bytes(row, 0)? else {
        return Ok(Value::Null);
    };
    let encoded_length = ((value.len() as u128 + 2) / 3) * 4;
    let total = encoded_length + encoded_length.saturating_sub(1) / 76;
    if packet_overflow("to_base64", total, config, warnings)? {
        return Ok(Value::Null);
    }
    let encoded = BASE64_STANDARD.encode(value);
    let output = encoded
        .as_bytes()
        .chunks(76)
        .map(String::from_utf8_lossy)
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Value::Bytes(output.into_bytes()))
}

/// FROM_BASE64 向量内核。
fn from_base64(
    row: &[Value],
    config: &EvalConfig,
    warnings: &mut Vec<EvalWarning>,
) -> Result<Value, EvalError> {
    let Some(value) = bytes(row, 0)? else {
        return Ok(Value::Null);
    };
    let needed = value.len() as u128 * 3 / 4;
    if packet_overflow("from_base64", needed, config, warnings)? {
        return Ok(Value::Null);
    }
    let compact: Vec<u8> = value
        .iter()
        .copied()
        .filter(|byte| !matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        .collect();
    Ok(BASE64_STANDARD
        .decode(compact)
        .map_or(Value::Null, Value::Bytes))
}

/// CHAR_LENGTH UTF-8 向量内核。
fn char_length_utf8(row: &[Value]) -> Result<Value, EvalError> {
    Ok(bytes(row, 0)?.map_or(Value::Null, |value| {
        Value::Int(String::from_utf8_lossy(value).chars().count() as i64)
    }))
}

/// TRANSLATE 二进制路径。
fn translate_binary(row: &[Value]) -> Result<Value, EvalError> {
    let (Some(source), Some(from), Some(to)) = (bytes(row, 0)?, bytes(row, 1)?, bytes(row, 2)?)
    else {
        return Ok(Value::Null);
    };
    let mut map: HashMap<u8, Option<u8>> = HashMap::new();
    for index in (0..from.len()).rev() {
        map.insert(from[index], to.get(index).copied());
    }
    let output = source
        .iter()
        .filter_map(|byte| map.get(byte).copied().unwrap_or(Some(*byte)))
        .collect();
    Ok(Value::Bytes(output))
}

/// TRANSLATE UTF-8 路径。
fn translate_utf8(row: &[Value]) -> Result<Value, EvalError> {
    let (Some(source), Some(from), Some(to)) = (bytes(row, 0)?, bytes(row, 1)?, bytes(row, 2)?)
    else {
        return Ok(Value::Null);
    };
    let from: Vec<char> = String::from_utf8_lossy(from).chars().collect();
    let to: Vec<char> = String::from_utf8_lossy(to).chars().collect();
    let mut map: HashMap<char, Option<char>> = HashMap::new();
    for index in (0..from.len()).rev() {
        map.insert(from[index], to.get(index).copied());
    }
    let output: String = String::from_utf8_lossy(source)
        .chars()
        .filter_map(|character| map.get(&character).copied().unwrap_or(Some(character)))
        .collect();
    Ok(Value::Bytes(output.into_bytes()))
}

/// 在 haystack 中查找 needle 首次出现的字节偏移；空 needle 视为偏移 0。
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
