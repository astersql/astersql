// Copyright 2026 AsterSQL.
// Copyright 2023-2024 PingCAP, Inc.
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

// GB18030 字符集编码实现。
//
// 对应 Go `encoding_gb18030.go`：探测 1/2/4 字节序列，编解码与大小写转换。
// GB18030 是中国国家标准多字节字符集；TiDB 刻意将 0x80 视为非法（WHATWG 会映射为欧元符）。

use crate::encoding::*;
use crate::encoding_gb18030_data::gb18030_case;
use encoding_rs::GB18030;

/// EncodingGb18030 对应 Go encodingGB18030。
pub struct EncodingGb18030;
/// 全局共享的 GB18030 编码实例。
pub static ENCODING_GB18030_IMPL: EncodingGb18030 = EncodingGb18030;

/// 按 GB18030 规则探测下一个字符的字节切片；非法时回退为首字节。
pub fn peek(src: &[u8]) -> &[u8] {
    if src.is_empty() {
        return src;
    }
    let invalid = &src[..1];
    match src[0] {
        // 单字节 ASCII 区间。
        0x00..=0x7f => invalid,
        // 双字节或四字节多字节前导。
        0x81..=0xfe => {
            if src.len() < 2 {
                return invalid;
            }
            // 双字节：第二字节落在 GBK 兼容续字节范围。
            if (0x40..0x7f).contains(&src[1]) || (0x80..=0xfe).contains(&src[1]) {
                return &src[..2];
            }
            // 四字节：形如 0x81-0xFE + 0x30-0x39 + 0x81-0xFE + 0x30-0x39。
            if src.len() >= 4
                && (0x30..=0x39).contains(&src[1])
                && (0x81..=0xfe).contains(&src[2])
                && (0x30..=0x39).contains(&src[3])
            {
                return &src[..4];
            }
            invalid
        }
        _ => invalid,
    }
}

/// 将一个 GB18030 字符块解码为 UTF-8 字节；失败返回 None。
fn decode_gb18030(chunk: &[u8]) -> Option<Vec<u8>> {
    // TiDB intentionally treats 0x80 as malformed even though WHATWG maps it to EURO SIGN.
    // TiDB 刻意将 0x80 视为非法，即便 WHATWG 会把它映射为欧元符。
    if chunk.first() == Some(&0x80) {
        return None;
    }
    GB18030
        .decode_without_bom_handling_and_without_replacement(chunk)
        .map(|text| text.into_owned().into_bytes())
}

/// 将一段 UTF-8 编码为 GB18030；含非法码点时返回 None。
fn encode_gb18030(chunk: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(chunk).ok()?;
    let (encoded, _, had_errors) = GB18030.encode(text);
    (!had_errors).then(|| encoded.into_owned())
}

/// 按 Op 方向逐字符遍历：解码到 UTF-8 或从 UTF-8 编码。
fn foreach_gb18030(src: &[u8], op: Op, callback: &mut dyn FnMut(&[u8], &[u8], bool) -> bool) {
    let mut offset = 0;
    while offset < src.len() {
        let (from, to) = if op & OP_TO_UTF8 != 0 {
            let from = peek(&src[offset..]);
            (from, decode_gb18030(from))
        } else {
            let (from, valid) = utf8_chunk(&src[offset..]);
            (from, valid.then(|| encode_gb18030(from)).flatten())
        };
        let ok = to.is_some();
        if !callback(from, to.as_deref().unwrap_or_default(), ok) {
            break;
        }
        offset += from.len();
    }
}

/// 通用转换入口：收集转换结果，并按 Op 处理首个错误（截断/替换/报错）。
fn transform_gb18030(dest: &mut Vec<u8>, src: &[u8], op: Op) -> Result<Vec<u8>, EncodingError> {
    let mut output = Vec::with_capacity(src.len());
    let mut first = None;
    foreach_gb18030(src, op, &mut |from, to, ok| {
        if ok {
            output.extend_from_slice(if op & OP_COLLECT_FROM != 0 { from } else { to });
            true
        } else {
            // 只记录首个非法字节序列，供后续生成 EncodingError。
            if first.is_none() {
                first = Some((CharsetGB18030, from.to_vec()));
            }
            invalid_action(&mut output, op)
        }
    });
    finish_transform(dest, output, first, op)
}

impl Encoding for EncodingGb18030 {
    /// 返回字符集名称 `gb18030`。
    fn Name(&self) -> &'static str {
        CharsetGB18030
    }
    /// 返回编码类型枚举值。
    fn Tp(&self) -> EncodingTp {
        EncodingTpGB18030
    }
    /// 探测下一个 GB18030 字符的字节边界。
    fn Peek<'a>(&self, src: &'a [u8]) -> &'a [u8] {
        peek(src)
    }
    /// 返回多字节字符长度（2 或 4）；非多字节前导返回 0。
    fn MbLen(&self, bytes: &[u8]) -> usize {
        if bytes.len() < 2 || !(0x81..=0xfe).contains(&bytes[0]) {
            return 0;
        }
        if (0x40..=0x7e).contains(&bytes[1]) || (0x80..=0xfe).contains(&bytes[1]) {
            return 2;
        }
        if bytes.len() >= 4
            && (0x30..=0x39).contains(&bytes[1])
            && (0x81..=0xfe).contains(&bytes[2])
            && (0x30..=0x39).contains(&bytes[3])
        {
            4
        } else {
            0
        }
    }
    /// 从 UTF-8 视角校验能否完整编码为 GB18030。
    fn IsValid(&self, src: &[u8]) -> bool {
        let mut valid = true;
        foreach_gb18030(src, OP_FROM_UTF8, &mut |_, _, ok| {
            valid = ok;
            ok
        });
        valid
    }
    /// 委托给 foreach_gb18030 逐字符回调。
    fn Foreach(&self, src: &[u8], op: Op, callback: &mut dyn FnMut(&[u8], &[u8], bool) -> bool) {
        foreach_gb18030(src, op, callback)
    }
    /// 委托给 transform_gb18030 执行编解码转换。
    fn Transform(&self, dest: &mut Vec<u8>, src: &[u8], op: Op) -> Result<Vec<u8>, EncodingError> {
        transform_gb18030(dest, src, op)
    }
    /// 使用 GB18030 特殊大小写表转大写。
    fn ToUpper(&self, src: &str) -> String {
        gb18030_case().to_upper(src)
    }
    /// 使用 GB18030 特殊大小写表转小写。
    fn ToLower(&self, src: &str) -> String {
        gb18030_case().to_lower(src)
    }
}

/// 自定义 GB18030 编码器，封装 OpEncode 路径。
pub struct CustomGb18030Encoder;
/// 自定义 GB18030 解码器；跟踪末尾是否为替换字符序列。
pub struct CustomGb18030Decoder {
    /// 最近一次输入是否以 U+FFFD 对应的 GB18030 字节结尾。
    rune_error_is_last_input: bool,
}

/// 构造自定义 GB18030 编码器。
pub fn new_custom_gb18030_encoder() -> CustomGb18030Encoder {
    CustomGb18030Encoder
}
/// 导出 Go 风格命名的编码器构造函数。
pub fn NewCustomGB18030Encoder() -> CustomGb18030Encoder {
    new_custom_gb18030_encoder()
}
/// 构造自定义 GB18030 解码器。
pub fn new_custom_gb18030_decoder() -> CustomGb18030Decoder {
    CustomGb18030Decoder {
        rune_error_is_last_input: false,
    }
}
/// 导出 Go 风格命名的解码器构造函数。
pub fn NewCustomGB18030Decoder() -> CustomGb18030Decoder {
    new_custom_gb18030_decoder()
}

impl CustomGb18030Encoder {
    /// 以编码方向转换字节序列。
    pub fn transform(&mut self, dest: &mut Vec<u8>, src: &[u8]) -> Result<Vec<u8>, EncodingError> {
        transform_gb18030(dest, src, OpEncode)
    }
    /// 重置编码器状态（当前无状态，保留接口形状）。
    pub fn reset(&mut self) {}
}

impl CustomGb18030Decoder {
    /// 以解码方向转换，并记录末尾是否为替换字符字节序列。
    pub fn transform(&mut self, dest: &mut Vec<u8>, src: &[u8]) -> Result<Vec<u8>, EncodingError> {
        let result = transform_gb18030(dest, src, OpDecode);
        // 0x84 0x31 0xA4 0x37 是 U+FFFD 在 GB18030 中的编码。
        self.rune_error_is_last_input = src.ends_with(&[0x84, 0x31, 0xa4, 0x37]);
        result
    }
    /// 返回最近一次输入是否以替换字符字节结尾。
    pub fn rune_error_is_last_input(&self) -> bool {
        self.rune_error_is_last_input
    }
    /// 清空替换字符跟踪标志。
    pub fn reset(&mut self) {
        self.rune_error_is_last_input = false;
    }
}

/// 将字节序列按大端解释为 u32，用于 GB18030 四字节码点数值。
pub fn convert_bytes_to_u32(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .fold(0, |value, byte| (value << 8) | (*byte as u32))
}

/// 将 u32 转为去掉前导零的大端字节序列。
pub fn convert_u32_to_bytes(value: u32) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    let first = bytes.iter().position(|byte| *byte != 0).unwrap_or(3);
    bytes[first..].to_vec()
}
