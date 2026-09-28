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

// MySQL 风格二进制字面量（`0x..` / `x'..'`）与位串字面量（`0b..` / `b'..'`）的表示与解析。
//
// 提供从整数构造、与 `u64` 互转、按大端比较，以及截断（truncate）错误处理；
// 对应 SQL 中的 BINARY / BIT / HEX 字面量语义。

use std::ops::Deref;

use crate::{Context, ValueResult, errors};

/// 原始字节序列形式的二进制字面量。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BinaryLiteral(pub Vec<u8>);

impl AsRef<[u8]> for BinaryLiteral {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Deref for BinaryLiteral {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

/// BIT 字面量包装，内部仍为 [`BinaryLiteral`]。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BitLiteral(pub BinaryLiteral);

/// HEX 字面量包装，内部仍为 [`BinaryLiteral`]。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HexLiteral(pub BinaryLiteral);

/// 返回空字节的零值二进制字面量。
pub fn ZeroBinaryLiteral() -> BinaryLiteral {
    BinaryLiteral(Vec::new())
}

/// 去掉前导零字节；全零时保留最后一个字节。
pub fn trimLeadingZeroBytes(bytes: &[u8]) -> &[u8] {
    if bytes.is_empty() {
        return bytes;
    }
    let first_non_zero = bytes
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(bytes.len() - 1);
    &bytes[first_non_zero..]
}

/// 从 `u64` 构造二进制字面量。
///
/// `byteSize == -1` 时去掉前导零；否则取大端表示的最低 `byteSize`（1..=8）字节。
pub fn NewBinaryLiteralFromUint(value: u64, byteSize: isize) -> BinaryLiteral {
    assert!(
        byteSize == -1 || (1..=8).contains(&byteSize),
        "Invalid byteSize"
    );
    let bytes = value.to_be_bytes();
    if byteSize == -1 {
        BinaryLiteral(trimLeadingZeroBytes(&bytes).to_vec())
    } else {
        BinaryLiteral(bytes[8 - byteSize as usize..].to_vec())
    }
}

impl BinaryLiteral {
    /// 格式化为 `0x` 加小写十六进制；空则为空串。
    pub fn String(&self) -> String {
        if self.0.is_empty() {
            String::new()
        } else {
            format!("0x{}", hex::encode(&self.0))
        }
    }

    /// 将字节按 UTF-8（lossy）解释为字符串。
    pub fn ToString(&self) -> String {
        String::from_utf8_lossy(&self.0).into_owned()
    }

    /// 格式化为 `b'...'` 位串；可选去掉前导 `0` 位。
    pub fn ToBitLiteralString(&self, trimLeadingZero: bool) -> String {
        if self.0.is_empty() {
            return "b''".to_owned();
        }
        let mut bits = String::with_capacity(self.0.len() * 8);
        for byte in &self.0 {
            use std::fmt::Write;
            write!(&mut bits, "{byte:08b}").expect("writing to String cannot fail");
        }
        if trimLeadingZero {
            let trimmed = bits.trim_start_matches('0');
            bits = if trimmed.is_empty() { "0" } else { trimmed }.to_owned();
        }
        format!("b'{bits}'")
    }

    /// 解释为大端无符号整数；超过 8 字节时走截断错误路径并返回 `u64::MAX`。
    pub fn ToInt(&self, ctx: Context) -> ValueResult<u64> {
        let bytes = trimLeadingZeroBytes(&self.0);
        if bytes.is_empty() {
            return Ok(0);
        }
        if bytes.len() > 8 {
            return ctx.HandleTruncate(
                u64::MAX,
                errors::New(format!(
                    "truncated incorrect BINARY value: {}",
                    self.String()
                )),
            );
        }
        Ok(bytes
            .iter()
            .fold(0_u64, |value, byte| (value << 8) | u64::from(*byte)))
    }

    /// 去掉前导零后按长度再按字典序比较，返回 -1/0/1。
    pub fn Compare(&self, other: BinaryLiteral) -> i32 {
        let left = trimLeadingZeroBytes(&self.0);
        let right = trimLeadingZeroBytes(&other.0);
        left.len()
            .cmp(&right.len())
            .then_with(|| left.cmp(right))
            .signum()
    }
}

/// 将 `Ordering` 映射为 Go 风格的 -1/0/1。
trait OrderingSignum {
    fn signum(self) -> i32;
}

impl OrderingSignum for std::cmp::Ordering {
    fn signum(self) -> i32 {
        match self {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }
    }
}

/// 解析 `b'...'` / `B'...'` / `0b...` 形式的位串为 [`BinaryLiteral`]。
pub fn ParseBitStr(mut input: String) -> Result<BinaryLiteral, errors::SharedError> {
    if input.is_empty() {
        return Err(errors::New("invalid empty string for parsing bit type"));
    }
    // 去掉 b'/B' 前缀或 0b 前缀，得到纯 01 串。
    if matches!(input.as_bytes()[0], b'b' | b'B') {
        input = input[1..].trim_matches('\'').to_owned();
    } else if input.starts_with("0b") {
        input = input[2..].to_owned();
    } else {
        return Err(errors::New(format!("invalid bit type format {input}")));
    }
    if input.is_empty() {
        return Ok(ZeroBinaryLiteral());
    }
    if !input.bytes().all(|byte| matches!(byte, b'0' | b'1')) {
        return Err(errors::New(format!(
            "invalid digit found in bit literal {input}"
        )));
    }

    // 左补零对齐到 8 的倍数，再按字节解析。
    let aligned_length = (input.len() + 7) & !7;
    let mut padded = String::with_capacity(aligned_length);
    padded.extend(std::iter::repeat_n('0', aligned_length - input.len()));
    padded.push_str(&input);
    let mut bytes = Vec::with_capacity(aligned_length / 8);
    for chunk in padded.as_bytes().chunks_exact(8) {
        let text = std::str::from_utf8(chunk).expect("bit literal is ASCII");
        let value = u8::from_str_radix(text, 2).map_err(|error| errors::New(error.to_string()))?;
        bytes.push(value);
    }
    Ok(BinaryLiteral(bytes))
}

/// 解析位串并包装为 [`BitLiteral`]。
pub fn NewBitLiteral(input: String) -> Result<BitLiteral, errors::SharedError> {
    ParseBitStr(input).map(BitLiteral)
}

impl BitLiteral {
    /// 委托内部二进制字面量的 `ToString`。
    pub fn ToString(&self) -> String {
        self.0.ToString()
    }
}

/// 解析 `x'...'` / `X'...'` / `0x...` 形式的十六进制串为 [`BinaryLiteral`]。
pub fn ParseHexStr(mut input: String) -> Result<BinaryLiteral, errors::SharedError> {
    if input.is_empty() {
        return Err(errors::New(
            "invalid empty string for parsing hexadecimal literal",
        ));
    }
    // x'/X' 形式要求偶数长度；0x 形式奇数时左补 0。
    if matches!(input.as_bytes()[0], b'x' | b'X') {
        input = input[1..].trim_matches('\'').to_owned();
        if input.len() % 2 != 0 {
            return Err(errors::New(format!(
                "invalid hexadecimal format, must even numbers, but {}",
                input.len()
            )));
        }
    } else if input.starts_with("0x") {
        input = input[2..].to_owned();
    } else {
        return Err(errors::New(format!("invalid hexadecimal format {input}")));
    }
    if input.is_empty() {
        return Ok(ZeroBinaryLiteral());
    }
    if input.len() % 2 != 0 {
        input.insert(0, '0');
    }
    hex::decode(&input)
        .map(BinaryLiteral)
        .map_err(|error| errors::New(error.to_string()))
}

/// 解析十六进制串并包装为 [`HexLiteral`]。
pub fn NewHexLiteral(input: String) -> Result<HexLiteral, errors::SharedError> {
    ParseHexStr(input).map(HexLiteral)
}

impl HexLiteral {
    /// 委托内部二进制字面量的 `ToString`。
    pub fn ToString(&self) -> String {
        self.0.ToString()
    }
}
