// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// VectorFloat32 基础行为单元测试：端序、零向量、解析、Datum 与序列化。
//
// 对齐 Go `types` 包向量相关测试，校验小端线格式与 Compare/Parse 语义。

#![allow(non_snake_case)]

use crate::datum::NewDatum;
use crate::vector::{
    InitVectorFloat32, ParseVectorFloat32, PeekBytesAsVectorFloat32,
    ZeroCopyDeserializeVectorFloat32, ZeroVectorFloat32,
};

#[test]
fn go_merge_10_vector_dimension_overflow_is_rejected() {
    let header = [0, 0, 0, 0x40];
    let expected = "bad VectorFloat32 value (len=4, expected=4294967300)";
    assert_eq!(
        PeekBytesAsVectorFloat32(&header).unwrap_err().to_string(),
        expected
    );
    assert_eq!(
        ZeroCopyDeserializeVectorFloat32(&header)
            .unwrap_err()
            .to_string(),
        expected
    );
}

/// 校验小端序列化布局：维度字 + 两个 f32 字。
#[test]
fn TestVectorEndianess() {
    let mut v = InitVectorFloat32(2);
    let vv = v.ElementsMut();
    vv[0] = 1.1;
    vv[1] = 2.2;
    assert_eq!(
        v.SerializeTo(Vec::new()),
        vec![
            0x02, 0x00, 0x00, 0x00, 0xcd, 0xcc, 0x8c, 0x3f, 0xcd, 0xcc, 0x0c, 0x40,
        ]
    );
}

/// 零维向量的序列化、反序列化与比较行为。
#[test]
fn TestZeroVector() {
    let zero = ZeroVectorFloat32();
    assert!(zero.IsZeroValue());
    assert_eq!(zero.Compare(&ZeroVectorFloat32()), 0);
    assert_eq!(zero.ZeroCopySerialize(), &[0, 0, 0, 0]);
    assert_eq!(zero.SerializedSize(), 4);
    assert_eq!(zero.SerializeTo(Vec::new()), vec![0, 0, 0, 0]);
    assert_eq!(zero.SerializeTo(vec![1, 2, 3]), vec![1, 2, 3, 0, 0, 0, 0]);

    let (v, remaining) = ZeroCopyDeserializeVectorFloat32(&[0, 0, 0, 0]).unwrap();
    assert!(remaining.is_empty());
    assert_eq!(v.Len(), 0);
    assert_eq!(v.String(), "[]");
    assert!(v.IsZeroValue());
    assert_eq!(v.Compare(&ZeroVectorFloat32()), 0);
    assert_eq!(ZeroVectorFloat32().Compare(&v), 0);
}

/// 非法文本应失败；合法 JSON 数组可解析并参与比较。
#[test]
fn TestVectorParse() {
    for input in [
        "abc",
        "null",
        "\"json_str\"",
        "123",
        "[123",
        "123]",
        "[123,]",
    ] {
        let error = ParseVectorFloat32(input).unwrap_err();
        assert!(error.to_string().contains("Invalid vector text"));
    }

    let v = ParseVectorFloat32("[]").unwrap();
    assert_eq!(v.Len(), 0);
    assert_eq!(v.String(), "[]");
    assert!(v.IsZeroValue());
    assert_eq!(v.Compare(&ZeroVectorFloat32()), 0);
    assert_eq!(ZeroVectorFloat32().Compare(&v), 0);

    let v = ParseVectorFloat32("[1.1, 2.2, 3.3]").unwrap();
    assert_eq!(v.Len(), 3);
    assert_eq!(v.String(), "[1.1,2.2,3.3]");
    assert!(!v.IsZeroValue());
    assert_eq!(v.Compare(&ZeroVectorFloat32()), 1);
    assert_eq!(ZeroVectorFloat32().Compare(&v), -1);

    // 超出 f32 范围；尾部多余字符视为非法
    assert_eq!(
        ParseVectorFloat32("[-1e39, 1e39]").unwrap_err().to_string(),
        "value -1e+39 out of range for float32"
    );
    for input in ["[1,2,3,4.4]ddddddddddddfasfa", "[1,2,3]extra"] {
        let error = ParseVectorFloat32(input).unwrap_err();
        assert!(error.to_string().contains("Invalid vector text"));
    }
}

/// Datum 承载 VectorFloat32 的读写往返。
#[test]
fn TestVectorDatum() {
    let mut datum = NewDatum(&());
    datum.SetVectorFloat32(ZeroVectorFloat32());
    let v = datum.GetVectorFloat32();
    assert_eq!(v.Len(), 0);
    assert_eq!(v.String(), "[]");
    assert!(v.IsZeroValue());
    assert_eq!(v.Compare(&ZeroVectorFloat32()), 0);
    assert_eq!(ZeroVectorFloat32().Compare(&v), 0);
}

/// 字典序比较：先比公共前缀分量，再比维度。
#[test]
fn TestVectorCompare() {
    let v1 = ParseVectorFloat32("[1.1, 2.2, 3.3]").unwrap();
    let v2 = ParseVectorFloat32("[-1.1, 4.2]").unwrap();
    assert_eq!(v1.Compare(&v2), 1);
    assert_eq!(v2.Compare(&v1), -1);

    let v1 = ParseVectorFloat32("[1.1, 2.2, 3.3]").unwrap();
    let v2 = ParseVectorFloat32("[1.1, 4.2]").unwrap();
    assert_eq!(v1.Compare(&v2), -1);
    assert_eq!(v2.Compare(&v1), 1);
}

/// 序列化后追加尾字节，反序列化应保留未消费后缀；畸形头报错。
#[test]
fn TestVectorSerialize() {
    let v1 = ParseVectorFloat32("[1.1, 2.2, 3.3]").unwrap();
    let mut serialized = v1.SerializeTo(Vec::new());
    serialized.extend_from_slice(&[0x01, 0x02, 0x03, 0x04]);

    let (v2, remaining) = ZeroCopyDeserializeVectorFloat32(&serialized).unwrap();
    assert_eq!(remaining.len(), 4);
    assert_eq!(remaining, &[0x01, 0x02, 0x03, 0x04]);
    assert_eq!(v2.String(), "[1.1,2.2,3.3]");

    let malformed = [0xf1, 0xfc];
    let error = ZeroCopyDeserializeVectorFloat32(&malformed).unwrap_err();
    assert_eq!(malformed, [0xf1, 0xfc]);
    assert!(error.to_string().contains("bad VectorFloat32 value header"));
}
