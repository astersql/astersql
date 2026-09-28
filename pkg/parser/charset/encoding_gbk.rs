// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// GBK 字符集编码实现。
//
// 对应 Go `encoding_gbk.go`：双字节汉字编码的探测、编解码与大小写转换。
// GBK 是 GB2312 的扩展；本实现对齐 Go simplifiedchinese.GBK，将 CP936 用户自定义区视为非法。

use crate::encoding::*;
use encoding_rs::GBK;

/// EncodingGbk 对应 Go encodingGBK。
pub struct EncodingGbk;
/// 全局共享的 GBK 编码实例。
pub static ENCODING_GBK_IMPL: EncodingGbk = EncodingGbk;

/// 按 GBK 规则探测下一个字符：高位字节开启则取 2 字节，否则取 1 字节。
pub(crate) fn peek_gbk(src: &[u8]) -> &[u8] {
    if src.is_empty() {
        return src;
    }
    let width = if src[0] < 0x80 { 1 } else { 2 };
    &src[..src.len().min(width)]
}

/// 将一个 GBK 字符块解码为 UTF-8；0x80 与私用区映射均视为非法。
fn decode_gbk(chunk: &[u8]) -> Option<Vec<u8>> {
    if chunk.first() == Some(&0x80) || is_gbk_private_use_pair(chunk) {
        return None;
    }
    GBK.decode_without_bom_handling_and_without_replacement(chunk)
        .map(|text| text.into_owned().into_bytes())
}

// encoding_rs follows the WHATWG GBK index and maps CP936 user-defined byte
// ranges to Unicode private-use code points. Go's simplifiedchinese.GBK leaves
// those table entries unmapped, so TiDB must report them as invalid instead.
/// 判断是否为 CP936 用户自定义区字节对；Go 侧不映射，故在此报告为非法。
fn is_gbk_private_use_pair(chunk: &[u8]) -> bool {
    let [lead, trail] = chunk else { return false };
    (((0xaa..=0xaf).contains(lead) || (0xf8..=0xfe).contains(lead))
        && (0xa1..=0xfe).contains(trail))
        || ((0xa1..=0xa7).contains(lead)
            && ((0x40..=0x7e).contains(trail) || (0x80..=0xa0).contains(trail)))
}

/// 将一段 UTF-8 编码为 GBK；欧元符在 Go/TiDB 中不支持，显式拒绝。
fn encode_gbk(chunk: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(chunk).ok()?;
    // Go simplifiedchinese.GBK 不编码欧元符 €。
    if text == "€" {
        return None;
    }
    let (encoded, _, had_errors) = GBK.encode(text);
    (!had_errors).then(|| encoded.into_owned())
}

/// 按 Op 方向逐字符遍历并回调。
fn foreach_gbk(src: &[u8], op: Op, callback: &mut dyn FnMut(&[u8], &[u8], bool) -> bool) {
    let mut offset = 0;
    while offset < src.len() {
        let (from, to) = if op & OP_TO_UTF8 != 0 {
            let from = peek_gbk(&src[offset..]);
            (from, decode_gbk(from))
        } else {
            let (from, valid) = utf8_chunk(&src[offset..]);
            (from, valid.then(|| encode_gbk(from)).flatten())
        };
        let ok = to.is_some();
        if !callback(from, to.as_deref().unwrap_or_default(), ok) {
            break;
        }
        offset += from.len();
    }
}

/// 通用转换入口：收集结果并按 Op 处理非法字符。
fn transform_gbk(dest: &mut Vec<u8>, src: &[u8], op: Op) -> Result<Vec<u8>, EncodingError> {
    let mut output = Vec::with_capacity(src.len());
    let mut first = None;
    foreach_gbk(src, op, &mut |from, to, ok| {
        if ok {
            output.extend_from_slice(if op & OP_COLLECT_FROM != 0 { from } else { to });
            true
        } else {
            if first.is_none() {
                first = Some((CharsetGBK, from.to_vec()));
            }
            invalid_action(&mut output, op)
        }
    });
    finish_transform(dest, output, first, op)
}

/// MySQL/GBK 大小写转换中保持不变的 Unicode 码点区间。
const GBK_UNCHANGED_CASE_RANGES: &[(u32, u32)] = &[
    (0x00e0, 0x00e1),
    (0x00e8, 0x00ea),
    (0x00ec, 0x00ed),
    (0x00f2, 0x00f3),
    (0x00f9, 0x00fa),
    (0x00fc, 0x00fc),
    (0x0101, 0x0101),
    (0x0113, 0x0113),
    (0x011b, 0x011b),
    (0x012b, 0x012b),
    (0x0144, 0x0144),
    (0x0148, 0x0148),
    (0x014d, 0x014d),
    (0x016b, 0x016b),
    (0x01ce, 0x01ce),
    (0x01d0, 0x01d0),
    (0x01d2, 0x01d2),
    (0x01d4, 0x01d4),
    (0x01d6, 0x01d6),
    (0x01d8, 0x01d8),
    (0x01da, 0x01da),
    (0x01dc, 0x01dc),
    (0x216a, 0x216b),
];

/// 按 GBK 特殊规则做大小写转换；落在不变区间内的字符原样保留。
fn gbk_case(src: &str, upper: bool) -> String {
    let mut output = String::with_capacity(src.len());
    for character in src.chars() {
        let code = character as u32;
        if GBK_UNCHANGED_CASE_RANGES
            .iter()
            .any(|(lo, hi)| *lo <= code && code <= *hi)
        {
            output.push(character);
        } else if upper {
            output.push(go_simple_upper(character));
        } else {
            output.push(go_simple_lower(character));
        }
    }
    output
}

/// 对齐 Go unicode.ToUpper 的单码点映射，避免 Rust 完整映射扩展成多个码点。
fn go_simple_upper(character: char) -> char {
    let mut mapped = character.to_uppercase();
    let first = mapped.next().unwrap_or(character);
    if mapped.next().is_none() {
        return first;
    }

    let code = character as u32;
    let simple = match code {
        0x1f80..=0x1f87 | 0x1f90..=0x1f97 | 0x1fa0..=0x1fa7 => code + 8,
        0x1fb3 => 0x1fbc,
        0x1fc3 => 0x1fcc,
        0x1ff3 => 0x1ffc,
        _ => code,
    };
    char::from_u32(simple).unwrap_or(character)
}

/// 对齐 Go unicode.ToLower 的单码点映射；U+0130 是 Rust 会展开的唯一标量。
fn go_simple_lower(character: char) -> char {
    let mut mapped = character.to_lowercase();
    let first = mapped.next().unwrap_or(character);
    if mapped.next().is_none() {
        return first;
    }
    if character == '\u{130}' {
        'i'
    } else {
        character
    }
}

impl Encoding for EncodingGbk {
    /// 返回字符集名称 `gbk`。
    fn Name(&self) -> &'static str {
        CharsetGBK
    }
    /// 返回编码类型枚举值。
    fn Tp(&self) -> EncodingTp {
        EncodingTpGBK
    }
    /// 探测下一个 GBK 字符的字节边界。
    fn Peek<'a>(&self, src: &'a [u8]) -> &'a [u8] {
        peek_gbk(src)
    }
    /// 双字节汉字返回 2，否则返回 0。
    fn MbLen(&self, bytes: &[u8]) -> usize {
        if bytes.len() >= 2
            && (0x81..=0xfe).contains(&bytes[0])
            && ((0x40..=0x7e).contains(&bytes[1]) || (0x80..=0xfe).contains(&bytes[1]))
        {
            2
        } else {
            0
        }
    }
    /// 从 UTF-8 视角校验能否完整编码为 GBK。
    fn IsValid(&self, src: &[u8]) -> bool {
        let mut valid = true;
        foreach_gbk(src, OP_FROM_UTF8, &mut |_, _, ok| {
            valid = ok;
            ok
        });
        valid
    }
    /// 委托给 foreach_gbk 逐字符回调。
    fn Foreach(&self, src: &[u8], op: Op, callback: &mut dyn FnMut(&[u8], &[u8], bool) -> bool) {
        foreach_gbk(src, op, callback)
    }
    /// 委托给 transform_gbk 执行编解码转换。
    fn Transform(&self, dest: &mut Vec<u8>, src: &[u8], op: Op) -> Result<Vec<u8>, EncodingError> {
        transform_gbk(dest, src, op)
    }
    /// 使用 GBK 特殊大小写规则转大写。
    fn ToUpper(&self, src: &str) -> String {
        gbk_case(src, true)
    }
    /// 使用 GBK 特殊大小写规则转小写。
    fn ToLower(&self, src: &str) -> String {
        gbk_case(src, false)
    }
}

/// 自定义 GBK 编码器，封装 OpEncode 路径。
pub struct CustomGbkEncoder;

/// 构造自定义 GBK 编码器。
pub fn new_custom_gbk_encoder() -> CustomGbkEncoder {
    CustomGbkEncoder
}
/// 导出 Go 风格命名的编码器构造函数。
pub fn NewCustomGBKEncoder() -> CustomGbkEncoder {
    new_custom_gbk_encoder()
}

impl CustomGbkEncoder {
    /// 以编码方向转换字节序列。
    pub fn transform(&mut self, dest: &mut Vec<u8>, src: &[u8]) -> Result<Vec<u8>, EncodingError> {
        transform_gbk(dest, src, OpEncode)
    }
    /// 重置编码器状态（当前无状态，保留接口形状）。
    pub fn reset(&mut self) {}
}
