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

// serialization 迁移回归测试：往返编解码、长度前缀与接口分派。
//
// 校验与 Go 一致的类型码、本机布局、字段顺序与 panic 文案，
// 确保 spill 恢复路径行为不漂移。

use super::*;
use std::io::{Cursor, Read};

/// 构造从偏移 0 开始的反序列化游标。
fn input(buf: Vec<u8>) -> PosAndBuf {
    PosAndBuf { Buf: buf, Pos: 0 }
}

/// 校验类型码序号与定长常量与 Go/本机布局一致。
#[test]
fn constants_match_go_type_codes_and_native_layout() {
    assert_eq!(
        [
            BoolType,
            Int64Type,
            Uint64Type,
            FloatType,
            StringType,
            BinaryJSONType,
            OpaqueType,
            TimeType,
            DurationType,
        ],
        [0, 1, 2, 3, 4, 5, 6, 7, 8]
    );
    assert_eq!(InterfaceTypeCodeLen, 1);
    assert_eq!(JSONTypeCodeLen, 1);
    assert_eq!(BoolLen, 1);
    assert_eq!(ByteLen, 1);
    assert_eq!(IntLen as usize, std::mem::size_of::<isize>());
    assert_eq!(TimeLen, 8);
    assert_eq!(TimeDurationLen, 8);
}

/// 标量往返：编码后解码值不变，且 `Pos` 恰好走到缓冲末尾。
#[test]
fn primitive_round_trips_preserve_go_native_bytes_and_position() {
    let mut buf = Vec::new();
    buf = SerializeByte(0xa5, buf);
    buf = SerializeBool(true, buf);
    buf = SerializeInt(-17, buf);
    buf = SerializeInt8(-8, buf);
    buf = SerializeUint8(250, buf);
    buf = SerializeInt32(-123_456, buf);
    buf = SerializeUint32(3_000_000_000, buf);
    buf = SerializeInt64(-9_876_543_210, buf);
    buf = SerializeUint64(18_000_000_000, buf);
    buf = SerializeFloat32(-12.5, buf);
    buf = SerializeFloat64(1234.25, buf);

    let mut pos = input(buf.clone());
    assert_eq!(DeserializeByte(&mut pos), 0xa5);
    assert!(DeserializeBool(&mut pos));
    assert_eq!(DeserializeInt(&mut pos), -17);
    assert_eq!(DeserializeInt8(&mut pos), -8);
    assert_eq!(DeserializeUint8(&mut pos), 250);
    assert_eq!(DeserializeInt32(&mut pos), -123_456);
    assert_eq!(DeserializeUint32(&mut pos), 3_000_000_000);
    assert_eq!(DeserializeInt64(&mut pos), -9_876_543_210);
    assert_eq!(DeserializeUint64(&mut pos), 18_000_000_000);
    assert_eq!(DeserializeFloat32(&mut pos), -12.5);
    assert_eq!(DeserializeFloat64(&mut pos), 1234.25);
    assert_eq!(pos.Pos as usize, buf.len());
}

/// 长度前缀字符串、Cursor 未读区间与 Column.Reset 行为对齐 Go。
#[test]
fn length_prefixed_values_and_cursor_match_go_buffer_behavior() {
    let prefix = vec![9, 8];
    let string_buf = SerializeString("hello", prefix.clone());
    assert_eq!(&string_buf[..2], prefix.as_slice());
    assert_eq!(
        &string_buf[2..2 + IntLen as usize],
        &(5_isize).to_ne_bytes()
    );
    let mut pos = PosAndBuf {
        Buf: string_buf,
        Pos: 2,
    };
    assert_eq!(DeserializeString(&mut pos), "hello");

    let mut source = Cursor::new(b"already-read:remaining".to_vec());
    source.set_position("already-read:".len() as u64);
    let encoded = SerializeBytesBuffer(&source, Vec::new());
    let mut pos = input(encoded);
    let mut decoded = DeserializeBytesBuffer(&mut pos);
    let mut text = String::new();
    decoded.read_to_string(&mut text).unwrap();
    assert_eq!(text, "remaining");

    let mut column = chunk::Column::default();
    column.offsets = vec![0, 3];
    column.data = b"row".to_vec();
    let mut reset = PosAndBuf {
        Buf: b"stale".to_vec(),
        Pos: 4,
    };
    reset.Reset(&column, 0);
    assert_eq!(reset, input(b"row".to_vec()));
}

/// 结构化类型按 Go 字段顺序往返（MyDecimal/Time/Duration/JSON 等）。
#[test]
fn structured_values_round_trip_with_go_field_order() {
    let decimal = types::MyDecimal {
        digitsInt: 9,
        digitsFrac: 4,
        resultFrac: 3,
        negative: true,
        wordBuf: [1, 22, 333, 4_444, 55_555, 6, 7, 8, 9],
    };
    let time = types::Time {
        coreTime: types::CoreTime(0x1234_5678_90ab_cdef),
    };
    let duration = types::Duration {
        Duration: -3_600_000_000_123,
        Fsp: 6,
    };
    let json = types::BinaryJSON {
        TypeCode: 0x0c,
        Value: b"json payload".to_vec(),
    };
    let opaque = types::Opaque {
        TypeCode: 253,
        Buf: vec![0, 1, 2, 255],
    };
    let set = types::Set {
        Value: 5,
        Name: "red,blue".to_owned(),
    };
    let enum_value = types::Enum {
        Value: 2,
        Name: "medium".to_owned(),
    };

    let mut buf = SerializeMyDecimal(&decimal, Vec::new());
    buf = SerializeTime(time, buf);
    buf = SerializeTypesDuration(duration, buf);
    buf = SerializeBinaryJSON(&json, buf);
    buf = SerializeOpaque(opaque.clone(), buf);
    buf = SerializeSet(&set, buf);
    buf = SerializeEnum(&enum_value, buf);

    assert_eq!(&buf[..4], &[9, 4, 3, 1]);
    let mut pos = input(buf.clone());
    assert_eq!(DeserializeMyDecimal(&mut pos), decimal);
    assert_eq!(DeserializeTime(&mut pos), time);
    assert_eq!(DeserializeTypesDuration(&mut pos), duration);
    assert_eq!(DeserializeBinaryJSON(&mut pos), json);
    assert_eq!(DeserializeOpaque(&mut pos), opaque);
    assert_eq!(DeserializeSet(&mut pos), set);
    assert_eq!(DeserializeEnum(&mut pos), enum_value);
    assert_eq!(pos.Pos as usize, buf.len());
}

/// 接口分派：每种 Go 支持的具体类型都能正确往返。
#[test]
fn interface_dispatch_round_trips_every_go_supported_variant() {
    let values: Vec<Box<dyn std::any::Any>> = vec![
        Box::new(true),
        Box::new(-42_i64),
        Box::new(42_u64),
        Box::new(3.5_f64),
        Box::new("spill".to_owned()),
        Box::new(types::BinaryJSON {
            TypeCode: 0x0c,
            Value: b"json".to_vec(),
        }),
        Box::new(types::Opaque {
            TypeCode: 7,
            Buf: b"opaque".to_vec(),
        }),
        Box::new(types::Time {
            coreTime: types::CoreTime(123),
        }),
        Box::new(types::Duration {
            Duration: -99,
            Fsp: 2,
        }),
    ];

    let mut encoded = Vec::new();
    for (expected_type, value) in values.iter().enumerate() {
        let one = SerializeInterface(value.as_ref(), Vec::new());
        assert_eq!(one[0], expected_type as u8);
        encoded.extend(one);
    }

    let mut pos = input(encoded.clone());
    assert!(matches!(
        DeserializeInterface(&mut pos),
        DeserializedInterface::Bool(true)
    ));
    assert!(matches!(
        DeserializeInterface(&mut pos),
        DeserializedInterface::Int64(-42)
    ));
    assert!(matches!(
        DeserializeInterface(&mut pos),
        DeserializedInterface::Uint64(42)
    ));
    assert!(matches!(
        DeserializeInterface(&mut pos),
        DeserializedInterface::Float64(value) if value == 3.5
    ));
    assert!(matches!(
        DeserializeInterface(&mut pos),
        DeserializedInterface::String(value) if value == "spill"
    ));
    assert!(matches!(
        DeserializeInterface(&mut pos),
        DeserializedInterface::BinaryJSON(value) if value.Value == b"json"
    ));
    assert!(matches!(
        DeserializeInterface(&mut pos),
        DeserializedInterface::Opaque(value) if value.Buf == b"opaque"
    ));
    assert!(matches!(
        DeserializeInterface(&mut pos),
        DeserializedInterface::Time(value) if value.coreTime == types::CoreTime(123)
    ));
    assert!(matches!(
        DeserializeInterface(&mut pos),
        DeserializedInterface::Duration(value) if value.Duration == -99 && value.Fsp == 2
    ));
    assert_eq!(pos.Pos as usize, encoded.len());
}

/// 非法接口类型码应复现 Go 侧反序列化 panic 文案。
#[test]
#[should_panic(expected = "Invalid data type happens in agg spill deserializing!")]
fn invalid_interface_type_keeps_go_panic() {
    DeserializeInterface(&mut input(vec![255]));
}

/// 不支持的具体类型应复现 Go 侧序列化 panic 文案。
#[test]
#[should_panic(expected = "Agg spill encounters an unexpected interface type!")]
fn invalid_interface_value_keeps_go_panic() {
    SerializeInterface(&123_i32, Vec::new());
}

/// 缓冲不足时 panic，而不是越界读取。
#[test]
#[should_panic]
fn truncated_scalar_panics_instead_of_reading_past_the_buffer() {
    DeserializeUint64(&mut input(vec![1, 2, 3]));
}
