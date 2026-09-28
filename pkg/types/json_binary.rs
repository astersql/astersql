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

// MySQL 兼容的 Binary JSON 编解码实现。
//
// `BinaryJSON` 以类型码 + 字节缓冲表示 JSON 值；数组/对象条目中的偏移
// 相对该容器起始位置。提供序列化、随机访问、哈希规范化与深度检查，
// 对齐 Go `types` 包 binary JSON 语义。

#![allow(non_snake_case, non_upper_case_globals)]

use super::json_constants::*;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use std::collections::BTreeMap;

/// JSON 文档最大嵌套深度（与 MySQL/TiDB 一致为 100）。
pub const maxJSONDepth: usize = 100;

/// Binary JSON 值。布局对齐 TiDB/MySQL：容器内条目偏移相对容器起点。
/// BinaryJSON follows TiDB's MySQL-compatible binary JSON layout. Offsets in
/// array and object entries are relative to the beginning of that container.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BinaryJSON {
    pub TypeCode: JSONTypeCode,
    pub Value: Vec<u8>,
}

/// 不透明 JSON 值：保留原始类型码与二进制缓冲。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Opaque {
    pub TypeCode: u8,
    pub Buf: Vec<u8>,
}

/// Binary JSON 中的打包时间。位布局同 CoreTime（见 pkg/types/time.go）。
/// Packed time representation used by binary JSON. The bit layout is the same
/// CoreTime layout used by pkg/types/time.go.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct JsonTime {
    pub CoreTime: u64,
    pub TypeCode: JSONTypeCode,
    pub Fsp: u8,
}

/// Binary JSON 中的 DURATION：纳秒时长 + 小数精度（Fsp）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct JsonDuration {
    pub Duration: i64,
    pub Fsp: u32,
}

/// 内存侧 JSON 值枚举，用于编码前的中间表示。
#[derive(Clone, Debug, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    Number(String),
    String(String),
    Binary(BinaryJSON),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
    Opaque(Opaque),
    Time(JsonTime),
    Duration(JsonDuration),
}

impl From<()> for JsonValue {
    fn from(_: ()) -> Self {
        Self::Null
    }
}
impl From<bool> for JsonValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}
impl From<i64> for JsonValue {
    fn from(value: i64) -> Self {
        Self::I64(value)
    }
}
impl From<i32> for JsonValue {
    fn from(value: i32) -> Self {
        Self::I64(value as i64)
    }
}
impl From<u64> for JsonValue {
    fn from(value: u64) -> Self {
        Self::U64(value)
    }
}
impl From<u32> for JsonValue {
    fn from(value: u32) -> Self {
        Self::U64(value as u64)
    }
}
impl From<f64> for JsonValue {
    fn from(value: f64) -> Self {
        Self::F64(value)
    }
}
impl From<String> for JsonValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}
impl From<&str> for JsonValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}
impl From<BinaryJSON> for JsonValue {
    fn from(value: BinaryJSON) -> Self {
        Self::Binary(value)
    }
}
impl From<Opaque> for JsonValue {
    fn from(value: Opaque) -> Self {
        Self::Opaque(value)
    }
}
impl From<JsonTime> for JsonValue {
    fn from(value: JsonTime) -> Self {
        Self::Time(value)
    }
}
impl From<JsonDuration> for JsonValue {
    fn from(value: JsonDuration) -> Self {
        Self::Duration(value)
    }
}
impl From<Vec<JsonValue>> for JsonValue {
    fn from(value: Vec<JsonValue>) -> Self {
        Self::Array(value)
    }
}
impl From<BTreeMap<String, JsonValue>> for JsonValue {
    fn from(value: BTreeMap<String, JsonValue>) -> Self {
        Self::Object(value)
    }
}

impl BinaryJSON {
    /// 序列化为 JSON 文本字符串；失败时返回空串。
    pub fn String(&self) -> String {
        self.MarshalJSON()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default()
    }

    /// 深拷贝 TypeCode 与 Value 缓冲。
    pub fn Copy(&self) -> BinaryJSON {
        BinaryJSON {
            TypeCode: self.TypeCode,
            Value: self.Value.clone(),
        }
    }

    /// 按类型码写出 UTF-8 JSON 文本字节。
    pub fn MarshalJSON(&self) -> Result<Vec<u8>, JsonError> {
        self.marshalTo(Vec::with_capacity(self.Value.len() * 3 / 2))
    }

    /// 按 TypeCode 分派到各标量/容器的 marshal 路径。
    fn marshalTo(&self, mut buf: Vec<u8>) -> Result<Vec<u8>, JsonError> {
        match self.TypeCode {
            JSONTypeCodeOpaque => Ok(jsonMarshalOpaqueTo(buf, self.GetOpaque())),
            JSONTypeCodeString => Ok(jsonMarshalStringTo(buf, &self.GetString())),
            JSONTypeCodeLiteral => Ok(jsonMarshalLiteralTo(buf, self.Value[0])),
            JSONTypeCodeInt64 => {
                buf.extend_from_slice(self.GetInt64().to_string().as_bytes());
                Ok(buf)
            }
            JSONTypeCodeUint64 => {
                buf.extend_from_slice(self.GetUint64().to_string().as_bytes());
                Ok(buf)
            }
            JSONTypeCodeFloat64 => self.marshalFloat64To(buf),
            JSONTypeCodeArray => self.marshalArrayTo(buf),
            JSONTypeCodeObject => self.marshalObjTo(buf),
            JSONTypeCodeDate | JSONTypeCodeDatetime | JSONTypeCodeTimestamp => {
                Ok(jsonMarshalTimeTo(buf, self.GetTime()))
            }
            JSONTypeCodeDuration => Ok(jsonMarshalDurationTo(buf, self.GetDuration())),
            _ => Ok(buf),
        }
    }

    /// 仅数值类型判断是否为零；其它类型恒为 false。
    pub fn IsZero(&self) -> bool {
        match self.TypeCode {
            JSONTypeCodeInt64 => self.GetInt64() == 0,
            JSONTypeCodeUint64 => self.GetUint64() == 0,
            JSONTypeCodeFloat64 => self.GetFloat64() == 0.0,
            _ => false,
        }
    }

    /// 读取小端 i64（复用 u64 位模式）。
    pub fn GetInt64(&self) -> i64 {
        read_u64(&self.Value, 0) as i64
    }

    /// 读取小端 u64。
    pub fn GetUint64(&self) -> u64 {
        read_u64(&self.Value, 0)
    }

    /// 按 IEEE754 位模式解读为 f64。
    pub fn GetFloat64(&self) -> f64 {
        f64::from_bits(self.GetUint64())
    }

    /// 解码 uvarint 长度前缀后的字符串载荷。
    pub fn GetString(&self) -> Vec<u8> {
        let (length, length_size) = decode_uvarint(&self.Value).expect("valid binary JSON string");
        self.Value[length_size..length_size + length as usize].to_vec()
    }

    /// 解码 opaque：首字节类型码 + uvarint 长度 + 载荷。
    pub fn GetOpaque(&self) -> Opaque {
        let type_code = self.Value[0];
        let (length, length_size) = decode_uvarint(&self.Value[1..]).expect("valid opaque value");
        let start = length_size + 1;
        Opaque {
            TypeCode: type_code,
            Buf: self.Value[start..start + length as usize].to_vec(),
        }
    }

    /// 以默认 Fsp=0 读取打包时间。
    pub fn GetTime(&self) -> JsonTime {
        self.GetTimeWithFsp(0)
    }

    /// 读取打包时间并指定小数秒精度。
    pub fn GetTimeWithFsp(&self, fsp: u8) -> JsonTime {
        JsonTime {
            CoreTime: self.GetUint64(),
            TypeCode: self.TypeCode,
            Fsp: fsp,
        }
    }

    /// 读取 duration：8 字节时长 + 4 字节 Fsp。
    pub fn GetDuration(&self) -> JsonDuration {
        JsonDuration {
            Duration: self.GetInt64(),
            Fsp: read_u32(&self.Value, 8),
        }
    }

    /// opaque 字段的 MySQL 类型码（Value 首字节）。
    pub fn GetOpaqueFieldType(&self) -> u8 {
        self.Value[0]
    }

    /// 对象键列表编码为 JSON 数组；非对象返回空数组。
    pub fn GetKeys(&self) -> BinaryJSON {
        if self.TypeCode != JSONTypeCodeObject {
            return CreateBinaryJSON(Vec::<JsonValue>::new());
        }
        let keys = (0..self.GetElemCount())
            .map(|index| {
                JsonValue::String(String::from_utf8_lossy(&self.objectGetKey(index)).into_owned())
            })
            .collect::<Vec<_>>();
        CreateBinaryJSON(keys)
    }

    /// 容器元素个数（对象键值对数或数组长度）。
    pub fn GetElemCount(&self) -> usize {
        read_u32(&self.Value, 0) as usize
    }

    /// 按下标读取数组元素（经 value entry）。
    pub fn ArrayGetElem(&self, index: usize) -> BinaryJSON {
        self.valEntryGet(headerSize + index * valEntrySize)
    }

    /// 从 key entry 表读取第 index 个对象键字节。
    fn objectGetKey(&self, index: usize) -> Vec<u8> {
        let entry = headerSize + index * keyEntrySize;
        let offset = read_u32(&self.Value, entry) as usize;
        let length = read_u16(&self.Value, entry + keyLenOff) as usize;
        self.Value[offset..offset + length].to_vec()
    }

    /// 读取对象第 index 个值（key entry 区之后的 value entry）。
    fn objectGetVal(&self, index: usize) -> BinaryJSON {
        let count = self.GetElemCount();
        self.valEntryGet(headerSize + count * keyEntrySize + index * valEntrySize)
    }

    /// 解析 value entry：字面量内联；其余按类型计算载荷长度后切片。
    fn valEntryGet(&self, entry: usize) -> BinaryJSON {
        let type_code = self.Value[entry];
        if type_code == JSONTypeCodeLiteral {
            return BinaryJSON {
                TypeCode: type_code,
                Value: vec![self.Value[entry + valTypeSize]],
            };
        }
        let offset = read_u32(&self.Value, entry + valTypeSize) as usize;
        // 定长类型直接用固定字节数；变长类型先读长度前缀或容器 size 字段
        let length = match type_code {
            JSONTypeCodeInt64
            | JSONTypeCodeUint64
            | JSONTypeCodeFloat64
            | JSONTypeCodeDate
            | JSONTypeCodeDatetime
            | JSONTypeCodeTimestamp => 8,
            JSONTypeCodeDuration => 12,
            JSONTypeCodeString => {
                let (length, length_size) =
                    decode_uvarint(&self.Value[offset..]).expect("valid string entry");
                length_size + length as usize
            }
            JSONTypeCodeOpaque => {
                let (length, length_size) =
                    decode_uvarint(&self.Value[offset + 1..]).expect("valid opaque entry");
                1 + length_size + length as usize
            }
            _ => read_u32(&self.Value, offset + dataSizeOff) as usize,
        };
        BinaryJSON {
            TypeCode: type_code,
            Value: self.Value[offset..offset + length].to_vec(),
        }
    }

    /// 浮点序列化：拒绝非有限值；极端量级用科学计数法并对齐 Go 格式。
    fn marshalFloat64To(&self, mut buf: Vec<u8>) -> Result<Vec<u8>, JsonError> {
        let value = self.GetFloat64();
        if !value.is_finite() {
            return Err(JsonError::new(
                JsonErrorKind::UnsupportedValue,
                format!("unsupported value: {value}"),
            ));
        }
        let absolute = value.abs();
        let mut text = if absolute != 0.0 && (absolute < 1e-15 || absolute >= 1e15) {
            format!("{value:e}")
        } else {
            value.to_string()
        };
        // 规范化指数：去掉 `+` 与多余前导零；纯整数补 `.0`
        if let Some(position) = text.find('e') {
            let exponent = &text[position + 1..];
            let normalized = if let Some(rest) = exponent.strip_prefix('+') {
                rest
            } else if let Some(rest) = exponent.strip_prefix("-0") {
                &format!("-{rest}")
            } else if let Some(rest) = exponent.strip_prefix('0') {
                rest
            } else {
                exponent
            };
            text = format!("{}e{}", &text[..position], normalized);
        } else if !text.contains('.') {
            text.push_str(".0");
        }
        buf.extend_from_slice(text.as_bytes());
        Ok(buf)
    }

    /// 将数组序列化为 `[elem, ...]` 文本。
    fn marshalArrayTo(&self, mut buf: Vec<u8>) -> Result<Vec<u8>, JsonError> {
        buf.push(b'[');
        for index in 0..self.GetElemCount() {
            if index != 0 {
                buf.extend_from_slice(b", ");
            }
            buf = self.ArrayGetElem(index).marshalTo(buf)?;
        }
        buf.push(b']');
        Ok(buf)
    }

    /// 将对象序列化为 `{"k": v, ...}` 文本。
    fn marshalObjTo(&self, mut buf: Vec<u8>) -> Result<Vec<u8>, JsonError> {
        buf.push(b'{');
        for index in 0..self.GetElemCount() {
            if index != 0 {
                buf.extend_from_slice(b", ");
            }
            buf = jsonMarshalStringTo(buf, &self.objectGetKey(index));
            buf.extend_from_slice(b": ");
            buf = self.objectGetVal(index).marshalTo(buf)?;
        }
        buf.push(b'}');
        Ok(buf)
    }

    /// 计算 Go `CalculateHashValueSize` 报告的长度。
    ///
    /// 精确可表示的整数按 float64 计 9 字节；其余值（包括容器）按存储载荷
    /// 加类型码字节计算。Go 当前也不会递归计算容器的规范化哈希长度。
    pub fn CalculateHashValueSize(&self) -> i64 {
        match self.TypeCode {
            JSONTypeCodeInt64 if getInt64FractionLength(self.GetInt64()) <= 52 => 9,
            JSONTypeCodeUint64 if getUint64FractionLength(self.GetUint64()) <= 52 => 9,
            _ => self.Value.len() as i64 + 1,
        }
    }

    /// 写出哈希输入：可精确表示的整数规范为 float64，容器递归展开。
    pub fn HashValue(&self, mut buf: Vec<u8>) -> Vec<u8> {
        match self.TypeCode {
            JSONTypeCodeInt64 if getInt64FractionLength(self.GetInt64()) <= 52 => {
                buf.push(JSONTypeCodeFloat64);
                appendBinaryFloat64(buf, self.GetInt64() as f64)
            }
            JSONTypeCodeUint64 if getUint64FractionLength(self.GetUint64()) <= 52 => {
                buf.push(JSONTypeCodeFloat64);
                appendBinaryFloat64(buf, self.GetUint64() as f64)
            }
            JSONTypeCodeArray => {
                buf.push(self.TypeCode);
                buf.extend_from_slice(&self.Value[..dataSizeOff]);
                for index in 0..self.GetElemCount() {
                    buf = self.ArrayGetElem(index).HashValue(buf);
                }
                buf
            }
            JSONTypeCodeObject => {
                buf.push(self.TypeCode);
                buf.extend_from_slice(&self.Value[..dataSizeOff]);
                for index in 0..self.GetElemCount() {
                    buf = appendBinaryString(
                        buf,
                        &String::from_utf8_lossy(&self.objectGetKey(index)),
                    );
                    buf = self.objectGetVal(index).HashValue(buf);
                }
                buf
            }
            _ => {
                buf.push(self.TypeCode);
                buf.extend_from_slice(&self.Value);
                buf
            }
        }
    }

    /// 解码为内存侧 `JsonValue` 树。
    pub fn GetValue(&self) -> JsonValue {
        match self.TypeCode {
            JSONTypeCodeLiteral => match self.Value[0] {
                JSONLiteralTrue => JsonValue::Bool(true),
                JSONLiteralFalse => JsonValue::Bool(false),
                _ => JsonValue::Null,
            },
            JSONTypeCodeInt64 => JsonValue::I64(self.GetInt64()),
            JSONTypeCodeUint64 => JsonValue::U64(self.GetUint64()),
            JSONTypeCodeFloat64 => JsonValue::F64(self.GetFloat64()),
            JSONTypeCodeString => {
                JsonValue::String(String::from_utf8_lossy(&self.GetString()).into_owned())
            }
            JSONTypeCodeOpaque => JsonValue::Opaque(self.GetOpaque()),
            JSONTypeCodeDate | JSONTypeCodeDatetime | JSONTypeCodeTimestamp => {
                JsonValue::Time(self.GetTime())
            }
            JSONTypeCodeDuration => JsonValue::Duration(self.GetDuration()),
            JSONTypeCodeArray => JsonValue::Array(
                (0..self.GetElemCount())
                    .map(|index| self.ArrayGetElem(index).GetValue())
                    .collect(),
            ),
            JSONTypeCodeObject => {
                let mut object = BTreeMap::new();
                for index in 0..self.GetElemCount() {
                    object.insert(
                        String::from_utf8_lossy(&self.objectGetKey(index)).into_owned(),
                        self.objectGetVal(index).GetValue(),
                    );
                }
                JsonValue::Object(object)
            }
            _ => JsonValue::Null,
        }
    }

    /// 文档嵌套深度（标量为 1，容器为 1+子节点最大深度）。
    pub fn GetElemDepth(&self) -> usize {
        match self.TypeCode {
            JSONTypeCodeArray => {
                1 + (0..self.GetElemCount())
                    .map(|index| self.ArrayGetElem(index).GetElemDepth())
                    .max()
                    .unwrap_or(0)
            }
            JSONTypeCodeObject => {
                1 + (0..self.GetElemCount())
                    .map(|index| self.objectGetVal(index).GetElemDepth())
                    .max()
                    .unwrap_or(0)
            }
            _ => 1,
        }
    }

    /// JSON_TYPE 风格的类型名字符串。
    pub fn Type(&self) -> &'static str {
        match self.TypeCode {
            JSONTypeCodeObject => "OBJECT",
            JSONTypeCodeArray => "ARRAY",
            JSONTypeCodeLiteral if self.Value.first() == Some(&JSONLiteralNil) => "NULL",
            JSONTypeCodeLiteral => "BOOLEAN",
            JSONTypeCodeInt64 => "INTEGER",
            JSONTypeCodeUint64 => "UNSIGNED INTEGER",
            JSONTypeCodeFloat64 => "DOUBLE",
            JSONTypeCodeString => "STRING",
            JSONTypeCodeOpaque => "OPAQUE",
            JSONTypeCodeDate => "DATE",
            JSONTypeCodeDatetime | JSONTypeCodeTimestamp => "DATETIME",
            JSONTypeCodeDuration => "TIME",
            _ => "OPAQUE",
        }
    }

    /// 用 JSON 文本字节替换自身内容（经 serde 解析再编码）。
    pub fn UnmarshalJSON(&mut self, data: &[u8]) -> Result<(), JsonError> {
        let value: serde_json::Value = serde_json::from_slice(data)
            .map_err(|error| JsonError::new(JsonErrorKind::InvalidJsonText, error.to_string()))?;
        let replacement = CreateBinaryJSONWithCheck(from_serde_value(value))?;
        *self = replacement;
        Ok(())
    }
}

/// 从 JSON 文本解析为 BinaryJSON；空文档与尾随内容按 Go 错误语义报错。
pub fn ParseBinaryJSONFromString(input: &str) -> Result<BinaryJSON, JsonError> {
    if input.is_empty() {
        return Err(JsonError::new(
            JsonErrorKind::InvalidJsonText,
            "The document is empty",
        ));
    }
    let value: serde_json::Value = serde_json::from_str(input).map_err(|error| {
        JsonError::new(
            JsonErrorKind::InvalidJsonText,
            format!("The document root must not be followed by other values: {error}"),
        )
    })?;
    CreateBinaryJSONWithCheck(from_serde_value(value))
}

/// 编码为 BinaryJSON；失败时 panic（对齐 Go 无检查路径）。
pub fn CreateBinaryJSON<T: Into<JsonValue>>(input: T) -> BinaryJSON {
    CreateBinaryJSONWithCheck(input).unwrap_or_else(|error| panic!("{error}"))
}

/// 编码为 BinaryJSON，并检查最大深度等约束。
pub fn CreateBinaryJSONWithCheck<T: Into<JsonValue>>(input: T) -> Result<BinaryJSON, JsonError> {
    let value = input.into();
    if value_depth(&value).saturating_sub(1) > maxJSONDepth {
        return Err(JsonError::new(
            JsonErrorKind::DocumentTooDeep,
            "JSON document exceeds maximum depth 100",
        ));
    }
    let (type_code, bytes) = appendBinaryJSON(Vec::new(), &value)?;
    Ok(BinaryJSON {
        TypeCode: type_code,
        Value: bytes,
    })
}

/// 估算编码后 Value 缓冲大小（不含类型码字节）。
pub fn CalculateBinaryJSONSize<T: Into<JsonValue>>(input: T) -> i64 {
    calculate_value_size(&input.into()).unwrap_or_else(|error| panic!("{error}"))
}

/// 按 JsonValue 变体追加二进制载荷并返回类型码。
fn appendBinaryJSON(
    mut buf: Vec<u8>,
    value: &JsonValue,
) -> Result<(JSONTypeCode, Vec<u8>), JsonError> {
    let type_code = match value {
        JsonValue::Null => {
            buf.push(JSONLiteralNil);
            JSONTypeCodeLiteral
        }
        JsonValue::Bool(value) => {
            buf.push(if *value {
                JSONLiteralTrue
            } else {
                JSONLiteralFalse
            });
            JSONTypeCodeLiteral
        }
        JsonValue::I64(value) => {
            buf = appendBinaryUint64(buf, *value as u64);
            JSONTypeCodeInt64
        }
        JsonValue::U64(value) => {
            buf = appendBinaryUint64(buf, *value);
            JSONTypeCodeUint64
        }
        JsonValue::F64(value) => {
            buf = appendBinaryFloat64(buf, *value);
            JSONTypeCodeFloat64
        }
        JsonValue::Number(value) => {
            let (code, new_buf) = appendBinaryNumber(buf, value)?;
            buf = new_buf;
            code
        }
        JsonValue::String(value) => {
            buf = appendBinaryString(buf, value);
            JSONTypeCodeString
        }
        JsonValue::Binary(value) => {
            buf.extend_from_slice(&value.Value);
            value.TypeCode
        }
        JsonValue::Array(value) => {
            buf = appendBinaryArray(buf, value)?;
            JSONTypeCodeArray
        }
        JsonValue::Object(value) => {
            buf = appendBinaryObject(buf, value)?;
            JSONTypeCodeObject
        }
        JsonValue::Opaque(value) => {
            buf = appendBinaryOpaque(buf, value);
            JSONTypeCodeOpaque
        }
        JsonValue::Time(value) => {
            buf = appendBinaryUint64(buf, value.CoreTime);
            value.TypeCode
        }
        JsonValue::Duration(value) => {
            buf = appendBinaryUint64(buf, value.Duration as u64);
            buf = appendBinaryUint32(buf, value.Fsp);
            JSONTypeCodeDuration
        }
    };
    Ok((type_code, buf))
}

/// 将十进制数字字符串编码为 int64 / uint64 / float64。
fn appendBinaryNumber(buf: Vec<u8>, number: &str) -> Result<(JSONTypeCode, Vec<u8>), JsonError> {
    if let Ok(value) = number.parse::<i64>() {
        return Ok((JSONTypeCodeInt64, appendBinaryUint64(buf, value as u64)));
    }
    if let Ok(value) = number.parse::<u64>() {
        return Ok((JSONTypeCodeUint64, appendBinaryUint64(buf, value)));
    }
    let value = number
        .parse::<f64>()
        .map_err(|error| JsonError::new(JsonErrorKind::InvalidJsonData, error.to_string()))?;
    Ok((JSONTypeCodeFloat64, appendBinaryFloat64(buf, value)))
}

/// 编码数组：element_count + size 占位 + value entry 表 + 载荷，最后回填 size。
fn appendBinaryArray(mut buf: Vec<u8>, array: &[JsonValue]) -> Result<Vec<u8>, JsonError> {
    let document_offset = buf.len();
    buf = appendBinaryUint32(buf, array.len() as u32);
    buf = appendZero(buf, dataSizeOff);
    let entries = buf.len();
    buf = appendZero(buf, array.len() * valEntrySize);
    for (index, value) in array.iter().enumerate() {
        buf = appendBinaryValElem(buf, document_offset, entries + index * valEntrySize, value)?;
    }
    let document_size = (buf.len() - document_offset) as u32;
    write_u32(&mut buf, document_offset + dataSizeOff, document_size);
    Ok(buf)
}

/// 编码对象：先写全部 key entry/键字节，再写 value entry 与载荷。
fn appendBinaryObject(
    mut buf: Vec<u8>,
    object: &BTreeMap<String, JsonValue>,
) -> Result<Vec<u8>, JsonError> {
    let document_offset = buf.len();
    buf = appendBinaryUint32(buf, object.len() as u32);
    buf = appendZero(buf, dataSizeOff);
    let key_entries = buf.len();
    buf = appendZero(buf, object.len() * keyEntrySize);
    let value_entries = buf.len();
    buf = appendZero(buf, object.len() * valEntrySize);

    for (index, key) in object.keys().enumerate() {
        if key.len() > u16::MAX as usize {
            return Err(JsonError::new(
                JsonErrorKind::ObjectKeyTooLong,
                "TiDB does not yet support JSON objects with the key length >= 65536",
            ));
        }
        let entry = key_entries + index * keyEntrySize;
        let key_offset = (buf.len() - document_offset) as u32;
        write_u32(&mut buf, entry, key_offset);
        write_u16(&mut buf, entry + keyLenOff, key.len() as u16);
        buf.extend_from_slice(key.as_bytes());
    }
    for (index, value) in object.values().enumerate() {
        buf = appendBinaryValElem(
            buf,
            document_offset,
            value_entries + index * valEntrySize,
            value,
        )?;
    }
    let document_size = (buf.len() - document_offset) as u32;
    write_u32(&mut buf, document_offset + dataSizeOff, document_size);
    Ok(buf)
}

/// 将单个值写入 entry：字面量内联到 entry；其它写入相对容器的偏移。
fn appendBinaryValElem(
    buf: Vec<u8>,
    document_offset: usize,
    entry: usize,
    value: &JsonValue,
) -> Result<Vec<u8>, JsonError> {
    let value_offset = buf.len();
    let (type_code, mut buf) = appendBinaryJSON(buf, value)?;
    if type_code == JSONTypeCodeLiteral {
        let literal = buf[value_offset];
        buf.truncate(value_offset);
        buf[entry] = JSONTypeCodeLiteral;
        buf[entry + 1] = literal;
        return Ok(buf);
    }
    buf[entry] = type_code;
    write_u32(&mut buf, entry + 1, (value_offset - document_offset) as u32);
    Ok(buf)
}

/// JSON 字符串转义写出（含 LS/PS 与控制字符的 `\u00XX`）。
fn jsonMarshalStringTo(mut buf: Vec<u8>, input: &[u8]) -> Vec<u8> {
    buf.push(b'"');
    let text = String::from_utf8_lossy(input);
    for character in text.chars() {
        match character {
            '"' => buf.extend_from_slice(b"\\\""),
            '\\' => buf.extend_from_slice(b"\\\\"),
            '\n' => buf.extend_from_slice(b"\\n"),
            '\r' => buf.extend_from_slice(b"\\r"),
            '\t' => buf.extend_from_slice(b"\\t"),
            '\u{08}' => buf.extend_from_slice(b"\\b"),
            '\u{0c}' => buf.extend_from_slice(b"\\f"),
            '\u{2028}' => buf.extend_from_slice(b"\\u2028"),
            '\u{2029}' => buf.extend_from_slice(b"\\u2029"),
            character if (character as u32) < 0x20 => {
                let byte = character as u8;
                buf.extend_from_slice(b"\\u00");
                buf.push(jsonHexChars[(byte >> 4) as usize]);
                buf.push(jsonHexChars[(byte & 0x0f) as usize]);
            }
            character if character.is_ascii() && jsonSafeSet[character as usize] => {
                buf.push(character as u8);
            }
            character => {
                let mut encoded = [0; 4];
                buf.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
            }
        }
    }
    buf.push(b'"');
    buf
}

/// opaque 序列化为 `"base64:typeN:..."` 文本形式。
fn jsonMarshalOpaqueTo(mut buf: Vec<u8>, opaque: Opaque) -> Vec<u8> {
    let encoded = BASE64_STANDARD.encode(opaque.Buf);
    buf.extend_from_slice(format!("\"base64:type{}:{}\"", opaque.TypeCode, encoded).as_bytes());
    buf
}

/// 字面量序列化为 null/true/false。
fn jsonMarshalLiteralTo(mut buf: Vec<u8>, literal: u8) -> Vec<u8> {
    match literal {
        JSONLiteralFalse => buf.extend_from_slice(b"false"),
        JSONLiteralTrue => buf.extend_from_slice(b"true"),
        JSONLiteralNil => buf.extend_from_slice(b"null"),
        _ => {}
    }
    buf
}

/// 时间值以带引号字符串形式写出。
fn jsonMarshalTimeTo(buf: Vec<u8>, time: JsonTime) -> Vec<u8> {
    jsonMarshalStringTo(buf, format_json_time(&time).as_bytes())
}

/// duration 以带引号字符串形式写出。
fn jsonMarshalDurationTo(buf: Vec<u8>, duration: JsonDuration) -> Vec<u8> {
    jsonMarshalStringTo(buf, format_json_duration(&duration).as_bytes())
}

/// 从 CoreTime 位域格式化 DATE 或 DATETIME 字符串。
fn format_json_time(time: &JsonTime) -> String {
    let packed = time.CoreTime;
    let year = (packed >> 50) & ((1 << 14) - 1);
    let month = (packed >> 46) & 0xf;
    let day = (packed >> 41) & 0x1f;
    if time.TypeCode == JSONTypeCodeDate {
        return format!("{year:04}-{month:02}-{day:02}");
    }
    let hour = (packed >> 36) & 0x1f;
    let minute = (packed >> 30) & 0x3f;
    let second = (packed >> 24) & 0x3f;
    let microsecond = (packed >> 4) & ((1 << 20) - 1);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{microsecond:06}")
}

/// 将纳秒 duration 格式化为 `±HH:MM:SS.micro`。
fn format_json_duration(duration: &JsonDuration) -> String {
    let negative = duration.Duration < 0;
    let nanos = duration.Duration.unsigned_abs();
    let total_seconds = nanos / 1_000_000_000;
    let hours = total_seconds / 3600;
    let minutes = total_seconds % 3600 / 60;
    let seconds = total_seconds % 60;
    let micros = nanos % 1_000_000_000 / 1000;
    format!(
        "{}{hours:02}:{minutes:02}:{seconds:02}.{micros:06}",
        if negative { "-" } else { "" }
    )
}

/// 递归估算 JsonValue 编码后的字节数。
fn calculate_value_size(value: &JsonValue) -> Result<i64, JsonError> {
    Ok(match value {
        JsonValue::Null | JsonValue::Bool(_) => 1,
        JsonValue::I64(_) | JsonValue::U64(_) | JsonValue::F64(_) | JsonValue::Time(_) => 8,
        JsonValue::Number(number) => {
            number
                .parse::<i64>()
                .or_else(|_| number.parse::<u64>().map(|v| v as i64))
                .or_else(|_| number.parse::<f64>().map(|v| v as i64))
                .map_err(|error| {
                    JsonError::new(JsonErrorKind::InvalidJsonData, error.to_string())
                })?;
            8
        }
        JsonValue::String(value) => calculateBinaryStringSize(value.len()),
        JsonValue::Binary(value) => value.Value.len() as i64,
        JsonValue::Array(array) => {
            let mut size =
                array.len() as i64 + dataSizeOff as i64 + array.len() as i64 * valEntrySize as i64;
            for value in array {
                size += calculate_value_size(value)?;
            }
            size
        }
        JsonValue::Object(object) => {
            let mut size =
                4 + dataSizeOff as i64 + object.len() as i64 * (keyEntrySize + valEntrySize) as i64;
            for (key, value) in object {
                size += key.len() as i64 + calculate_value_size(value)?;
            }
            size
        }
        JsonValue::Opaque(value) => 1 + 10 + value.Buf.len() as i64,
        JsonValue::Duration(_) => 12,
    })
}

/// 计算 JsonValue 树深度。
fn value_depth(value: &JsonValue) -> usize {
    match value {
        JsonValue::Array(values) => 1 + values.iter().map(value_depth).max().unwrap_or(0),
        JsonValue::Object(values) => 1 + values.values().map(value_depth).max().unwrap_or(0),
        JsonValue::Binary(value) => value.GetElemDepth(),
        _ => 1,
    }
}

/// 将 serde_json::Value 转为中间 JsonValue（数字保留为 Number 字符串）。
fn from_serde_value(value: serde_json::Value) -> JsonValue {
    match value {
        serde_json::Value::Null => JsonValue::Null,
        serde_json::Value::Bool(value) => JsonValue::Bool(value),
        serde_json::Value::Number(value) => JsonValue::Number(value.to_string()),
        serde_json::Value::String(value) => JsonValue::String(value),
        serde_json::Value::Array(values) => {
            JsonValue::Array(values.into_iter().map(from_serde_value).collect())
        }
        serde_json::Value::Object(values) => JsonValue::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, from_serde_value(value)))
                .collect(),
        ),
    }
}

/// 有符号整数有效尾数位数（用于判断能否无损转 float64）。
fn getInt64FractionLength(value: i64) -> u32 {
    getUint64FractionLength(value.unsigned_abs())
}

/// 无符号整数有效尾数位数。
fn getUint64FractionLength(value: u64) -> u32 {
    if value == 0 {
        0
    } else {
        63 - value.leading_zeros() - value.trailing_zeros()
    }
}

/// 字符串编码大小上界（uvarint 最大约 10 字节 + 载荷）。
fn calculateBinaryStringSize(length: usize) -> i64 {
    10 + length as i64
}

fn appendZero(mut buf: Vec<u8>, length: usize) -> Vec<u8> {
    buf.resize(buf.len() + length, 0);
    buf
}

/// 写入 uvarint 长度前缀后的 UTF-8 字符串。
fn appendBinaryString(mut buf: Vec<u8>, value: &str) -> Vec<u8> {
    encode_uvarint(value.len() as u64, &mut buf);
    buf.extend_from_slice(value.as_bytes());
    buf
}

/// 写入 opaque：类型码 + uvarint 长度 + 载荷。
fn appendBinaryOpaque(mut buf: Vec<u8>, value: &Opaque) -> Vec<u8> {
    buf.push(value.TypeCode);
    encode_uvarint(value.Buf.len() as u64, &mut buf);
    buf.extend_from_slice(&value.Buf);
    buf
}

fn appendBinaryFloat64(mut buf: Vec<u8>, value: f64) -> Vec<u8> {
    buf.extend_from_slice(&value.to_bits().to_le_bytes());
    buf
}

fn appendBinaryUint64(mut buf: Vec<u8>, value: u64) -> Vec<u8> {
    buf.extend_from_slice(&value.to_le_bytes());
    buf
}

fn appendBinaryUint32(mut buf: Vec<u8>, value: u32) -> Vec<u8> {
    buf.extend_from_slice(&value.to_le_bytes());
    buf
}

/// Protobuf 风格 uvarint 编码。
fn encode_uvarint(mut value: u64, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push(value as u8 | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

/// 解码 uvarint，返回 (值, 消耗字节数)；超长则失败。
fn decode_uvarint(input: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0_u64;
    for (index, byte) in input.iter().copied().enumerate().take(10) {
        if index == 9 && byte > 1 {
            return None;
        }
        value |= ((byte & 0x7f) as u64) << (index * 7);
        if byte < 0x80 {
            return Some((value, index + 1));
        }
    }
    None
}

fn read_u16(input: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(input[offset..offset + 2].try_into().expect("valid u16"))
}

fn read_u32(input: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(input[offset..offset + 4].try_into().expect("valid u32"))
}

fn read_u64(input: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(input[offset..offset + 8].try_into().expect("valid u64"))
}

fn write_u16(output: &mut [u8], offset: usize, value: u16) {
    output[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(output: &mut [u8], offset: usize, value: u32) {
    output[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
