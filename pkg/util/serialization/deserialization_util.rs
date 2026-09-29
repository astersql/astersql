// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 反序列化工具：从字节缓冲按本机字节序读出标量与结构化类型。
//
// 与 `serialization_util` 对称；用于聚合 spill 恢复。所有 `DeserializeXXX`
// 通过 `PosAndBuf` 推进读指针；缓冲截断或非法类型码会 panic，对齐 Go 行为。

use crate::{chunk, common_util::*, types};
use std::io::Cursor;

/// Go `time.Duration` 的 Rust 别名：纳秒级有符号 64 位整数。
pub type GoTimeDuration = i64;

// PosAndBuf is the parameter of all DeserializeXXX functions.
/// 反序列化游标：持有字节缓冲与当前读位置（字节偏移）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PosAndBuf {
    /// 待解析的原始字节。
    pub Buf: Vec<u8>,
    /// 当前读位置（从 0 起，按字节推进）。
    pub Pos: i64,
}

impl PosAndBuf {
    // Reset resets data in PosAndBuf.
    /// 从列（Column）第 `idx` 行取出字节，重置缓冲与读位置。
    pub fn Reset(&mut self, col: &chunk::Column, idx: usize) {
        self.Buf = col.GetBytes(idx).to_vec();
        self.Pos = 0;
    }
}

/// 从游标取出固定 N 字节并推进 `Pos`；不足则 panic。
fn take<const N: usize>(pos_and_buf: &mut PosAndBuf) -> [u8; N] {
    let start = usize::try_from(pos_and_buf.Pos).expect("serialization position is non-negative");
    let end = start
        .checked_add(N)
        .expect("serialization position overflow");
    let value = pos_and_buf
        .Buf
        .get(start..end)
        .expect("serialized buffer is truncated")
        .try_into()
        .expect("slice length matches requested value");
    pos_and_buf.Pos = i64::try_from(end).expect("serialization position fits i64");
    value
}

/// 先读长度前缀（Go int），再切出对应字节切片并推进 `Pos`。
fn deserializeBuffer(pos_and_buf: &mut PosAndBuf) -> &[u8] {
    let buf_len = DeserializeInt(pos_and_buf);
    let buf_len = usize::try_from(buf_len).expect("serialized buffer length is non-negative");
    let start = usize::try_from(pos_and_buf.Pos).expect("serialization position is non-negative");
    let end = start
        .checked_add(buf_len)
        .expect("serialized buffer length overflow");
    if end > pos_and_buf.Buf.len() {
        panic!("serialized buffer is truncated");
    }
    pos_and_buf.Pos = i64::try_from(end).expect("serialization position fits i64");
    &pos_and_buf.Buf[start..end]
}

// DeserializeByte deserializes byte type.
/// 反序列化单字节。
pub fn DeserializeByte(pos_and_buf: &mut PosAndBuf) -> u8 {
    take::<1>(pos_and_buf)[0]
}

// DeserializeBool deserializes bool type.
/// 反序列化布尔：0=false，1=true，其它值 panic。
pub fn DeserializeBool(pos_and_buf: &mut PosAndBuf) -> bool {
    match DeserializeByte(pos_and_buf) {
        0 => false,
        1 => true,
        _ => panic!("invalid serialized bool"),
    }
}

// DeserializeInt deserializes int type.
/// 反序列化 Go `int` / `isize`（本机字节序）。
pub fn DeserializeInt(pos_and_buf: &mut PosAndBuf) -> isize {
    isize::from_ne_bytes(take(pos_and_buf))
}

// DeserializeInt8 deserializes int8 type.
/// 反序列化 i8。
pub fn DeserializeInt8(pos_and_buf: &mut PosAndBuf) -> i8 {
    i8::from_ne_bytes(take(pos_and_buf))
}

// DeserializeUint8 deserializes uint8 type.
/// 反序列化 u8。
pub fn DeserializeUint8(pos_and_buf: &mut PosAndBuf) -> u8 {
    u8::from_ne_bytes(take(pos_and_buf))
}

// DeserializeInt32 deserializes int32 type.
/// 反序列化 i32。
pub fn DeserializeInt32(pos_and_buf: &mut PosAndBuf) -> i32 {
    i32::from_ne_bytes(take(pos_and_buf))
}

// DeserializeUint32 deserializes uint32 type.
/// 反序列化 u32。
pub fn DeserializeUint32(pos_and_buf: &mut PosAndBuf) -> u32 {
    u32::from_ne_bytes(take(pos_and_buf))
}

// DeserializeUint64 deserializes uint64 type.
/// 反序列化 u64。
pub fn DeserializeUint64(pos_and_buf: &mut PosAndBuf) -> u64 {
    u64::from_ne_bytes(take(pos_and_buf))
}

// DeserializeInt64 deserializes int64 type.
/// 反序列化 i64。
pub fn DeserializeInt64(pos_and_buf: &mut PosAndBuf) -> i64 {
    i64::from_ne_bytes(take(pos_and_buf))
}

// DeserializeFloat32 deserializes float32 type.
/// 反序列化 f32。
pub fn DeserializeFloat32(pos_and_buf: &mut PosAndBuf) -> f32 {
    f32::from_ne_bytes(take(pos_and_buf))
}

// DeserializeFloat64 deserializes float64 type.
/// 反序列化 f64。
pub fn DeserializeFloat64(pos_and_buf: &mut PosAndBuf) -> f64 {
    f64::from_ne_bytes(take(pos_and_buf))
}

// DeserializeMyDecimal deserializes MyDecimal using the exact Go field order.
/// 按 Go 字段顺序还原 MyDecimal（定点数，用于精确十进制运算）。
pub fn DeserializeMyDecimal(pos_and_buf: &mut PosAndBuf) -> types::MyDecimal {
    let digits_int = DeserializeInt8(pos_and_buf);
    let digits_frac = DeserializeInt8(pos_and_buf);
    let result_frac = DeserializeInt8(pos_and_buf);
    let negative = DeserializeBool(pos_and_buf);
    let mut words = [0_i32; 9];
    for word in &mut words {
        *word = DeserializeInt32(pos_and_buf);
    }
    debug_assert_eq!(types::MyDecimalStructSize, 40);
    types::MyDecimal {
        digitsInt: digits_int,
        digitsFrac: digits_frac,
        resultFrac: result_frac,
        negative,
        wordBuf: words,
    }
}

// DeserializeTime deserializes Time type.
/// 反序列化 `types::Time`（内部为打包的 CoreTime u64）。
pub fn DeserializeTime(pos_and_buf: &mut PosAndBuf) -> types::Time {
    types::Time {
        coreTime: types::CoreTime(DeserializeUint64(pos_and_buf)),
    }
}

// DeserializeTimeDuration deserializes time.Duration type.
/// 反序列化 Go `time.Duration`（纳秒 i64）。
pub fn DeserializeTimeDuration(pos_and_buf: &mut PosAndBuf) -> GoTimeDuration {
    DeserializeInt64(pos_and_buf)
}

// DeserializeTypesDuration deserializes types.Duration type.
/// 反序列化 `types::Duration`：时长 + 小数秒精度 Fsp。
pub fn DeserializeTypesDuration(pos_and_buf: &mut PosAndBuf) -> types::Duration {
    types::Duration {
        Duration: DeserializeTimeDuration(pos_and_buf),
        Fsp: i32::try_from(DeserializeInt(pos_and_buf)).expect("duration Fsp fits i32"),
    }
}

// DeserializeVectorFloat32 deserializes VectorFloat32 type.
/// 读取长度前缀、复制独立载荷，并按 Go 向量线格式解码；非法载荷 panic。
pub fn DeserializeVectorFloat32(pos_and_buf: &mut PosAndBuf) -> types::VectorFloat32 {
    let bytes = deserializeBuffer(pos_and_buf).to_vec();
    let (vector, _) =
        types::ZeroCopyDeserializeVectorFloat32(&bytes).unwrap_or_else(|error| panic!("{error}"));
    vector
}

// DeserializeJSONTypeCode deserializes JSONTypeCode type.
/// 反序列化 JSON 类型码单字节。
pub fn DeserializeJSONTypeCode(pos_and_buf: &mut PosAndBuf) -> types::JSONTypeCode {
    DeserializeByte(pos_and_buf)
}

// DeserializeBinaryJSON deserializes BinaryJSON type.
/// 反序列化 BinaryJSON：类型码 + 长度前缀载荷。
pub fn DeserializeBinaryJSON(pos_and_buf: &mut PosAndBuf) -> types::BinaryJSON {
    let type_code = DeserializeJSONTypeCode(pos_and_buf);
    let value = deserializeBuffer(pos_and_buf).to_vec();
    types::BinaryJSON {
        TypeCode: type_code,
        Value: value,
    }
}

// DeserializeSet deserializes Set type.
/// 反序列化 SET 类型（位图值 + 名称字符串）。
pub fn DeserializeSet(pos_and_buf: &mut PosAndBuf) -> types::Set {
    types::Set {
        Value: DeserializeUint64(pos_and_buf),
        Name: DeserializeString(pos_and_buf),
    }
}

// DeserializeEnum deserializes Enum type.
/// 反序列化 ENUM 类型（序号 + 名称字符串）。
pub fn DeserializeEnum(pos_and_buf: &mut PosAndBuf) -> types::Enum {
    types::Enum {
        Value: DeserializeUint64(pos_and_buf),
        Name: DeserializeString(pos_and_buf),
    }
}

// DeserializeOpaque deserializes Opaque type.
/// 反序列化 Opaque：类型码 + 原始字节缓冲。
pub fn DeserializeOpaque(pos_and_buf: &mut PosAndBuf) -> types::Opaque {
    let type_code = DeserializeByte(pos_and_buf);
    let buf = deserializeBuffer(pos_and_buf).to_vec();
    types::Opaque {
        TypeCode: type_code,
        Buf: buf,
    }
}

// DeserializeString deserializes String type.
/// 反序列化 UTF-8 字符串（长度前缀 + 字节）；非法 UTF-8 会 panic。
pub fn DeserializeString(pos_and_buf: &mut PosAndBuf) -> String {
    String::from_utf8(deserializeBuffer(pos_and_buf).to_vec())
        .expect("serialized Rust string must contain valid UTF-8")
}

// DeserializeBytesBuffer deserializes bytes.Buffer type.
/// 反序列化为 `Cursor<Vec<u8>>`，对应 Go `bytes.Buffer` 未读内容。
pub fn DeserializeBytesBuffer(pos_and_buf: &mut PosAndBuf) -> Cursor<Vec<u8>> {
    Cursor::new(deserializeBuffer(pos_and_buf).to_vec())
}

/// `DeserializeInterface` 的结果枚举，覆盖 Go type switch 支持的全部变体。
#[derive(Clone, Debug, PartialEq)]
pub enum DeserializedInterface {
    /// 布尔。
    Bool(bool),
    /// 有符号 64 位整数。
    Int64(i64),
    /// 无符号 64 位整数。
    Uint64(u64),
    /// 双精度浮点。
    Float64(f64),
    /// 字符串。
    String(String),
    /// 二进制 JSON。
    BinaryJSON(types::BinaryJSON),
    /// 不透明载荷。
    Opaque(types::Opaque),
    /// 时间。
    Time(types::Time),
    /// 时长。
    Duration(types::Duration),
}

// DeserializeInterface deserializes every concrete type supported by the Go switch.
/// 先读类型码，再按码分派到对应反序列化函数；未知类型码 panic。
pub fn DeserializeInterface(pos_and_buf: &mut PosAndBuf) -> DeserializedInterface {
    let data_type = DeserializeByte(pos_and_buf) as i64;
    match data_type {
        BoolType => DeserializedInterface::Bool(DeserializeBool(pos_and_buf)),
        Int64Type => DeserializedInterface::Int64(DeserializeInt64(pos_and_buf)),
        Uint64Type => DeserializedInterface::Uint64(DeserializeUint64(pos_and_buf)),
        FloatType => DeserializedInterface::Float64(DeserializeFloat64(pos_and_buf)),
        StringType => DeserializedInterface::String(DeserializeString(pos_and_buf)),
        BinaryJSONType => DeserializedInterface::BinaryJSON(DeserializeBinaryJSON(pos_and_buf)),
        OpaqueType => DeserializedInterface::Opaque(DeserializeOpaque(pos_and_buf)),
        TimeType => DeserializedInterface::Time(DeserializeTime(pos_and_buf)),
        DurationType => DeserializedInterface::Duration(DeserializeTypesDuration(pos_and_buf)),
        _ => panic!("Invalid data type happens in agg spill deserializing!"),
    }
}
