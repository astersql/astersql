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

// 字符编码（encoding）抽象：查找、校验、分块遍历与编解码变换。
//
// 对照 `encoding.go`：按字符集名解析到具体实现，提供 `Op*` 位标志控制
// 替换/截断/收集方向；本文件内含 binary 与 ASCII 两种基础实现，其余实现在独立模块。

use crate::encoding_gb18030::ENCODING_GB18030_IMPL;
use crate::encoding_gbk::ENCODING_GBK_IMPL;
use crate::encoding_latin1::ENCODING_LATIN1_IMPL;
use crate::encoding_utf8::ENCODING_UTF8_IMPL;

/// utf8mb4 字符集名常量（编码查找用）。
pub const CharsetUTF8MB4: &str = "utf8mb4";
/// utf8 字符集名常量。
pub const CharsetUTF8: &str = "utf8";
/// gbk 字符集名常量。
pub const CharsetGBK: &str = "gbk";
/// latin1 字符集名常量。
pub const CharsetLatin1: &str = "latin1";
/// binary 伪字符集名常量。
pub const CharsetBin: &str = "binary";
/// ascii 字符集名常量。
pub const CharsetASCII: &str = "ascii";
/// gb18030 字符集名常量。
pub const CharsetGB18030: &str = "gb18030";

/// 编码实现类型枚举，对应 Go 的 EncodingTp。
#[repr(i8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodingTp {
    /// 未指定。
    None = 0,
    /// UTF-8 / utf8mb4。
    Utf8,
    /// 严格 utf8mb3 语义。
    Utf8Mb3Strict,
    /// ASCII。
    Ascii,
    /// Latin1。
    Latin1,
    /// 二进制透传。
    Bin,
    /// GBK。
    Gbk,
    /// GB18030。
    Gb18030,
}

/// EncodingTp::None 的别名常量。
pub const EncodingTpNone: EncodingTp = EncodingTp::None;
/// EncodingTp::Utf8 的别名常量。
pub const EncodingTpUTF8: EncodingTp = EncodingTp::Utf8;
/// EncodingTp::Utf8Mb3Strict 的别名常量。
pub const EncodingTpUTF8MB3Strict: EncodingTp = EncodingTp::Utf8Mb3Strict;
/// EncodingTp::Ascii 的别名常量。
pub const EncodingTpASCII: EncodingTp = EncodingTp::Ascii;
/// EncodingTp::Latin1 的别名常量。
pub const EncodingTpLatin1: EncodingTp = EncodingTp::Latin1;
/// EncodingTp::Bin 的别名常量。
pub const EncodingTpBin: EncodingTp = EncodingTp::Bin;
/// EncodingTp::Gbk 的别名常量。
pub const EncodingTpGBK: EncodingTp = EncodingTp::Gbk;
/// EncodingTp::Gb18030 的别名常量。
pub const EncodingTpGB18030: EncodingTp = EncodingTp::Gb18030;

/// 变换操作位标志类型。
pub type Op = i16;
/// 从 UTF-8 侧出发做变换。
pub(crate) const OP_FROM_UTF8: Op = 1 << 0;
/// 变换到 UTF-8。
pub(crate) const OP_TO_UTF8: Op = 1 << 1;
/// 遇到非法字节时截断输出。
pub(crate) const OP_TRUNCATE_TRIM: Op = 1 << 2;
/// 遇到非法字节时用 `?` 替换。
pub(crate) const OP_TRUNCATE_REPLACE: Op = 1 << 3;
/// 收集源侧字节。
pub(crate) const OP_COLLECT_FROM: Op = 1 << 4;
/// 收集目标侧字节。
pub(crate) const OP_COLLECT_TO: Op = 1 << 5;
/// 跳过错误，不返回 EncodingError。
pub(crate) const OP_SKIP_ERROR: Op = 1 << 6;

/// 替换非法字符且不报错（常见校验/清洗路径）。
pub const OpReplaceNoErr: Op = OP_FROM_UTF8 | OP_TRUNCATE_REPLACE | OP_COLLECT_FROM | OP_SKIP_ERROR;
/// 替换非法字符并在首次非法时报错。
pub const OpReplace: Op = OP_FROM_UTF8 | OP_TRUNCATE_REPLACE | OP_COLLECT_FROM;
/// 编码：截断非法并收集目标字节。
pub const OpEncode: Op = OP_FROM_UTF8 | OP_TRUNCATE_TRIM | OP_COLLECT_TO;
/// 编码且跳过错误。
pub const OpEncodeNoErr: Op = OpEncode | OP_SKIP_ERROR;
/// 编码并用 `?` 替换非法。
pub const OpEncodeReplace: Op = OP_FROM_UTF8 | OP_TRUNCATE_REPLACE | OP_COLLECT_TO;
/// 解码：截断非法并收集目标字节。
pub const OpDecode: Op = OP_TO_UTF8 | OP_TRUNCATE_TRIM | OP_COLLECT_TO;
/// 解码且跳过错误。
pub const OpDecodeNoErr: Op = OpDecode | OP_SKIP_ERROR;
/// 解码并用 `?` 替换非法。
pub const OpDecodeReplace: Op = OP_TO_UTF8 | OP_TRUNCATE_REPLACE | OP_COLLECT_TO;

/// 编码变换失败：记录编码名、非法片段与已产出输出。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodingError {
    /// 编码名称，用于错误文案。
    encoding: &'static str,
    /// 首次非法字节序列。
    invalid: Vec<u8>,
    /// 出错时已生成的输出缓冲。
    output: Vec<u8>,
}

impl EncodingError {
    /// 构造 EncodingError。
    pub(crate) fn new(encoding: &'static str, invalid: &[u8], output: Vec<u8>) -> Self {
        Self {
            encoding,
            invalid: invalid.to_vec(),
            output,
        }
    }

    /// 返回出错时已写入的输出字节。
    pub fn output(&self) -> &[u8] {
        &self.output
    }
}

impl std::fmt::Display for EncodingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "invalid {} character string: {:02X?}",
            self.encoding, self.invalid
        )
    }
}

impl std::error::Error for EncodingError {}

/// 字符编码 trait：名称、类型、窥视、多字节长度、校验、遍历、变换与大小写。
pub trait Encoding: Sync {
    /// 返回字符集名称。
    fn Name(&self) -> &'static str;
    /// 返回实现类型枚举。
    fn Tp(&self) -> EncodingTp;
    /// 窥视下一个字符占用的源字节切片。
    fn Peek<'a>(&self, src: &'a [u8]) -> &'a [u8];
    /// 多字节字符长度相关辅助（二进制/ASCII 常返回 0）。
    fn MbLen(&self, src: &[u8]) -> usize;
    /// 判断整段输入是否合法。
    fn IsValid(&self, src: &[u8]) -> bool;
    /// 按字符分块回调；回调返回 false 时提前停止。
    fn Foreach(&self, src: &[u8], op: Op, callback: &mut dyn FnMut(&[u8], &[u8], bool) -> bool);
    /// 按 Op 标志做编解码/替换变换。
    fn Transform(&self, dest: &mut Vec<u8>, src: &[u8], op: Op) -> Result<Vec<u8>, EncodingError>;
    /// 按该编码语义转大写。
    fn ToUpper(&self, src: &str) -> String;
    /// 按该编码语义转小写。
    fn ToLower(&self, src: &str) -> String;
}

/// 静态编码实现引用别名。
pub type EncodingRef = &'static dyn Encoding;

/// 判断字符集是否属于当前真正支持编解码处理的集合。
pub fn IsSupportedEncoding(charset: &str) -> bool {
    matches!(
        charset,
        CharsetUTF8MB4
            | CharsetUTF8
            | CharsetGBK
            | CharsetLatin1
            | CharsetBin
            | CharsetASCII
            | CharsetGB18030
    )
}

/// 查找编码；若为 UTF-8 则退化为 binary 空操作（部分路径无需真正 UTF-8 校验）。
pub fn FindEncodingTakeUTF8AsNoop(charset: &str) -> EncodingRef {
    let encoding = FindEncoding(charset);
    if encoding.Tp() == EncodingTpUTF8 {
        &ENCODING_BIN_IMPL
    } else {
        encoding
    }
}

/// 按字符集名查找编码实现；未知名回退到 binary。
pub fn FindEncoding(charset: &str) -> EncodingRef {
    match charset {
        CharsetUTF8MB4 | CharsetUTF8 => &ENCODING_UTF8_IMPL,
        CharsetGBK => &ENCODING_GBK_IMPL,
        CharsetLatin1 => &ENCODING_LATIN1_IMPL,
        CharsetASCII => &ENCODING_ASCII_IMPL,
        CharsetGB18030 => &ENCODING_GB18030_IMPL,
        _ => &ENCODING_BIN_IMPL,
    }
}

/// 统计从 UTF-8 方向看连续合法前缀的字节数。
pub fn CountValidBytes(encoding: EncodingRef, src: &[u8]) -> usize {
    count_valid(encoding, src, OP_FROM_UTF8)
}

/// 统计解码到 UTF-8 方向看连续合法前缀的字节数。
pub fn CountValidBytesDecode(encoding: EncodingRef, src: &[u8]) -> usize {
    count_valid(encoding, src, OP_TO_UTF8)
}

/// 用 Foreach 累加合法分块长度，遇到非法即停止。
fn count_valid(encoding: EncodingRef, src: &[u8], op: Op) -> usize {
    let mut count = 0;
    encoding.Foreach(src, op, &mut |from, _, ok| {
        if ok {
            count += from.len();
        }
        ok
    });
    count
}

/// 收尾 Transform：写回 dest，并按 SKIP_ERROR 决定是否返回 EncodingError。
pub(crate) fn finish_transform(
    dest: &mut Vec<u8>,
    output: Vec<u8>,
    first_invalid: Option<(&'static str, Vec<u8>)>,
    op: Op,
) -> Result<Vec<u8>, EncodingError> {
    dest.clear();
    dest.extend_from_slice(&output);
    if let Some((name, invalid)) = first_invalid {
        if op & OP_SKIP_ERROR == 0 {
            return Err(EncodingError::new(name, &invalid, output));
        }
    }
    Ok(output)
}

/// 处理非法字符：TRIM 则停止；REPLACE 则追加 `?` 并继续。
pub(crate) fn invalid_action(output: &mut Vec<u8>, op: Op) -> bool {
    if op & OP_TRUNCATE_TRIM != 0 {
        return false;
    }
    if op & OP_TRUNCATE_REPLACE != 0 {
        output.push(b'?');
    }
    true
}

/// 从源缓冲切出下一个 UTF-8 字符宽度；非法时返回首字节并标记失败。
pub(crate) fn utf8_chunk(src: &[u8]) -> (&[u8], bool) {
    if src.is_empty() {
        return (src, true);
    }
    // 按首字节估计宽度，再验证该切片是否为合法 UTF-8。
    let width = match src[0] {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => 1,
    };
    if src.len() >= width && std::str::from_utf8(&src[..width]).is_ok() {
        (&src[..width], true)
    } else {
        (&src[..1], false)
    }
}

/// 二进制编码：任意字节均合法，变换为拷贝透传。
struct EncodingBin;
static ENCODING_BIN_IMPL: EncodingBin = EncodingBin;

impl Encoding for EncodingBin {
    fn Name(&self) -> &'static str {
        CharsetBin
    }
    fn Tp(&self) -> EncodingTp {
        EncodingTpBin
    }
    fn Peek<'a>(&self, src: &'a [u8]) -> &'a [u8] {
        if src.is_empty() { src } else { &src[..1] }
    }
    fn MbLen(&self, _: &[u8]) -> usize {
        0
    }
    fn IsValid(&self, _: &[u8]) -> bool {
        true
    }
    fn Foreach(&self, src: &[u8], _: Op, callback: &mut dyn FnMut(&[u8], &[u8], bool) -> bool) {
        // 逐字节回调，始终标记合法。
        for byte in src.chunks(1) {
            if !callback(byte, byte, true) {
                break;
            }
        }
    }
    fn Transform(&self, _: &mut Vec<u8>, src: &[u8], _: Op) -> Result<Vec<u8>, EncodingError> {
        Ok(src.to_vec())
    }
    fn ToUpper(&self, src: &str) -> String {
        src.to_uppercase()
    }
    fn ToLower(&self, src: &str) -> String {
        src.to_lowercase()
    }
}

/// ASCII 编码：仅 0x00..=0x7F 合法；非法按 Op 替换或截断。
struct EncodingAscii;
static ENCODING_ASCII_IMPL: EncodingAscii = EncodingAscii;

impl Encoding for EncodingAscii {
    fn Name(&self) -> &'static str {
        CharsetASCII
    }
    fn Tp(&self) -> EncodingTp {
        EncodingTpASCII
    }
    fn Peek<'a>(&self, src: &'a [u8]) -> &'a [u8] {
        if src.is_empty() { src } else { &src[..1] }
    }
    fn MbLen(&self, _: &[u8]) -> usize {
        0
    }
    fn IsValid(&self, src: &[u8]) -> bool {
        src.iter().all(u8::is_ascii)
    }
    fn Foreach(&self, src: &[u8], _: Op, callback: &mut dyn FnMut(&[u8], &[u8], bool) -> bool) {
        let mut offset = 0;
        while offset < src.len() {
            // Go 对高位字节使用 EncodingUTF8Impl.Peek，仅按引导字节宽度
            // 聚合非法片段，并不先验证该 UTF-8 序列。
            let chunk = if src[offset].is_ascii() {
                &src[offset..offset + 1]
            } else {
                crate::encoding_utf8::peek_utf8(&src[offset..])
            };
            let ok = chunk.len() == 1 && chunk[0].is_ascii();
            if !callback(chunk, chunk, ok) {
                break;
            }
            offset += chunk.len();
        }
    }
    fn Transform(&self, dest: &mut Vec<u8>, src: &[u8], op: Op) -> Result<Vec<u8>, EncodingError> {
        if self.IsValid(src) {
            return Ok(src.to_vec());
        }
        let mut output = Vec::with_capacity(src.len());
        let mut first = None;
        self.Foreach(src, op, &mut |from, _, ok| {
            if ok {
                output.extend_from_slice(from);
                true
            } else {
                if first.is_none() {
                    first = Some((CharsetASCII, from.to_vec()));
                }
                invalid_action(&mut output, op)
            }
        });
        finish_transform(dest, output, first, op)
    }
    fn ToUpper(&self, src: &str) -> String {
        src.to_uppercase()
    }
    fn ToLower(&self, src: &str) -> String {
        src.to_lowercase()
    }
}
