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

// 整数编解码：定长有符号/无符号、变长 varint，以及 memcomparable 变长整数。
//
// 对应 Go `pkg/util/codec/number.go`。定长编码用大端 8 字节；有符号通过异或
// `signMask` 映射到无符号比较域。Desc 变体对编码结果按位取反以实现降序。
// Comparable* 系列用标签字节区分负数长度与正数长度，保证字节序与数值序一致。

use crate::errors;

/// 符号位掩码：把有符号整数最高位翻转后映射到无符号比较空间。
pub const signMask: u64 = 0x8000000000000000;

/// 将有符号整数映射为可比较的无符号值（最高位异或）。
// EncodeIntToCmpUint make int v to comparable uint type.
// Go 用最高位异或把有符号整数映射到无符号比较域。
pub fn EncodeIntToCmpUint(v: i64) -> u64 {
    (v as u64) ^ signMask
}

/// EncodeIntToCmpUint 的逆变换。
// DecodeCmpUintToInt decodes the u that encoded by EncodeIntToCmpUint.
pub fn DecodeCmpUintToInt(u: u64) -> i64 {
    (u ^ signMask) as i64
}

/// 升序可比较地编码有符号整数（大端 8 字节）。
// EncodeInt appends the encoded value to slice b and returns the appended slice.
// EncodeInt guarantees that the encoded value is in ascending order for comparison.
pub fn EncodeInt(mut b: Vec<u8>, v: i64) -> Vec<u8> {
    let u = EncodeIntToCmpUint(v);
    b.extend_from_slice(&u.to_be_bytes());
    b
}

/// 降序可比较地编码有符号整数（对可比 uint 取反后写大端）。
// EncodeIntDesc appends the encoded value to slice b and returns the appended slice.
// EncodeIntDesc guarantees that the encoded value is in descending order for comparison.
pub fn EncodeIntDesc(mut b: Vec<u8>, v: i64) -> Vec<u8> {
    let u = EncodeIntToCmpUint(v);
    b.extend_from_slice(&(!u).to_be_bytes());
    b
}

/// 解码 EncodeInt 写出的 8 字节，返回剩余切片与 i64。
// DecodeInt decodes value encoded by EncodeInt before.
// It returns the leftover un-decoded slice, decoded value if no error.
pub fn DecodeInt(b: &[u8]) -> Result<(&[u8], i64), errors::SharedError> {
    if b.len() < 8 {
        return Err(errors::New("insufficient bytes to decode value"));
    }
    let u = u64::from_be_bytes(b[0..8].try_into().unwrap());
    let v = DecodeCmpUintToInt(u);
    Ok((&b[8..], v))
}

/// 解码 EncodeIntDesc 写出的 8 字节。
// DecodeIntDesc decodes value encoded by EncodeInt before.
// It returns the leftover un-decoded slice, decoded value if no error.
pub fn DecodeIntDesc(b: &[u8]) -> Result<(&[u8], i64), errors::SharedError> {
    if b.len() < 8 {
        return Err(errors::New("insufficient bytes to decode value"));
    }
    let u = u64::from_be_bytes(b[0..8].try_into().unwrap());
    let v = DecodeCmpUintToInt(!u);
    Ok((&b[8..], v))
}

/// 升序可比较地编码无符号整数（大端 8 字节）。
// EncodeUint appends the encoded value to slice b and returns the appended slice.
// EncodeUint guarantees that the encoded value is in ascending order for comparison.
pub fn EncodeUint(mut b: Vec<u8>, v: u64) -> Vec<u8> {
    b.extend_from_slice(&v.to_be_bytes());
    b
}

/// 降序可比较地编码无符号整数（按位取反后写大端）。
// EncodeUintDesc appends the encoded value to slice b and returns the appended slice.
// EncodeUintDesc guarantees that the encoded value is in descending order for comparison.
pub fn EncodeUintDesc(mut b: Vec<u8>, v: u64) -> Vec<u8> {
    b.extend_from_slice(&(!v).to_be_bytes());
    b
}

/// 解码 EncodeUint 写出的 8 字节。
// DecodeUint decodes value encoded by EncodeUint before.
// It returns the leftover un-decoded slice, decoded value if no error.
pub fn DecodeUint(b: &[u8]) -> Result<(&[u8], u64), errors::SharedError> {
    if b.len() < 8 {
        return Err(errors::New("insufficient bytes to decode value"));
    }
    let v = u64::from_be_bytes(b[0..8].try_into().unwrap());
    Ok((&b[8..], v))
}

/// 解码 EncodeUintDesc 写出的 8 字节（读出后取反还原）。
// DecodeUintDesc decodes value encoded by EncodeInt before.
// It returns the leftover un-decoded slice, decoded value if no error.
pub fn DecodeUintDesc(b: &[u8]) -> Result<(&[u8], u64), errors::SharedError> {
    if b.len() < 8 {
        return Err(errors::New("insufficient bytes to decode value"));
    }
    let v = u64::from_be_bytes(b[0..8].try_into().unwrap());
    Ok((&b[8..], !v))
}

/// uvarint 最大长度（对齐 encoding/binary）。
const maxVarintLen64: usize = 10;

/// 变长整数解码错误：字节不足或超过 64 位。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VarintDecodeError {
    Insufficient,
    Overflow,
}

/// 按 encoding/binary.Uvarint 规则解析前缀，含第十字节溢出判定。
// decodeUvarintPrefix follows encoding/binary.Uvarint, including its tenth-byte
// overflow rule. The byte count is returned so callers can preserve the suffix.
fn decodeUvarintPrefix(b: &[u8]) -> Result<(u64, usize), VarintDecodeError> {
    let mut value = 0u64;
    let mut shift = 0u32;
    for (index, byte) in b.iter().copied().enumerate() {
        if index == maxVarintLen64 - 1 && byte > 1 {
            return Err(VarintDecodeError::Overflow);
        }
        if byte < 0x80 {
            return Ok((value | u64::from(byte) << shift, index + 1));
        }
        value |= u64::from(byte & 0x7f) << shift;
        shift += 7;
    }
    Err(VarintDecodeError::Insufficient)
}

/// 编码有符号变长整数（非 memcomparable；负数用 ZigZag 变体）。
// EncodeVarint appends the encoded value to slice b and returns the appended slice.
// Note that the encoded result is not memcomparable.
pub fn EncodeVarint(b: Vec<u8>, v: i64) -> Vec<u8> {
    let mut value = (v as u64) << 1;
    if v < 0 {
        value = !value;
    }
    EncodeUvarint(b, value)
}

/// 解码 EncodeVarint 写出的变长整数。
// DecodeVarint decodes value encoded by EncodeVarint before.
// It returns the leftover un-decoded slice, decoded value if no error.
pub fn DecodeVarint(b: &[u8]) -> Result<(&[u8], i64), errors::SharedError> {
    match decodeUvarintPrefix(b) {
        Ok((value, length)) => {
            let mut decoded = (value >> 1) as i64;
            if value & 1 != 0 {
                decoded = !decoded;
            }
            Ok((&b[length..], decoded))
        }
        Err(VarintDecodeError::Overflow) => Err(errors::New("value larger than 64 bits")),
        Err(VarintDecodeError::Insufficient) => {
            Err(errors::New("insufficient bytes to decode value"))
        }
    }
}

/// 编码无符号变长整数（非 memcomparable）。
// EncodeUvarint appends the encoded value to slice b and returns the appended slice.
// Note that the encoded result is not memcomparable.
pub fn EncodeUvarint(mut b: Vec<u8>, v: u64) -> Vec<u8> {
    let mut value = v;
    while value >= 0x80 {
        b.push(value as u8 | 0x80);
        value >>= 7;
    }
    b.push(value as u8);
    b
}

/// 解码 EncodeUvarint 写出的变长无符号整数。
// DecodeUvarint decodes value encoded by EncodeUvarint before.
// It returns the leftover un-decoded slice, decoded value if no error.
pub fn DecodeUvarint(b: &[u8]) -> Result<(&[u8], u64), errors::SharedError> {
    match decodeUvarintPrefix(b) {
        Ok((value, length)) => Ok((&b[length..], value)),
        Err(VarintDecodeError::Overflow) => Err(errors::New("value larger than 64 bits")),
        Err(VarintDecodeError::Insufficient) => {
            Err(errors::New("insufficient bytes to decode value"))
        }
    }
}

/// 负标签终点：负数字节数 = negativeTagEnd - tag。
const negativeTagEnd: u8 = 8; // negative tag is (negativeTagEnd - length).
/// 正标签起点：正数字节数 = first - positiveTagStart。
const positiveTagStart: u8 = 0xff - 8; // Positive tag is (positiveTagStart + length).

/// 将 i64 编码为 memcomparable 变长字节（负数带长度标签）。
// EncodeComparableVarint encodes an int64 to a mem-comparable bytes.
pub fn EncodeComparableVarint(mut b: Vec<u8>, v: i64) -> Vec<u8> {
    if v < 0 {
        // All negative value has a tag byte prefix (negativeTagEnd - length).
        // Smaller negative value encodes to more bytes, has smaller tag.
        // 按绝对值大小选择 1~8 字节载荷，标签越小表示越大（更负）的值
        if v >= -0xff {
            b.extend_from_slice(&[negativeTagEnd - 1, v as u8]);
        } else if v >= -0xffff {
            b.extend_from_slice(&[negativeTagEnd - 2, (v >> 8) as u8, v as u8]);
        } else if v >= -0xffffff {
            b.extend_from_slice(&[negativeTagEnd - 3, (v >> 16) as u8, (v >> 8) as u8, v as u8]);
        } else if v >= -0xffffffff {
            b.extend_from_slice(&[
                negativeTagEnd - 4,
                (v >> 24) as u8,
                (v >> 16) as u8,
                (v >> 8) as u8,
                v as u8,
            ]);
        } else if v >= -0xffffffffff {
            b.extend_from_slice(&[
                negativeTagEnd - 5,
                (v >> 32) as u8,
                (v >> 24) as u8,
                (v >> 16) as u8,
                (v >> 8) as u8,
                v as u8,
            ]);
        } else if v >= -0xffffffffffff {
            b.extend_from_slice(&[
                negativeTagEnd - 6,
                (v >> 40) as u8,
                (v >> 32) as u8,
                (v >> 24) as u8,
                (v >> 16) as u8,
                (v >> 8) as u8,
                v as u8,
            ]);
        } else if v >= -0xffffffffffffff {
            b.extend_from_slice(&[
                negativeTagEnd - 7,
                (v >> 48) as u8,
                (v >> 40) as u8,
                (v >> 32) as u8,
                (v >> 24) as u8,
                (v >> 16) as u8,
                (v >> 8) as u8,
                v as u8,
            ]);
        } else {
            b.extend_from_slice(&[
                negativeTagEnd - 8,
                (v >> 56) as u8,
                (v >> 48) as u8,
                (v >> 40) as u8,
                (v >> 32) as u8,
                (v >> 24) as u8,
                (v >> 16) as u8,
                (v >> 8) as u8,
                v as u8,
            ]);
        }
        return b;
    }
    EncodeComparableUvarint(b, v as u64)
}

/// 将 u64 编码为 memcomparable 变长字节（单字节或正标签+载荷）。
// EncodeComparableUvarint encodes uint64 into mem-comparable bytes.
pub fn EncodeComparableUvarint(mut b: Vec<u8>, v: u64) -> Vec<u8> {
    // The first byte has 256 values, [0, 7] is reserved for negative tags,
    // [248, 255] is reserved for larger positive tags.
    // Values fitting in [0, 239] use a single byte; larger values carry a length tag.
    if v <= (positiveTagStart - negativeTagEnd) as u64 {
        b.push(v as u8 + negativeTagEnd);
    } else if v <= 0xff {
        b.extend_from_slice(&[positiveTagStart + 1, v as u8]);
    } else if v <= 0xffff {
        b.extend_from_slice(&[positiveTagStart + 2, (v >> 8) as u8, v as u8]);
    } else if v <= 0xffffff {
        b.extend_from_slice(&[
            positiveTagStart + 3,
            (v >> 16) as u8,
            (v >> 8) as u8,
            v as u8,
        ]);
    } else if v <= 0xffffffff {
        b.extend_from_slice(&[
            positiveTagStart + 4,
            (v >> 24) as u8,
            (v >> 16) as u8,
            (v >> 8) as u8,
            v as u8,
        ]);
    } else if v <= 0xffffffffff {
        b.extend_from_slice(&[
            positiveTagStart + 5,
            (v >> 32) as u8,
            (v >> 24) as u8,
            (v >> 16) as u8,
            (v >> 8) as u8,
            v as u8,
        ]);
    } else if v <= 0xffffffffffff {
        b.extend_from_slice(&[
            positiveTagStart + 6,
            (v >> 40) as u8,
            (v >> 32) as u8,
            (v >> 24) as u8,
            (v >> 16) as u8,
            (v >> 8) as u8,
            v as u8,
        ]);
    } else if v <= 0xffffffffffffff {
        b.extend_from_slice(&[
            positiveTagStart + 7,
            (v >> 48) as u8,
            (v >> 40) as u8,
            (v >> 32) as u8,
            (v >> 24) as u8,
            (v >> 16) as u8,
            (v >> 8) as u8,
            v as u8,
        ]);
    } else {
        b.extend_from_slice(&[
            positiveTagStart + 8,
            (v >> 56) as u8,
            (v >> 48) as u8,
            (v >> 40) as u8,
            (v >> 32) as u8,
            (v >> 24) as u8,
            (v >> 16) as u8,
            (v >> 8) as u8,
            v as u8,
        ]);
    }
    b
}

// Preserve Go package-level sentinel identity across calls and Trace wrappers.
/// 构造“字节不足”错误。
fn errDecodeInsufficient() -> errors::SharedError {
    static ERROR: std::sync::LazyLock<errors::SharedError> =
        std::sync::LazyLock::new(|| errors::New("insufficient bytes to decode value"));
    ERROR.clone()
}

/// 构造“非法编码”错误。
fn errDecodeInvalid() -> errors::SharedError {
    static ERROR: std::sync::LazyLock<errors::SharedError> =
        std::sync::LazyLock::new(|| errors::New("invalid bytes to decode value"));
    ERROR.clone()
}

// DecodeComparableUvarint decodes mem-comparable uvarint.
/// 对错误做 Trace 包装，保持与 Go errors.Trace 一致。
fn traceError(error: errors::SharedError) -> errors::SharedError {
    errors::Trace(Some(error)).expect("tracing a present error preserves it")
}

/// 解码 memcomparable 无符号变长整数。
pub fn DecodeComparableUvarint(b: &[u8]) -> Result<(&[u8], u64), errors::SharedError> {
    if b.is_empty() {
        return Err(errDecodeInsufficient());
    }
    let first = b[0];
    let b = &b[1..];
    // 负标签区不能出现在无符号解码路径
    if first < negativeTagEnd {
        return Err(traceError(errDecodeInvalid()));
    }
    // 单字节正数：值 = first - negativeTagEnd
    if first <= positiveTagStart {
        return Ok((b, (first - negativeTagEnd) as u64));
    }
    let length = (first - positiveTagStart) as usize;
    if b.len() < length {
        return Err(traceError(errDecodeInsufficient()));
    }
    let mut v = 0u64;
    for c in &b[..length] {
        v = (v << 8) | (*c as u64);
    }
    Ok((&b[length..], v))
}

/// 解码 memcomparable 有符号变长整数。
// DecodeComparableVarint decodes mem-comparable varint.
pub fn DecodeComparableVarint(b: &[u8]) -> Result<(&[u8], i64), errors::SharedError> {
    if b.is_empty() {
        return Err(traceError(errDecodeInsufficient()));
    }
    let first = b[0];
    if first >= negativeTagEnd && first <= positiveTagStart {
        // Keep Go's historical contract: the inline branch returns the original
        // slice instead of consuming its single tag byte.
        return Ok((b, first as i64 - negativeTagEnd as i64));
    }
    let b = &b[1..];
    let length: usize;
    let mut v: u64 = 0;
    if first < negativeTagEnd {
        length = (negativeTagEnd - first) as usize;
        // negative value has all bits on by default.
        v = u64::MAX;
    } else {
        length = (first - positiveTagStart) as usize;
    }
    if b.len() < length {
        return Err(traceError(errDecodeInsufficient()));
    }
    for c in &b[..length] {
        v = (v << 8) | (*c as u64);
    }
    // 校验正数不得溢出 i64、负数载荷必须落在负半区
    if first > positiveTagStart && v > i64::MAX as u64 {
        return Err(traceError(errDecodeInvalid()));
    } else if first < negativeTagEnd && v <= i64::MAX as u64 {
        return Err(traceError(errDecodeInvalid()));
    }
    Ok((&b[length..], v as i64))
}
