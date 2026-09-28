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

// 序列化工具：按本机字节序把标量与结构化类型追加写入字节缓冲。
//
// 与 `deserialization_util` 对称；用于聚合 spill 落盘。变长载荷统一为
//「Go int 长度前缀 + 原始字节」；`SerializeInterface` 先写类型码再写载荷。

use crate::{common_util::*, types};
use std::any::Any;
use std::io::Cursor;

/// Go `time.Duration` 的 Rust 别名：纳秒级有符号 64 位整数。
pub type GoTimeDuration = i64;

/// 写入长度前缀（Go int）再追加原始字节。
fn serializeBuffer(value: &[u8], mut buf: Vec<u8>) -> Vec<u8> {
    let length = isize::try_from(value.len()).expect("buffer length fits Go int");
    buf = SerializeInt(length, buf);
    buf.extend_from_slice(value);
    buf
}

// SerializeByte serializes byte type.
/// 序列化单字节。
pub fn SerializeByte(value: u8, mut buf: Vec<u8>) -> Vec<u8> {
    buf.push(value);
    buf
}

// SerializeBool serializes bool type.
/// 序列化布尔：false→0，true→1。
pub fn SerializeBool(value: bool, mut buf: Vec<u8>) -> Vec<u8> {
    buf.push(u8::from(value));
    buf
}

// SerializeInt serializes int type.
/// 序列化 Go `int` / `isize`（本机字节序）。
pub fn SerializeInt(value: isize, mut buf: Vec<u8>) -> Vec<u8> {
    buf.extend_from_slice(&value.to_ne_bytes());
    buf
}

// SerializeInt8 serializes int8 type.
/// 序列化 i8。
pub fn SerializeInt8(value: i8, mut buf: Vec<u8>) -> Vec<u8> {
    buf.extend_from_slice(&value.to_ne_bytes());
    buf
}

// SerializeUint8 serializes uint8 type.
/// 序列化 u8。
pub fn SerializeUint8(value: u8, mut buf: Vec<u8>) -> Vec<u8> {
    buf.extend_from_slice(&value.to_ne_bytes());
    buf
}

// SerializeInt32 serializes int32 type.
/// 序列化 i32。
pub fn SerializeInt32(value: i32, mut buf: Vec<u8>) -> Vec<u8> {
    buf.extend_from_slice(&value.to_ne_bytes());
    buf
}

// SerializeUint32 serializes uint32 type.
/// 序列化 u32。
pub fn SerializeUint32(value: u32, mut buf: Vec<u8>) -> Vec<u8> {
    buf.extend_from_slice(&value.to_ne_bytes());
    buf
}

// SerializeUint64 serializes uint64 type.
/// 序列化 u64。
pub fn SerializeUint64(value: u64, mut buf: Vec<u8>) -> Vec<u8> {
    buf.extend_from_slice(&value.to_ne_bytes());
    buf
}

// SerializeInt64 serializes int64 type.
/// 序列化 i64。
pub fn SerializeInt64(value: i64, mut buf: Vec<u8>) -> Vec<u8> {
    buf.extend_from_slice(&value.to_ne_bytes());
    buf
}

// SerializeFloat32 serializes float32 type.
/// 序列化 f32。
pub fn SerializeFloat32(value: f32, mut buf: Vec<u8>) -> Vec<u8> {
    buf.extend_from_slice(&value.to_ne_bytes());
    buf
}

// SerializeFloat64 serializes float64 type.
/// 序列化 f64。
pub fn SerializeFloat64(value: f64, mut buf: Vec<u8>) -> Vec<u8> {
    buf.extend_from_slice(&value.to_ne_bytes());
    buf
}

// SerializeMyDecimal serializes MyDecimal type in the exact Go field order.
/// 按 Go 字段顺序写出 MyDecimal（定点数）。
pub fn SerializeMyDecimal(value: &types::MyDecimal, mut buf: Vec<u8>) -> Vec<u8> {
    buf.push(value.digitsInt as u8);
    buf.push(value.digitsFrac as u8);
    buf.push(value.resultFrac as u8);
    buf.push(u8::from(value.negative));
    for word in value.wordBuf {
        buf.extend_from_slice(&word.to_ne_bytes());
    }
    debug_assert_eq!(types::MyDecimalStructSize, 40);
    buf
}

// SerializeTime serializes Time type.
/// 序列化 `types::Time`（写出 CoreTime 的 u64）。
pub fn SerializeTime(value: types::Time, mut buf: Vec<u8>) -> Vec<u8> {
    buf.extend_from_slice(&value.coreTime.0.to_ne_bytes());
    buf
}

// SerializeGoTimeDuration serializes time.Duration type.
/// 序列化 Go `time.Duration`（纳秒 i64）。
pub fn SerializeGoTimeDuration(value: GoTimeDuration, mut buf: Vec<u8>) -> Vec<u8> {
    buf.extend_from_slice(&value.to_ne_bytes());
    buf
}

// SerializeTypesDuration serializes types.Duration type.
/// 序列化 `types::Duration`：时长 + 小数秒精度 Fsp。
pub fn SerializeTypesDuration(value: types::Duration, mut buf: Vec<u8>) -> Vec<u8> {
    buf = SerializeGoTimeDuration(value.Duration, buf);
    SerializeInt(value.Fsp as isize, buf)
}

// SerializeJSONTypeCode serializes JSONTypeCode type.
/// 序列化 JSON 类型码单字节。
pub fn SerializeJSONTypeCode(value: types::JSONTypeCode, mut buf: Vec<u8>) -> Vec<u8> {
    buf.push(value);
    buf
}

// SerializeBinaryJSON serializes BinaryJSON type.
/// 序列化 BinaryJSON：类型码 + 长度前缀载荷。
pub fn SerializeBinaryJSON(value: &types::BinaryJSON, mut buf: Vec<u8>) -> Vec<u8> {
    buf = SerializeJSONTypeCode(value.TypeCode, buf);
    serializeBuffer(&value.Value, buf)
}

// SerializeSet serializes Set type.
/// 序列化 SET（位图值 + 名称字节）。
pub fn SerializeSet(value: &types::Set, mut buf: Vec<u8>) -> Vec<u8> {
    buf = SerializeUint64(value.Value, buf);
    serializeBuffer(value.Name.as_bytes(), buf)
}

// SerializeEnum serializes Enum type.
/// 序列化 ENUM（序号 + 名称字节）。
pub fn SerializeEnum(value: &types::Enum, mut buf: Vec<u8>) -> Vec<u8> {
    buf = SerializeUint64(value.Value, buf);
    serializeBuffer(value.Name.as_bytes(), buf)
}

// SerializeOpaque serializes Opaque type.
/// 序列化 Opaque：类型码 + 原始字节缓冲。
pub fn SerializeOpaque(value: types::Opaque, mut buf: Vec<u8>) -> Vec<u8> {
    buf = SerializeByte(value.TypeCode, buf);
    serializeBuffer(&value.Buf, buf)
}

// SerializeString serializes String type.
/// 序列化字符串（UTF-8 字节 + 长度前缀）。
pub fn SerializeString(value: &str, buf: Vec<u8>) -> Vec<u8> {
    serializeBuffer(value.as_bytes(), buf)
}

// SerializeBytesBuffer serializes the unread portion of a bytes.Buffer.
/// 序列化 `Cursor` 当前位置之后的未读区间，对齐 Go `bytes.Buffer`。
pub fn SerializeBytesBuffer(value: &Cursor<Vec<u8>>, buf: Vec<u8>) -> Vec<u8> {
    let start = usize::try_from(value.position()).expect("cursor position fits usize");
    serializeBuffer(&value.get_ref()[start..], buf)
}

// SerializeInterface serializes every concrete type supported by the Go type switch.
/// 按具体类型写类型码再写载荷；不支持的类型 panic。
pub fn SerializeInterface(value: &dyn Any, mut buf: Vec<u8>) -> Vec<u8> {
    if let Some(value) = value.downcast_ref::<bool>() {
        buf.push(BoolType as u8);
        return SerializeBool(*value, buf);
    }
    if let Some(value) = value.downcast_ref::<i64>() {
        buf.push(Int64Type as u8);
        return SerializeInt64(*value, buf);
    }
    if let Some(value) = value.downcast_ref::<u64>() {
        buf.push(Uint64Type as u8);
        return SerializeUint64(*value, buf);
    }
    if let Some(value) = value.downcast_ref::<f64>() {
        buf.push(FloatType as u8);
        return SerializeFloat64(*value, buf);
    }
    if let Some(value) = value.downcast_ref::<String>() {
        buf.push(StringType as u8);
        return SerializeString(value, buf);
    }
    if let Some(value) = value.downcast_ref::<types::BinaryJSON>() {
        buf.push(BinaryJSONType as u8);
        return SerializeBinaryJSON(value, buf);
    }
    if let Some(value) = value.downcast_ref::<types::Opaque>() {
        buf.push(OpaqueType as u8);
        return SerializeOpaque(value.clone(), buf);
    }
    if let Some(value) = value.downcast_ref::<types::Time>() {
        buf.push(TimeType as u8);
        return SerializeTime(*value, buf);
    }
    if let Some(value) = value.downcast_ref::<types::Duration>() {
        buf.push(DurationType as u8);
        return SerializeTypesDuration(*value, buf);
    }
    panic!("Agg spill encounters an unexpected interface type!");
}
