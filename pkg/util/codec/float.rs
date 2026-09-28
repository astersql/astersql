// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 浮点数编解码：将 f64 转为可按字节序升序/降序比较的 8 字节表示。
//
// 对应 Go `pkg/util/codec/float.go`。先把 IEEE754 bits 映射为可比较的 u64
// （正数置符号位、负数取反），再复用无符号整数编解码；Desc 变体对可比 uint 再取反。

use crate::{DecodeUint, DecodeUintDesc, EncodeUint, EncodeUintDesc, errors, signMask};

/// 把 IEEE754 bits 转成按字节序可比较的 uint64。
// encodeFloatToCmpUint64 对应 Go 的内部函数：把 IEEE754 bits 转成按字节序可比较的 uint64。
fn encodeFloatToCmpUint64(f: f64) -> u64 {
    let mut u = f.to_bits();
    if f >= 0.0 {
        // 正数设置最高位，使所有非负数排在负数之后且保持原 bits 顺序。
        u |= signMask;
    } else {
        // 负数整体取反，使越小的负数编码越小。
        u = !u;
    }
    u
}

/// encodeFloatToCmpUint64 的反变换：按最高位判断是否曾取反。
// decodeCmpUintToFloat 是 encodeFloatToCmpUint64 的反变换，保留 Go 的最高位判断。
fn decodeCmpUintToFloat(mut u: u64) -> f64 {
    if u & signMask > 0 {
        u &= !signMask;
    } else {
        u = !u;
    }
    f64::from_bits(u)
}

/// 升序可比较地编码 float：先转可比 uint，再 EncodeUint。
// EncodeFloat encodes a float v into a byte slice which can be sorted lexicographically later.
// EncodeFloat guarantees that the encoded value is in ascending order for comparison.
pub fn EncodeFloat(b: Vec<u8>, v: f64) -> Vec<u8> {
    let u = encodeFloatToCmpUint64(v);
    EncodeUint(b, u)
}

/// 解码 EncodeFloat 写出的字节，返回剩余切片与 f64。
// DecodeFloat decodes a float from a byte slice generated with EncodeFloat before.
pub fn DecodeFloat(b: &[u8]) -> Result<(&[u8], f64), errors::SharedError> {
    let (remain, u) = DecodeUint(b).map_err(|error| {
        errors::Trace(Some(error)).expect("tracing a present error preserves it")
    })?;
    Ok((remain, decodeCmpUintToFloat(u)))
}

/// 降序可比较地编码 float：可比 uint 经 EncodeUintDesc 写出。
// EncodeFloatDesc encodes a float v into a byte slice which can be sorted lexicographically later.
// EncodeFloatDesc guarantees that the encoded value is in descending order for comparison.
pub fn EncodeFloatDesc(b: Vec<u8>, v: f64) -> Vec<u8> {
    let u = encodeFloatToCmpUint64(v);
    EncodeUintDesc(b, u)
}

/// 解码 EncodeFloatDesc 写出的字节。
// DecodeFloatDesc decodes a float from a byte slice generated with EncodeFloatDesc before.
pub fn DecodeFloatDesc(b: &[u8]) -> Result<(&[u8], f64), errors::SharedError> {
    let (remain, u) = DecodeUintDesc(b).map_err(|error| {
        errors::Trace(Some(error)).expect("tracing a present error preserves it")
    })?;
    Ok((remain, decodeCmpUintToFloat(u)))
}
