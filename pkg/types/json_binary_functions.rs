// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// MySQL 兼容二进制 JSON 上的函数实现：编解码、路径抽取/修改、比较与合并。
//
// 对齐 Go `json_binary_functions.go`：BinaryJSON 布局为类型码 + 载荷；
// 支持 Extract/Modify/Remove、Merge/MergePatch、Contains/Overlaps、Walk/Search 等。

use base64::Engine;
use std::collections::{BTreeMap, HashSet};
use std::fmt;

/// 数组/对象头部：元素个数(4) + 数据总长(4)。
const HEADER_SIZE: usize = 8;
/// 头部中数据总长字段偏移。
const DATA_SIZE_OFFSET: usize = 4;
/// 对象 key entry：偏移(4) + 长度(2)。
const KEY_ENTRY_SIZE: usize = 6;
/// key entry 内长度字段偏移。
const KEY_LENGTH_OFFSET: usize = 4;
/// value entry 类型码字节数。
const VALUE_TYPE_SIZE: usize = 1;
/// value entry：类型码(1) + 偏移/内联字面量(4)。
const VALUE_ENTRY_SIZE: usize = 5;
/// JSON 文档最大嵌套深度（与 TiDB 一致）。
const MAX_JSON_DEPTH: usize = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 二进制 JSON 操作错误（消息字符串）。
pub struct JsonBinaryError(String);

impl JsonBinaryError {
    /// 由消息构造错误。
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for JsonBinaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for JsonBinaryError {}

/// 小端读取 u16。
fn read_u16(input: &[u8], offset: usize) -> Result<u16, JsonBinaryError> {
    let bytes = input
        .get(offset..offset + 2)
        .ok_or_else(|| JsonBinaryError::new("truncated binary JSON u16"))?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

/// 小端读取 u32。
fn read_u32(input: &[u8], offset: usize) -> Result<u32, JsonBinaryError> {
    let bytes = input
        .get(offset..offset + 4)
        .ok_or_else(|| JsonBinaryError::new("truncated binary JSON u32"))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// 小端读取 u64。
fn read_u64(input: &[u8], offset: usize) -> Result<u64, JsonBinaryError> {
    let bytes = input
        .get(offset..offset + 8)
        .ok_or_else(|| JsonBinaryError::new("truncated binary JSON u64"))?;
    Ok(u64::from_le_bytes(
        bytes.try_into().expect("eight-byte slice"),
    ))
}

/// 小端写入 u16。
fn put_u16(output: &mut [u8], offset: usize, value: u16) {
    output[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

/// 小端写入 u32。
fn put_u32(output: &mut [u8], offset: usize, value: u32) {
    output[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// 追加无符号变长整数（protobuf 风格）。
fn append_uvarint(output: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        output.push((value as u8) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

/// 读取无符号变长整数，返回 (值, 消耗字节数)。
fn read_uvarint(input: &[u8]) -> Result<(u64, usize), JsonBinaryError> {
    let mut value = 0_u64;
    for (index, byte) in input.iter().copied().enumerate().take(10) {
        if index == 9 && byte > 1 {
            break;
        }
        value |= u64::from(byte & 0x7f) << (index * 7);
        if byte < 0x80 {
            return Ok((value, index + 1));
        }
    }
    Err(JsonBinaryError::new("invalid binary JSON varint"))
}

/// 读取字面量载荷首字节（null/true/false）。
fn literal_value(value: &BinaryJSON) -> Result<u8, JsonBinaryError> {
    value
        .Value
        .first()
        .copied()
        .ok_or_else(|| JsonBinaryError::new("truncated JSON literal"))
}

/// 读取数组/对象头部的元素个数。
fn element_count(value: &BinaryJSON) -> Result<usize, JsonBinaryError> {
    Ok(read_u32(&value.Value, 0)? as usize)
}

/// 按类型码计算载荷字节长度。
fn value_payload_len(type_code: JSONTypeCode, value: &[u8]) -> Result<usize, JsonBinaryError> {
    match type_code {
        JSONTypeCodeLiteral => Ok(1),
        JSONTypeCodeInt64
        | JSONTypeCodeUint64
        | JSONTypeCodeFloat64
        | JSONTypeCodeDate
        | JSONTypeCodeDatetime
        | JSONTypeCodeTimestamp => Ok(8),
        JSONTypeCodeDuration => Ok(12),
        JSONTypeCodeString => {
            let (length, prefix) = read_uvarint(value)?;
            Ok(prefix + length as usize)
        }
        JSONTypeCodeOpaque => {
            let (length, prefix) = read_uvarint(value.get(1..).unwrap_or_default())?;
            Ok(1 + prefix + length as usize)
        }
        JSONTypeCodeArray | JSONTypeCodeObject => Ok(read_u32(value, DATA_SIZE_OFFSET)? as usize),
        _ => Err(JsonBinaryError::new(format!(
            "unknown type code: {type_code}"
        ))),
    }
}

/// 从容器 value entry 解出子 BinaryJSON（字面量内联，其它按偏移切片）。
fn value_entry(value: &BinaryJSON, entry_offset: usize) -> Result<BinaryJSON, JsonBinaryError> {
    let type_code = *value
        .Value
        .get(entry_offset)
        .ok_or_else(|| JsonBinaryError::new("truncated binary JSON value entry"))?;
    if type_code == JSONTypeCodeLiteral {
        return Ok(BinaryJSON {
            TypeCode: type_code,
            Value: vec![
                *value
                    .Value
                    .get(entry_offset + VALUE_TYPE_SIZE)
                    .ok_or_else(|| JsonBinaryError::new("truncated inline literal"))?,
            ],
        });
    }
    let value_offset = read_u32(&value.Value, entry_offset + VALUE_TYPE_SIZE)? as usize;
    let remaining = value
        .Value
        .get(value_offset..)
        .ok_or_else(|| JsonBinaryError::new("invalid binary JSON value offset"))?;
    let length = value_payload_len(type_code, remaining)?;
    Ok(BinaryJSON {
        TypeCode: type_code,
        Value: remaining
            .get(..length)
            .ok_or_else(|| JsonBinaryError::new("truncated binary JSON value"))?
            .to_vec(),
    })
}

/// 展开二进制数组为元素列表。
fn array_elements(value: &BinaryJSON) -> Result<Vec<BinaryJSON>, JsonBinaryError> {
    if value.TypeCode != JSONTypeCodeArray {
        return Err(JsonBinaryError::new("JSON value is not an array"));
    }
    (0..element_count(value)?)
        .map(|index| value_entry(value, HEADER_SIZE + index * VALUE_ENTRY_SIZE))
        .collect()
}

/// 展开二进制对象为 (key, value) 列表（key 按字典序存储）。
fn object_entries(value: &BinaryJSON) -> Result<Vec<(Vec<u8>, BinaryJSON)>, JsonBinaryError> {
    if value.TypeCode != JSONTypeCodeObject {
        return Err(JsonBinaryError::new("JSON value is not an object"));
    }
    let count = element_count(value)?;
    (0..count)
        .map(|index| {
            let entry_offset = HEADER_SIZE + index * KEY_ENTRY_SIZE;
            let key_offset = read_u32(&value.Value, entry_offset)? as usize;
            let key_length = read_u16(&value.Value, entry_offset + KEY_LENGTH_OFFSET)? as usize;
            let key = value
                .Value
                .get(key_offset..key_offset + key_length)
                .ok_or_else(|| JsonBinaryError::new("truncated binary JSON object key"))?
                .to_vec();
            let item = value_entry(
                value,
                HEADER_SIZE + count * KEY_ENTRY_SIZE + index * VALUE_ENTRY_SIZE,
            )?;
            Ok((key, item))
        })
        .collect()
}

/// 将元素列表写入 value entry 区，非字面量追加到尾部并回填偏移。
fn build_binary_elements(
    mut output: Vec<u8>,
    entry_start: usize,
    elements: &[BinaryJSON],
) -> Result<Vec<u8>, JsonBinaryError> {
    for (index, element) in elements.iter().enumerate() {
        let entry = entry_start + index * VALUE_ENTRY_SIZE;
        output[entry] = element.TypeCode;
        if element.TypeCode == JSONTypeCodeLiteral {
            output[entry + VALUE_TYPE_SIZE] = literal_value(element)?;
        } else {
            let offset = u32::try_from(output.len())
                .map_err(|_| JsonBinaryError::new("binary JSON document is too large"))?;
            put_u32(&mut output, entry + VALUE_TYPE_SIZE, offset);
            output.extend_from_slice(&element.Value);
        }
    }
    Ok(output)
}

/// 由元素列表编码二进制数组。
fn buildBinaryJSONArray(elements: &[BinaryJSON]) -> Result<BinaryJSON, JsonBinaryError> {
    let entry_size = elements
        .len()
        .checked_mul(VALUE_ENTRY_SIZE)
        .and_then(|size| size.checked_add(HEADER_SIZE))
        .ok_or_else(|| JsonBinaryError::new("binary JSON array is too large"))?;
    let mut output = vec![0; entry_size];
    put_u32(&mut output, 0, elements.len() as u32);
    output = build_binary_elements(output, HEADER_SIZE, elements)?;
    let total_size = output.len() as u32;
    put_u32(&mut output, DATA_SIZE_OFFSET, total_size);
    Ok(BinaryJSON {
        TypeCode: JSONTypeCodeArray,
        Value: output,
    })
}

// 按 key 字节序排序后编码；key 长度不得超过 u16。
/// 由键值列表编码二进制对象。
fn buildBinaryJSONObject(entries: &[(Vec<u8>, BinaryJSON)]) -> Result<BinaryJSON, JsonBinaryError> {
    let mut entries = entries.to_vec();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    if entries
        .iter()
        .any(|entry| entry.0.len() > u16::MAX as usize)
    {
        return Err(JsonBinaryError::new(
            "TiDB does not yet support JSON objects with the key length >= 65536",
        ));
    }
    let count = entries.len();
    let entry_size = HEADER_SIZE + count * (KEY_ENTRY_SIZE + VALUE_ENTRY_SIZE);
    let mut output = vec![0; entry_size];
    put_u32(&mut output, 0, count as u32);
    for (index, (key, _)) in entries.iter().enumerate() {
        let entry = HEADER_SIZE + index * KEY_ENTRY_SIZE;
        let key_offset = output.len() as u32;
        put_u32(&mut output, entry, key_offset);
        put_u16(&mut output, entry + KEY_LENGTH_OFFSET, key.len() as u16);
        output.extend_from_slice(key);
    }
    let values: Vec<_> = entries.into_iter().map(|(_, value)| value).collect();
    output = build_binary_elements(output, HEADER_SIZE + count * KEY_ENTRY_SIZE, &values)?;
    let total_size = output.len() as u32;
    put_u32(&mut output, DATA_SIZE_OFFSET, total_size);
    Ok(BinaryJSON {
        TypeCode: JSONTypeCodeObject,
        Value: output,
    })
}

/// 将 serde_json::Value 转为 BinaryJSON，并校验嵌套深度。
pub fn CreateBinaryJSON(value: serde_json::Value) -> Result<BinaryJSON, JsonBinaryError> {
    let result = match value {
        serde_json::Value::Null => BinaryJSON {
            TypeCode: JSONTypeCodeLiteral,
            Value: vec![JSONLiteralNil],
        },
        serde_json::Value::Bool(value) => BinaryJSON {
            TypeCode: JSONTypeCodeLiteral,
            Value: vec![if value {
                JSONLiteralTrue
            } else {
                JSONLiteralFalse
            }],
        },
        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                BinaryJSON {
                    TypeCode: JSONTypeCodeInt64,
                    Value: value.to_le_bytes().to_vec(),
                }
            } else if let Some(value) = value.as_u64() {
                BinaryJSON {
                    TypeCode: JSONTypeCodeUint64,
                    Value: value.to_le_bytes().to_vec(),
                }
            } else {
                let value = value
                    .as_f64()
                    .ok_or_else(|| JsonBinaryError::new("invalid JSON number"))?;
                BinaryJSON {
                    TypeCode: JSONTypeCodeFloat64,
                    Value: value.to_bits().to_le_bytes().to_vec(),
                }
            }
        }
        serde_json::Value::String(value) => {
            let mut output = Vec::with_capacity(value.len() + 10);
            append_uvarint(&mut output, value.len() as u64);
            output.extend_from_slice(value.as_bytes());
            BinaryJSON {
                TypeCode: JSONTypeCodeString,
                Value: output,
            }
        }
        serde_json::Value::Array(values) => {
            let values = values
                .into_iter()
                .map(CreateBinaryJSON)
                .collect::<Result<Vec<_>, _>>()?;
            buildBinaryJSONArray(&values)?
        }
        serde_json::Value::Object(values) => {
            let entries = values
                .into_iter()
                .map(|(key, value)| Ok((key.into_bytes(), CreateBinaryJSON(value)?)))
                .collect::<Result<Vec<_>, JsonBinaryError>>()?;
            buildBinaryJSONObject(&entries)?
        }
    };
    if result.GetElemDepth() > MAX_JSON_DEPTH + 1 {
        return Err(JsonBinaryError::new("JSON document is too deep"));
    }
    Ok(result)
}

/// 将 BinaryJSON 转回 serde_json::Value（opaque 渲染为 base64 字符串）。
pub fn BinaryJSONToSerde(value: &BinaryJSON) -> Result<serde_json::Value, JsonBinaryError> {
    Ok(match value.TypeCode {
        JSONTypeCodeLiteral => match literal_value(value)? {
            JSONLiteralNil => serde_json::Value::Null,
            JSONLiteralTrue => serde_json::Value::Bool(true),
            JSONLiteralFalse => serde_json::Value::Bool(false),
            literal => return Err(JsonBinaryError::new(format!("unknown literal: {literal}"))),
        },
        JSONTypeCodeInt64 => serde_json::json!(read_u64(&value.Value, 0)? as i64),
        JSONTypeCodeUint64 => serde_json::json!(read_u64(&value.Value, 0)?),
        JSONTypeCodeFloat64 => serde_json::json!(f64::from_bits(read_u64(&value.Value, 0)?)),
        JSONTypeCodeString => {
            let (length, prefix) = read_uvarint(&value.Value)?;
            let bytes = value
                .Value
                .get(prefix..prefix + length as usize)
                .ok_or_else(|| JsonBinaryError::new("truncated JSON string"))?;
            serde_json::Value::String(String::from_utf8_lossy(bytes).into_owned())
        }
        JSONTypeCodeArray => serde_json::Value::Array(
            array_elements(value)?
                .iter()
                .map(BinaryJSONToSerde)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        JSONTypeCodeObject => {
            let mut object = serde_json::Map::new();
            for (key, value) in object_entries(value)? {
                object.insert(
                    String::from_utf8_lossy(&key).into_owned(),
                    BinaryJSONToSerde(&value)?,
                );
            }
            serde_json::Value::Object(object)
        }
        JSONTypeCodeOpaque => {
            let opaque = opaque_parts(value)?;
            serde_json::Value::String(format!(
                "base64:type{}:{}",
                opaque.0,
                base64::engine::general_purpose::STANDARD.encode(opaque.1)
            ))
        }
        JSONTypeCodeDate | JSONTypeCodeDatetime | JSONTypeCodeTimestamp => {
            serde_json::json!(read_u64(&value.Value, 0)?)
        }
        JSONTypeCodeDuration => serde_json::json!(read_u64(&value.Value, 0)? as i64),
        code => return Err(JsonBinaryError::new(format!("unknown type code: {code}"))),
    })
}

impl BinaryJSON {
    /// 序列化为 类型码 + 载荷。
    pub fn Serialize(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(self.Value.len() + 1);
        output.push(self.TypeCode);
        output.extend_from_slice(&self.Value);
        output
    }

    /// 返回 MySQL JSON_TYPE 风格的类型名字符串。
    pub fn Type(&self) -> String {
        match self.TypeCode {
            JSONTypeCodeObject => "OBJECT",
            JSONTypeCodeArray => "ARRAY",
            JSONTypeCodeLiteral if self.Value.first() == Some(&JSONLiteralNil) => "NULL",
            JSONTypeCodeLiteral => "BOOLEAN",
            JSONTypeCodeInt64 => "INTEGER",
            JSONTypeCodeUint64 => "UNSIGNED INTEGER",
            JSONTypeCodeFloat64 => "DOUBLE",
            JSONTypeCodeString => "STRING",
            JSONTypeCodeOpaque => match self.Value.first().copied().unwrap_or_default() {
                249 | 250 | 251 | 252 | 15 | 253 | 254 => "BLOB",
                16 => "BIT",
                _ => "OPAQUE",
            },
            JSONTypeCodeDate => "DATE",
            JSONTypeCodeDatetime | JSONTypeCodeTimestamp => "DATETIME",
            JSONTypeCodeDuration => "TIME",
            code => panic!("unknown type code: {code}"),
        }
        .to_owned()
    }

    /// JSON_UNQUOTE：字符串去引号，其它类型格式化为文本。
    pub fn Unquote(&self) -> Result<String, JsonBinaryError> {
        if self.TypeCode == JSONTypeCodeString {
            let value = BinaryJSONToSerde(self)?;
            return UnquoteString(
                serde_json::to_string(&value).expect("JSON string serialization"),
            );
        }
        format_binary_json(self)
    }
}

/// 若两端有双引号则反转义内部字符串，否则原样返回。
pub fn UnquoteString(value: String) -> Result<String, JsonBinaryError> {
    if value.len() >= 2 && value.as_bytes()[0] == b'"' && value.as_bytes()[value.len() - 1] == b'"'
    {
        return unquoteJSONString(value[1..value.len() - 1].to_owned());
    }
    Ok(value)
}

/// 解码 4 或 8 个十六进制字符为 UTF-8（支持代理对）。
fn decodeOneEscapedUnicode(input: &[u8]) -> Result<([u8; 4], usize, bool), JsonBinaryError> {
    if input.len() != 4 && input.len() != 8 {
        return Err(JsonBinaryError::new(format!(
            "Invalid unicode length: {}",
            input.len()
        )));
    }
    let text =
        std::str::from_utf8(input).map_err(|error| JsonBinaryError::new(error.to_string()))?;
    let first = u16::from_str_radix(&text[..4], 16)
        .map_err(|error| JsonBinaryError::new(error.to_string()))?;
    let character = if input.len() == 8 {
        let second = u16::from_str_radix(&text[4..], 16)
            .map_err(|error| JsonBinaryError::new(error.to_string()))?;
        let mut decoded = char::decode_utf16([first, second]);
        match (decoded.next(), decoded.next()) {
            (Some(Ok(value)), None) => value,
            _ => return Err(JsonBinaryError::new(format!("Invalid unicode: {text}"))),
        }
    } else if (0xd800..=0xdfff).contains(&first) {
        return Err(JsonBinaryError::new(format!("surrogate:{first:04x}")));
    } else {
        char::from_u32(u32::from(first))
            .ok_or_else(|| JsonBinaryError::new(format!("Invalid unicode: {text}")))?
    };
    let mut output = [0; 4];
    let size = character.encode_utf8(&mut output).len();
    Ok((output, size, false))
}

/// 测试导出：解码转义 Unicode。
pub fn DecodeOneEscapedUnicodeForTest(
    input: &[u8],
) -> Result<([u8; 4], usize, bool), JsonBinaryError> {
    decodeOneEscapedUnicode(input)
}

/// 反转义 JSON 字符串内容（`\u`、控制字符转义等）。
fn unquoteJSONString(input: String) -> Result<String, JsonBinaryError> {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\\' {
            output.push(bytes[index]);
            index += 1;
            continue;
        }
        index += 1;
        let escaped = *bytes
            .get(index)
            .ok_or_else(|| JsonBinaryError::new("Missing a closing quotation mark in string"))?;
        match escaped {
            b'"' => output.push(b'"'),
            b'b' => output.push(8),
            b'f' => output.push(12),
            b'n' => output.push(b'\n'),
            b'r' => output.push(b'\r'),
            b't' => output.push(b'\t'),
            b'\\' => output.push(b'\\'),
            b'u' => {
                let first = bytes
                    .get(index + 1..index + 5)
                    .ok_or_else(|| JsonBinaryError::new("Invalid unicode"))?;
                let decoded = decodeOneEscapedUnicode(first);
                let (character, size, _) = match decoded {
                    Ok(decoded) => decoded,
                    Err(error)
                        if error.0.starts_with("surrogate:")
                            && bytes.get(index + 5..index + 7) == Some(b"\\u") =>
                    {
                        let second = bytes
                            .get(index + 7..index + 11)
                            .ok_or_else(|| JsonBinaryError::new("Invalid unicode"))?;
                        let mut pair = Vec::with_capacity(8);
                        pair.extend_from_slice(first);
                        pair.extend_from_slice(second);
                        index += 6;
                        decodeOneEscapedUnicode(&pair)?
                    }
                    Err(error) => return Err(error),
                };
                output.extend_from_slice(&character[..size]);
                index += 4;
            }
            other => output.push(other),
        }
        index += 1;
    }
    String::from_utf8(output).map_err(|error| JsonBinaryError::new(error.to_string()))
}

/// 测试导出：UnquoteJSONString。
pub fn UnquoteJSONStringForTest(input: String) -> Result<String, JsonBinaryError> {
    unquoteJSONString(input)
}

/// 测试导出：按需对标识符加 JSON 引号。
pub fn QuoteJSONStringForTest(input: String) -> String {
    quoteJSONString(input)
}

/// 按字节判定 ECMAScript 标识符（对齐 Go 行为）。
fn is_ecmascript_identifier(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }

    // Go intentionally converts each UTF-8 byte to a rune before classifying
    // it. Preserve that byte-wise behavior: µ is accepted, while multi-byte
    // identifiers such as 你 and Ѡ require JSON quotes.
    for (index, byte) in value.bytes().enumerate() {
        let character = char::from(byte);
        if character.is_alphabetic() || character == '$' || character == '_' {
            continue;
        }
        if index != 0 && character.is_ascii_digit() {
            continue;
        }
        return false;
    }
    true
}

/// 标识符可裸写，否则返回 JSON 引号字符串。
fn quoteJSONString(value: String) -> String {
    let quoted = serde_json::to_string(&value).expect("string serialization cannot fail");
    if quoted == format!("\"{value}\"") && is_ecmascript_identifier(&value) {
        value
    } else {
        quoted
    }
}

/// 将 BinaryJSON 格式化为可读文本（数组/对象带空格）。
fn format_binary_json(value: &BinaryJSON) -> Result<String, JsonBinaryError> {
    Ok(match value.TypeCode {
        JSONTypeCodeArray => {
            let values = array_elements(value)?;
            let rendered = values
                .iter()
                .map(format_binary_json)
                .collect::<Result<Vec<_>, _>>()?;
            format!("[{}]", rendered.join(", "))
        }
        JSONTypeCodeObject => {
            let rendered = object_entries(value)?
                .into_iter()
                .map(|(key, value)| {
                    Ok(format!(
                        "{}: {}",
                        serde_json::to_string(&String::from_utf8_lossy(&key)).unwrap(),
                        format_binary_json(&value)?
                    ))
                })
                .collect::<Result<Vec<_>, JsonBinaryError>>()?;
            format!("{{{}}}", rendered.join(", "))
        }
        _ => serde_json::to_string(&BinaryJSONToSerde(value)?)
            .map_err(|error| JsonBinaryError::new(error.to_string()))?,
    })
}

/// 负下标相对数组长度解析。
fn resolve_index(index: isize, count: usize) -> isize {
    if index < 0 {
        count as isize + index
    } else {
        index
    }
}

/// 将路径数组选择解析为闭区间 `[start, end]`。
fn selection_range(selection: &JsonPathArraySelection, count: usize) -> Option<(usize, usize)> {
    let last = count.checked_sub(1)? as isize;
    let (start, end) = match *selection {
        JsonPathArraySelection::Asterisk => (0, last),
        JsonPathArraySelection::Index(index) => {
            let start = resolve_index(index, count);
            (start, start.min(last))
        }
        JsonPathArraySelection::Range(start, end) => (
            resolve_index(start, count),
            resolve_index(end, count).min(last),
        ),
    };
    (start >= 0 && start <= end).then_some((start as usize, end as usize))
}

/// 在已排序对象中二分查找 key。
fn object_search_key(
    value: &BinaryJSON,
    key: &[u8],
) -> Result<Option<BinaryJSON>, JsonBinaryError> {
    let entries = object_entries(value)?;
    match entries.binary_search_by(|entry| entry.0.as_slice().cmp(key)) {
        Ok(index) => Ok(Some(entries[index].1.clone())),
        Err(_) => Ok(None),
    }
}

/// 沿路径 legs 递归抽取；`one` 为真时找到即停。
fn extract_recursive(
    value: &BinaryJSON,
    legs: &[JsonPathLeg],
    identity: &mut Vec<usize>,
    one: bool,
    output: &mut Vec<(Vec<usize>, BinaryJSON)>,
) -> Result<(), JsonBinaryError> {
    if legs.is_empty() {
        if !output.iter().any(|(existing, _)| existing == identity) {
            output.push((identity.clone(), value.clone()));
        }
        return Ok(());
    }
    match &legs[0] {
        JsonPathLeg::Array(selection) => {
            if value.TypeCode != JSONTypeCodeArray {
                let matches_self = match *selection {
                    JsonPathArraySelection::Index(index) => index == 0 || index == -1,
                    JsonPathArraySelection::Range(start, end) => start == 0 && end >= -1,
                    JsonPathArraySelection::Asterisk => false,
                };
                if matches_self {
                    extract_recursive(value, &legs[1..], identity, one, output)?;
                }
                return Ok(());
            }
            let elements = array_elements(value)?;
            if let Some((start, end)) = selection_range(selection, elements.len()) {
                for (index, element) in elements.iter().enumerate().take(end + 1).skip(start) {
                    identity.push(index);
                    extract_recursive(element, &legs[1..], identity, one, output)?;
                    identity.pop();
                    if one && !output.is_empty() {
                        break;
                    }
                }
            }
        }
        JsonPathLeg::Key(key) if value.TypeCode == JSONTypeCodeObject => {
            if key == "*" {
                for (index, (_, child)) in object_entries(value)?.into_iter().enumerate() {
                    identity.push(index);
                    extract_recursive(&child, &legs[1..], identity, one, output)?;
                    identity.pop();
                    if one && !output.is_empty() {
                        break;
                    }
                }
            } else if let Some(child) = object_search_key(value, key.as_bytes())? {
                identity.push(
                    object_entries(value)?
                        .iter()
                        .position(|entry| entry.0 == key.as_bytes())
                        .unwrap_or_default(),
                );
                extract_recursive(&child, &legs[1..], identity, one, output)?;
                identity.pop();
            }
        }
        JsonPathLeg::DoubleAsterisk => {
            extract_recursive(value, &legs[1..], identity, one, output)?;
            if one && !output.is_empty() {
                return Ok(());
            }
            let children = if value.TypeCode == JSONTypeCodeArray {
                array_elements(value)?
            } else if value.TypeCode == JSONTypeCodeObject {
                object_entries(value)?
                    .into_iter()
                    .map(|entry| entry.1)
                    .collect()
            } else {
                Vec::new()
            };
            for (index, child) in children.into_iter().enumerate() {
                identity.push(index);
                extract_recursive(&child, legs, identity, one, output)?;
                identity.pop();
                if one && !output.is_empty() {
                    break;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

impl BinaryJSON {
    /// JSON_EXTRACT：多路径结果打包为数组，单路径单值则直接返回。
    pub fn Extract(
        &self,
        paths: &[JSONPathExpression],
    ) -> Result<Option<BinaryJSON>, JsonBinaryError> {
        let mut output = Vec::new();
        for path in paths {
            extract_recursive(self, &path.legs, &mut Vec::new(), false, &mut output)?;
        }
        if output.is_empty() {
            return Ok(None);
        }
        if paths.len() == 1 && output.len() == 1 && !paths[0].CouldMatchMultipleValues() {
            return Ok(Some(output.remove(0).1));
        }
        Ok(Some(buildBinaryJSONArray(
            &output.into_iter().map(|entry| entry.1).collect::<Vec<_>>(),
        )?))
    }
}

/// 路径是否含通配/范围（修改类操作不允许）。
fn has_multiple_selection(path: &JSONPathExpression) -> bool {
    path.legs.iter().any(|leg| {
        matches!(leg, JsonPathLeg::DoubleAsterisk)
            || matches!(leg, JsonPathLeg::Key(key) if key == "*")
            || matches!(
                leg,
                JsonPathLeg::Array(
                    JsonPathArraySelection::Asterisk | JsonPathArraySelection::Range(_, _)
                )
            )
    })
}

/// 按 Insert/Replace/Set 语义递归修改路径指向的值。
fn modify_recursive(
    value: &BinaryJSON,
    legs: &[JsonPathLeg],
    replacement: &BinaryJSON,
    modify_type: JSONModifyType,
) -> Result<(BinaryJSON, bool), JsonBinaryError> {
    if legs.is_empty() {
        return Ok(match modify_type {
            JSONModifyInsert => (value.clone(), false),
            JSONModifyReplace | JSONModifySet => (replacement.clone(), true),
            _ => return Err(JsonBinaryError::new("invalid JSON modify type")),
        });
    }
    match &legs[0] {
        JsonPathLeg::Key(key) if value.TypeCode == JSONTypeCodeObject => {
            let mut entries = object_entries(value)?;
            let position = entries.iter().position(|entry| entry.0 == key.as_bytes());
            if legs.len() == 1 {
                match (position, modify_type) {
                    (Some(_), JSONModifyInsert) | (None, JSONModifyReplace) => {
                        return Ok((value.clone(), false));
                    }
                    (Some(index), JSONModifyReplace | JSONModifySet) => {
                        entries[index].1 = replacement.clone();
                    }
                    (None, JSONModifyInsert | JSONModifySet) => {
                        entries.push((key.as_bytes().to_vec(), replacement.clone()));
                    }
                    _ => return Err(JsonBinaryError::new("invalid JSON modify type")),
                }
                return Ok((buildBinaryJSONObject(&entries)?, true));
            }
            let Some(position) = position else {
                return Ok((value.clone(), false));
            };
            let (child, changed) =
                modify_recursive(&entries[position].1, &legs[1..], replacement, modify_type)?;
            if changed {
                entries[position].1 = child;
                return Ok((buildBinaryJSONObject(&entries)?, true));
            }
            Ok((value.clone(), false))
        }
        JsonPathLeg::Array(JsonPathArraySelection::Index(index)) => {
            if value.TypeCode != JSONTypeCodeArray {
                if legs.len() != 1 {
                    return Ok((value.clone(), false));
                }
                if *index == 0 || *index == -1 {
                    return Ok(match modify_type {
                        JSONModifyInsert => (value.clone(), false),
                        JSONModifyReplace | JSONModifySet => (replacement.clone(), true),
                        _ => return Err(JsonBinaryError::new("invalid JSON modify type")),
                    });
                }
                return Ok(match modify_type {
                    JSONModifyInsert | JSONModifySet => (
                        buildBinaryJSONArray(&[value.clone(), replacement.clone()])?,
                        true,
                    ),
                    JSONModifyReplace => (value.clone(), false),
                    _ => return Err(JsonBinaryError::new("invalid JSON modify type")),
                });
            }
            let mut elements = array_elements(value)?;
            let resolved = resolve_index(*index, elements.len());
            if legs.len() == 1 {
                if resolved >= 0 && (resolved as usize) < elements.len() {
                    return Ok(match modify_type {
                        JSONModifyInsert => (value.clone(), false),
                        JSONModifyReplace | JSONModifySet => {
                            elements[resolved as usize] = replacement.clone();
                            (buildBinaryJSONArray(&elements)?, true)
                        }
                        _ => return Err(JsonBinaryError::new("invalid JSON modify type")),
                    });
                }
                return Ok(match modify_type {
                    JSONModifyInsert | JSONModifySet => {
                        elements.push(replacement.clone());
                        (buildBinaryJSONArray(&elements)?, true)
                    }
                    JSONModifyReplace => (value.clone(), false),
                    _ => return Err(JsonBinaryError::new("invalid JSON modify type")),
                });
            }
            if resolved < 0 || resolved as usize >= elements.len() {
                return Ok((value.clone(), false));
            }
            let (child, changed) = modify_recursive(
                &elements[resolved as usize],
                &legs[1..],
                replacement,
                modify_type,
            )?;
            if changed {
                elements[resolved as usize] = child;
                return Ok((buildBinaryJSONArray(&elements)?, true));
            }
            Ok((value.clone(), false))
        }
        _ => Ok((value.clone(), false)),
    }
}

impl BinaryJSON {
    /// JSON_SET/INSERT/REPLACE：路径与值一一对应，禁止通配。
    pub fn Modify(
        &self,
        paths: &[JSONPathExpression],
        values: &[BinaryJSON],
        modify_type: JSONModifyType,
    ) -> Result<BinaryJSON, JsonBinaryError> {
        if paths.len() != values.len() {
            return Err(JsonBinaryError::new("Incorrect parameter count"));
        }
        if paths.iter().any(has_multiple_selection) {
            return Err(JsonBinaryError::new(
                "JSON path may not contain * or a range",
            ));
        }
        let mut result = self.clone();
        for (path, value) in paths.iter().zip(values) {
            result = modify_recursive(&result, &path.legs, value, modify_type)?.0;
        }
        if result.GetElemDepth() > MAX_JSON_DEPTH + 1 {
            return Err(JsonBinaryError::new("JSON document is too deep"));
        }
        Ok(result)
    }

    /// 在指定数组下标处插入元素（路径末 leg 须为单下标）。
    pub fn ArrayInsert(
        &self,
        path: JSONPathExpression,
        value: BinaryJSON,
    ) -> Result<BinaryJSON, JsonBinaryError> {
        let (last, parent) = path
            .legs
            .split_last()
            .ok_or_else(|| JsonBinaryError::new("JSON path is not an array cell"))?;
        let JsonPathLeg::Array(JsonPathArraySelection::Index(index)) = last else {
            return Err(JsonBinaryError::new("JSON path is not an array cell"));
        };
        let parent_path = JSONPathExpression::from_legs(parent.to_vec());
        let Some(array) = self.Extract(std::slice::from_ref(&parent_path))? else {
            return Ok(self.clone());
        };
        if array.TypeCode != JSONTypeCodeArray {
            return Ok(self.clone());
        }
        let mut elements = array_elements(&array)?;
        let resolved = resolve_index(*index, elements.len()).max(0) as usize;
        elements.insert(resolved.min(elements.len()), value);
        self.Modify(
            std::slice::from_ref(&parent_path),
            &[buildBinaryJSONArray(&elements)?],
            JSONModifySet,
        )
    }
}

/// 递归删除路径指向的键或数组元素。
fn remove_recursive(
    value: &BinaryJSON,
    legs: &[JsonPathLeg],
) -> Result<(BinaryJSON, bool), JsonBinaryError> {
    match legs.first() {
        Some(JsonPathLeg::Key(key)) if value.TypeCode == JSONTypeCodeObject => {
            let mut entries = object_entries(value)?;
            let Some(position) = entries.iter().position(|entry| entry.0 == key.as_bytes()) else {
                return Ok((value.clone(), false));
            };
            if legs.len() == 1 {
                entries.remove(position);
                return Ok((buildBinaryJSONObject(&entries)?, true));
            }
            let (child, changed) = remove_recursive(&entries[position].1, &legs[1..])?;
            if changed {
                entries[position].1 = child;
                return Ok((buildBinaryJSONObject(&entries)?, true));
            }
            Ok((value.clone(), false))
        }
        Some(JsonPathLeg::Array(JsonPathArraySelection::Index(index)))
            if value.TypeCode == JSONTypeCodeArray =>
        {
            let mut elements = array_elements(value)?;
            let resolved = resolve_index(*index, elements.len());
            if resolved < 0 || resolved as usize >= elements.len() {
                return Ok((value.clone(), false));
            }
            if legs.len() == 1 {
                elements.remove(resolved as usize);
                return Ok((buildBinaryJSONArray(&elements)?, true));
            }
            let (child, changed) = remove_recursive(&elements[resolved as usize], &legs[1..])?;
            if changed {
                elements[resolved as usize] = child;
                return Ok((buildBinaryJSONArray(&elements)?, true));
            }
            Ok((value.clone(), false))
        }
        _ => Ok((value.clone(), false)),
    }
}

impl BinaryJSON {
    /// JSON_REMOVE：禁止空路径与通配。
    pub fn Remove(&self, paths: &[JSONPathExpression]) -> Result<BinaryJSON, JsonBinaryError> {
        let mut result = self.clone();
        for path in paths {
            if path.legs.is_empty() {
                return Err(JsonBinaryError::new("JSON path cannot be vacuous"));
            }
            if has_multiple_selection(path) {
                return Err(JsonBinaryError::new(
                    "JSON path may not contain * or a range",
                ));
            }
            result = remove_recursive(&result, &path.legs)?.0;
        }
        Ok(result)
    }
}

/// 浮点比较容差。
pub const floatEpsilon: f64 = 1.0e-8;

/// 在 floatEpsilon 内视为相等。
fn compare_float_precision_loss(left: f64, right: f64) -> i32 {
    if left - right < floatEpsilon && right - left < floatEpsilon {
        0
    } else if left < right {
        -1
    } else {
        1
    }
}

/// MySQL JSON 类型比较优先级（数值越小越低）。
fn precedence(value: &BinaryJSON) -> i32 {
    match value.Type().as_str() {
        "NULL" => -12,
        "INTEGER" | "UNSIGNED INTEGER" | "DOUBLE" => -11,
        "STRING" => -10,
        "OBJECT" => -9,
        "ARRAY" => -8,
        "BOOLEAN" => -7,
        "DATE" => -6,
        "TIME" => -5,
        "DATETIME" => -4,
        "OPAQUE" => -3,
        "BIT" => -2,
        "BLOB" => -1,
        _ => unreachable!(),
    }
}

/// 整数/无符号/浮点混比，对齐 MySQL 规则。
fn numeric_compare(left: &BinaryJSON, right: &BinaryJSON) -> Result<i32, JsonBinaryError> {
    Ok(match (left.TypeCode, right.TypeCode) {
        (JSONTypeCodeInt64, JSONTypeCodeInt64) => {
            (read_u64(&left.Value, 0)? as i64).cmp(&(read_u64(&right.Value, 0)? as i64)) as i32
        }
        (JSONTypeCodeUint64, JSONTypeCodeUint64) => {
            read_u64(&left.Value, 0)?.cmp(&read_u64(&right.Value, 0)?) as i32
        }
        (JSONTypeCodeInt64, JSONTypeCodeUint64) => {
            let left = read_u64(&left.Value, 0)? as i64;
            if left < 0 {
                -1
            } else {
                (left as u64).cmp(&read_u64(&right.Value, 0)?) as i32
            }
        }
        (JSONTypeCodeUint64, JSONTypeCodeInt64) => -numeric_compare(right, left)?,
        (JSONTypeCodeFloat64, JSONTypeCodeInt64) => compare_float_precision_loss(
            f64::from_bits(read_u64(&left.Value, 0)?),
            read_u64(&right.Value, 0)? as i64 as f64,
        ),
        (JSONTypeCodeFloat64, JSONTypeCodeUint64) => compare_float_precision_loss(
            f64::from_bits(read_u64(&left.Value, 0)?),
            read_u64(&right.Value, 0)? as f64,
        ),
        (JSONTypeCodeInt64 | JSONTypeCodeUint64, JSONTypeCodeFloat64) => {
            -numeric_compare(right, left)?
        }
        (JSONTypeCodeFloat64, JSONTypeCodeFloat64) => {
            let left = f64::from_bits(read_u64(&left.Value, 0)?);
            let right = f64::from_bits(read_u64(&right.Value, 0)?);
            if left < right {
                -1
            } else if left == right {
                0
            } else {
                1
            }
        }
        _ => 0,
    })
}

/// 拆出 opaque 的类型码与原始字节。
fn opaque_parts(value: &BinaryJSON) -> Result<(u8, &[u8]), JsonBinaryError> {
    let type_code = *value
        .Value
        .first()
        .ok_or_else(|| JsonBinaryError::new("truncated opaque value"))?;
    let (length, prefix) = read_uvarint(&value.Value[1..])?;
    let start = 1 + prefix;
    let bytes = value
        .Value
        .get(start..start + length as usize)
        .ok_or_else(|| JsonBinaryError::new("truncated opaque value"))?;
    Ok((type_code, bytes))
}

/// 先比类型优先级，再按类型内容比较。
fn compare_binary_json(left: &BinaryJSON, right: &BinaryJSON) -> Result<i32, JsonBinaryError> {
    let left_precedence = precedence(left);
    let right_precedence = precedence(right);
    if left_precedence != right_precedence {
        return Ok((left_precedence - right_precedence).signum());
    }
    Ok(match left.TypeCode {
        JSONTypeCodeLiteral => {
            if left_precedence == -12 {
                0
            } else {
                i32::from(literal_value(right)?) - i32::from(literal_value(left)?)
            }
        }
        JSONTypeCodeInt64 | JSONTypeCodeUint64 | JSONTypeCodeFloat64 => {
            numeric_compare(left, right)?
        }
        JSONTypeCodeString => {
            let left = BinaryJSONToSerde(left)?.as_str().unwrap().to_owned();
            let right = BinaryJSONToSerde(right)?.as_str().unwrap().to_owned();
            left.as_bytes().cmp(right.as_bytes()) as i32
        }
        JSONTypeCodeArray => {
            let left = array_elements(left)?;
            let right = array_elements(right)?;
            for (left, right) in left.iter().zip(&right) {
                let comparison = compare_binary_json(left, right)?;
                if comparison != 0 {
                    return Ok(comparison);
                }
            }
            (left.len() as i32 - right.len() as i32).signum()
        }
        JSONTypeCodeObject => {
            let left = object_entries(left)?;
            let right = object_entries(right)?;
            if left.len() != right.len() {
                return Ok((left.len() as i32 - right.len() as i32).signum());
            }
            for ((left_key, left), (right_key, right)) in left.iter().zip(&right) {
                let comparison = left_key.cmp(right_key) as i32;
                if comparison != 0 {
                    return Ok(comparison);
                }
                let comparison = compare_binary_json(left, right)?;
                if comparison != 0 {
                    return Ok(comparison);
                }
            }
            0
        }
        JSONTypeCodeOpaque => opaque_parts(left)?.1.cmp(opaque_parts(right)?.1) as i32,
        JSONTypeCodeDate | JSONTypeCodeDatetime | JSONTypeCodeTimestamp => {
            read_u64(&left.Value, 0)?.cmp(&read_u64(&right.Value, 0)?) as i32
        }
        JSONTypeCodeDuration => {
            (read_u64(&left.Value, 0)? as i64).cmp(&(read_u64(&right.Value, 0)? as i64)) as i32
        }
        _ => 0,
    })
}

/// 公开比较入口，返回 -1 / 0 / 1。
pub fn CompareBinaryJSON(left: &BinaryJSON, right: &BinaryJSON) -> i32 {
    compare_binary_json(left, right).expect("valid BinaryJSON")
}

/// RFC 7396 风格 Merge Patch：patch 为非对象则整体替换；null 删除键。
fn merge_patch(
    target: Option<&BinaryJSON>,
    patch: Option<&BinaryJSON>,
) -> Result<Option<BinaryJSON>, JsonBinaryError> {
    let Some(patch) = patch else {
        return Ok(None);
    };
    if patch.TypeCode != JSONTypeCodeObject {
        return Ok(Some(patch.clone()));
    }
    let Some(target) = target else {
        return Ok(None);
    };
    let mut values: BTreeMap<Vec<u8>, BinaryJSON> = if target.TypeCode == JSONTypeCodeObject {
        object_entries(target)?.into_iter().collect()
    } else {
        BTreeMap::new()
    };
    for (key, patch_value) in object_entries(patch)? {
        if patch_value.TypeCode == JSONTypeCodeLiteral
            && literal_value(&patch_value)? == JSONLiteralNil
        {
            values.remove(&key);
        } else {
            let empty_object = (patch_value.TypeCode == JSONTypeCodeObject
                && !values.contains_key(&key))
            .then(|| buildBinaryJSONObject(&[]))
            .transpose()?;
            let merged = merge_patch(
                values.get(&key).or(empty_object.as_ref()),
                Some(&patch_value),
            )?
            .ok_or_else(|| {
                JsonBinaryError::new("merge patch unexpectedly produced null pointer")
            })?;
            values.insert(key, merged);
        }
    }
    Ok(Some(buildBinaryJSONObject(
        &values.into_iter().collect::<Vec<_>>(),
    )?))
}

/// 从右向左找非对象起点，再依次应用 patch。
pub fn MergePatchBinaryJSON(
    values: &[Option<&BinaryJSON>],
) -> Result<Option<BinaryJSON>, JsonBinaryError> {
    if values.is_empty() {
        return Err(JsonBinaryError::new(
            "merge patch requires at least one value",
        ));
    }
    let start = values
        .iter()
        .rposition(|value| value.is_none_or(|value| value.TypeCode != JSONTypeCodeObject))
        .unwrap_or(0);
    let mut target = values[start].cloned();
    for patch in &values[start + 1..] {
        target = merge_patch(target.as_ref(), *patch)?;
    }
    Ok(target)
}

/// 合并为数组：子数组展平。
fn merge_array(values: &[BinaryJSON]) -> Result<BinaryJSON, JsonBinaryError> {
    let mut output = Vec::new();
    for value in values {
        if value.TypeCode == JSONTypeCodeArray {
            output.extend(array_elements(value)?);
        } else {
            output.push(value.clone());
        }
    }
    buildBinaryJSONArray(&output)
}

/// 合并对象：同键递归 MergeBinaryJSON。
fn merge_objects(values: &[BinaryJSON]) -> Result<BinaryJSON, JsonBinaryError> {
    let mut output = BTreeMap::<Vec<u8>, BinaryJSON>::new();
    for value in values {
        for (key, value) in object_entries(value)? {
            if let Some(previous) = output.remove(&key) {
                output.insert(key, MergeBinaryJSON(&[previous, value])?);
            } else {
                output.insert(key, value);
            }
        }
    }
    buildBinaryJSONObject(&output.into_iter().collect::<Vec<_>>())
}

/// JSON_MERGE：连续对象先合并，再与其它值组成数组。
pub fn MergeBinaryJSON(values: &[BinaryJSON]) -> Result<BinaryJSON, JsonBinaryError> {
    if values.is_empty() {
        return buildBinaryJSONArray(&[]);
    }
    let mut results = Vec::new();
    let mut index = 0;
    while index < values.len() {
        if values[index].TypeCode != JSONTypeCodeObject {
            results.push(values[index].clone());
            index += 1;
            continue;
        }
        let start = index;
        while index < values.len() && values[index].TypeCode == JSONTypeCodeObject {
            index += 1;
        }
        results.push(merge_objects(&values[start..index])?);
    }
    if results.len() == 1 {
        Ok(results.remove(0))
    } else {
        merge_array(&results)
    }
}

/// 窥视序列化字节，返回完整 BinaryJSON 占用长度。
pub fn PeekBytesAsJSON(bytes: &[u8]) -> Result<usize, JsonBinaryError> {
    let Some(type_code) = bytes.first().copied() else {
        return Err(JsonBinaryError::new("Cant peek from empty bytes"));
    };
    let payload = &bytes[1..];
    let payload_size = match type_code {
        JSONTypeCodeObject | JSONTypeCodeArray => read_u32(payload, DATA_SIZE_OFFSET)? as usize,
        JSONTypeCodeString => {
            let (length, prefix) = read_uvarint(payload)?;
            prefix + length as usize
        }
        JSONTypeCodeInt64
        | JSONTypeCodeUint64
        | JSONTypeCodeFloat64
        | JSONTypeCodeDate
        | JSONTypeCodeDatetime
        | JSONTypeCodeTimestamp => 8,
        JSONTypeCodeLiteral => 1,
        JSONTypeCodeOpaque => {
            let (length, prefix) = read_uvarint(payload.get(1..).unwrap_or_default())?;
            1 + prefix + length as usize
        }
        JSONTypeCodeDuration => 12,
        _ => return Err(JsonBinaryError::new("Invalid JSON bytes")),
    };
    Ok(1 + payload_size)
}

/// JSON_CONTAINS：对象键值递包含、数组元素全覆盖或标量相等。
pub fn ContainsBinaryJSON(value: &BinaryJSON, target: &BinaryJSON) -> bool {
    match value.TypeCode {
        JSONTypeCodeObject if target.TypeCode == JSONTypeCodeObject => object_entries(target)
            .and_then(|entries| {
                entries
                    .into_iter()
                    .try_fold(true, |_, (key, target_value)| {
                        Ok(object_search_key(value, &key)?
                            .is_some_and(|value| ContainsBinaryJSON(&value, &target_value)))
                    })
            })
            .unwrap_or(false),
        JSONTypeCodeObject => false,
        JSONTypeCodeArray if target.TypeCode == JSONTypeCodeArray => array_elements(target)
            .map(|targets| {
                targets
                    .iter()
                    .all(|target| ContainsBinaryJSON(value, target))
            })
            .unwrap_or(false),
        JSONTypeCodeArray => array_elements(value)
            .map(|values| values.iter().any(|value| ContainsBinaryJSON(value, target)))
            .unwrap_or(false),
        _ => CompareBinaryJSON(value, target) == 0,
    }
}

/// JSON_OVERLAPS：任一侧数组则元素相交；对象键值相等则重叠。
pub fn OverlapsBinaryJSON(value: &BinaryJSON, target: &BinaryJSON) -> bool {
    if value.TypeCode != JSONTypeCodeArray && target.TypeCode == JSONTypeCodeArray {
        return OverlapsBinaryJSON(target, value);
    }
    match value.TypeCode {
        JSONTypeCodeObject if target.TypeCode == JSONTypeCodeObject => object_entries(target)
            .map(|entries| {
                entries.into_iter().any(|(key, target_value)| {
                    object_search_key(value, &key)
                        .ok()
                        .flatten()
                        .is_some_and(|value| CompareBinaryJSON(&value, &target_value) == 0)
                })
            })
            .unwrap_or(false),
        JSONTypeCodeObject => false,
        JSONTypeCodeArray if target.TypeCode == JSONTypeCodeArray => {
            let left = array_elements(value).unwrap_or_default();
            let right = array_elements(target).unwrap_or_default();
            left.iter().any(|left| {
                right
                    .iter()
                    .any(|right| CompareBinaryJSON(left, right) == 0)
            })
        }
        JSONTypeCodeArray => array_elements(value)
            .unwrap_or_default()
            .iter()
            .any(|value| CompareBinaryJSON(value, target) == 0),
        _ => CompareBinaryJSON(value, target) == 0,
    }
}

impl BinaryJSON {
    /// 文档深度：标量为 1，容器为子节点最大深度 + 1。
    pub fn GetElemDepth(&self) -> usize {
        let children = if self.TypeCode == JSONTypeCodeArray {
            array_elements(self).unwrap_or_default()
        } else if self.TypeCode == JSONTypeCodeObject {
            object_entries(self)
                .unwrap_or_default()
                .into_iter()
                .map(|entry| entry.1)
                .collect()
        } else {
            return 1;
        };
        children
            .iter()
            .map(BinaryJSON::GetElemDepth)
            .max()
            .unwrap_or_default()
            + 1
    }
}

/// 带完整路径的抽取，供 Walk/Search 回调使用。
fn callback_extract(
    value: &BinaryJSON,
    legs: &[JsonPathLeg],
    full_path: &JSONPathExpression,
    output: &mut Vec<(JSONPathExpression, BinaryJSON)>,
) -> Result<(), JsonBinaryError> {
    if legs.is_empty() {
        output.push((full_path.clone(), value.clone()));
        return Ok(());
    }
    match &legs[0] {
        JsonPathLeg::Array(selection) if value.TypeCode == JSONTypeCodeArray => {
            let elements = array_elements(value)?;
            if let Some((start, end)) = selection_range(selection, elements.len()) {
                for (index, element) in elements.iter().enumerate().take(end + 1).skip(start) {
                    let path = full_path.push_array(index as isize);
                    callback_extract(element, &legs[1..], &path, output)?;
                }
            }
        }
        JsonPathLeg::Key(key) if value.TypeCode == JSONTypeCodeObject => {
            if key == "*" {
                for (key, child) in object_entries(value)? {
                    let path = full_path.push_key(String::from_utf8_lossy(&key).into_owned());
                    callback_extract(&child, &legs[1..], &path, output)?;
                }
            } else if let Some(child) = object_search_key(value, key.as_bytes())? {
                callback_extract(&child, &legs[1..], &full_path.push_key(key.clone()), output)?;
            }
        }
        JsonPathLeg::DoubleAsterisk => {
            callback_extract(value, &legs[1..], full_path, output)?;
            let children = if value.TypeCode == JSONTypeCodeArray {
                array_elements(value)?
                    .into_iter()
                    .enumerate()
                    .map(|(index, value)| {
                        (
                            JsonPathLeg::Array(JsonPathArraySelection::Index(index as isize)),
                            value,
                        )
                    })
                    .collect::<Vec<_>>()
            } else if value.TypeCode == JSONTypeCodeObject {
                object_entries(value)?
                    .into_iter()
                    .map(|(key, value)| {
                        (
                            JsonPathLeg::Key(String::from_utf8_lossy(&key).into_owned()),
                            value,
                        )
                    })
                    .collect()
            } else {
                Vec::new()
            };
            for (leg, child) in children {
                let path = full_path.push_leg(leg);
                callback_extract(&child, legs, &path, output)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// DFS 遍历；回调返回 true 则提前终止；`seen` 去重路径。
fn walk_tree<F>(
    value: &BinaryJSON,
    path: &JSONPathExpression,
    seen: &mut HashSet<String>,
    callback: &mut F,
) -> Result<bool, JsonBinaryError>
where
    F: FnMut(&JSONPathExpression, &BinaryJSON) -> Result<bool, JsonBinaryError>,
{
    if !seen.insert(path.to_string()) {
        return Ok(false);
    }
    if callback(path, value)? {
        return Ok(true);
    }
    if value.TypeCode == JSONTypeCodeArray {
        for (index, child) in array_elements(value)?.iter().enumerate() {
            if walk_tree(child, &path.push_array(index as isize), seen, callback)? {
                return Ok(true);
            }
        }
    } else if value.TypeCode == JSONTypeCodeObject {
        for (key, child) in object_entries(value)? {
            if walk_tree(
                &child,
                &path.push_key(String::from_utf8_lossy(&key).into_owned()),
                seen,
                callback,
            )? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

impl BinaryJSON {
    /// 路径遍历：无路径则从根走，有路径则先抽取再 Walk。
    pub fn Walk<F>(
        &self,
        mut callback: F,
        paths: &[JSONPathExpression],
    ) -> Result<(), JsonBinaryError>
    where
        F: FnMut(&JSONPathExpression, &BinaryJSON) -> Result<bool, JsonBinaryError>,
    {
        let root = JSONPathExpression::root();
        let mut seen = HashSet::new();
        if paths.is_empty() {
            let _ = walk_tree(self, &root, &mut seen, &mut callback)?;
            return Ok(());
        }
        for path in paths {
            let mut selected = Vec::new();
            callback_extract(self, &path.legs, &root, &mut selected)?;
            for (path, value) in selected {
                if walk_tree(&value, &path, &mut seen, &mut callback)? {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// JSON_SEARCH：对字符串叶子做 LIKE 匹配，返回命中路径。
    pub fn Search(
        &self,
        contain_type: &str,
        search: &str,
        escape: u8,
        paths: &[JSONPathExpression],
    ) -> Result<Option<BinaryJSON>, JsonBinaryError> {
        if contain_type != JSONContainsPathOne && contain_type != JSONContainsPathAll {
            return Err(JsonBinaryError::new("json_search expects 'one' or 'all'"));
        }
        let mut matches = Vec::new();
        self.Walk(
            |path, value| {
                if value.TypeCode == JSONTypeCodeString {
                    let text = BinaryJSONToSerde(value)?;
                    if like_match(text.as_str().unwrap_or_default(), search, escape) {
                        matches.push(path.to_string());
                        return Ok(contain_type == JSONContainsPathOne);
                    }
                }
                Ok(false)
            },
            paths,
        )?;
        match matches.len() {
            0 => Ok(None),
            1 => Ok(Some(CreateBinaryJSON(serde_json::json!(
                matches.remove(0)
            ))?)),
            _ => Ok(Some(CreateBinaryJSON(serde_json::json!(matches))?)),
        }
    }
}

// DP：`%` 任意、`_` 单个 Unicode 字符、escape 转义字面量。
/// SQL LIKE 风格匹配。
fn like_match(value: &str, pattern: &str, escape: u8) -> bool {
    // Go stringutil.CompilePattern/DoMatch converts both operands to []rune.
    // Using bytes here would make `_` consume only one byte of a UTF-8 character.
    let value: Vec<char> = value.chars().collect();
    let pattern: Vec<char> = pattern.chars().collect();
    let escape = char::from(escape);
    let mut table = vec![vec![false; value.len() + 1]; pattern.len() + 1];
    table[0][0] = true;
    let mut pattern_index = 0;
    while pattern_index < pattern.len() {
        if pattern[pattern_index] == escape && pattern_index + 1 < pattern.len() {
            let literal = pattern[pattern_index + 1];
            for value_index in 0..value.len() {
                table[pattern_index + 2][value_index + 1] =
                    table[pattern_index][value_index] && value[value_index] == literal;
            }
            pattern_index += 2;
            continue;
        }
        match pattern[pattern_index] {
            '%' => {
                for value_index in 0..=value.len() {
                    table[pattern_index + 1][value_index] = table[pattern_index][value_index]
                        || (value_index > 0 && table[pattern_index + 1][value_index - 1]);
                }
            }
            '_' => {
                for value_index in 0..value.len() {
                    table[pattern_index + 1][value_index + 1] = table[pattern_index][value_index];
                }
            }
            literal => {
                for value_index in 0..value.len() {
                    table[pattern_index + 1][value_index + 1] =
                        table[pattern_index][value_index] && value[value_index] == literal;
                }
            }
        }
        pattern_index += 1;
    }
    table[pattern.len()][value.len()]
}
