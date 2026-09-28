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

// UTF-8 / utf8mb4 字符集编码实现。
//
// MySQL 中 `utf8mb4` 是完整的 UTF-8（最多 4 字节/字符），而历史名 `utf8`
// 实际是最多 3 字节的 utf8mb3。本模块实现 `EncodingUtf8`：按 UTF-8 规则
// 探测、校验与转换字节序列；`strict_mb3` 为真时拒绝 4 字节码点，对应
// utf8mb3 严格模式。

use crate::encoding::*;

/// UTF-8 编码处理器。
///
/// `strict_mb3` 控制是否拒绝超过 3 字节的 Unicode 码点（emoji 等需 4 字节）。
pub struct EncodingUtf8 {
    /// 为真时按 utf8mb3 严格校验：每个字符编码长度不得超过 3 字节。
    strict_mb3: bool,
}

/// 默认 utf8mb4 编码实例（允许 4 字节字符）。
pub static ENCODING_UTF8_IMPL: EncodingUtf8 = EncodingUtf8 { strict_mb3: false };
/// utf8mb3 严格模式实例（拒绝 4 字节字符）。
static ENCODING_UTF8_MB3_STRICT_IMPL: EncodingUtf8 = EncodingUtf8 { strict_mb3: true };

/// 返回 utf8mb3 严格模式编码的引用，对应 Go 的 EncodingUTF8MB3StrictImpl。
pub fn EncodingUTF8MB3StrictImpl() -> EncodingRef {
    &ENCODING_UTF8_MB3_STRICT_IMPL
}

/// 按首字节高位探测下一个 UTF-8 字符的字节宽度并截取前缀。
///
/// 不校验后续字节是否合法；仅用于 Peek 等“看一眼”场景。
pub(crate) fn peek_utf8(src: &[u8]) -> &[u8] {
    if src.is_empty() {
        return src;
    }
    // 根据首字节区间判断 1/2/3/4 字节宽度，再与剩余长度取 min。
    let width = match src[0] {
        0x00..=0x7f => 1,
        0x80..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    };
    &src[..src.len().min(width)]
}

impl EncodingUtf8 {
    /// 取出从 `src` 开头起的一个 UTF-8 码点切片，并报告是否有效。
    ///
    /// 在 `strict_mb3` 模式下，即使 UTF-8 合法，长度 > 3 也视为无效。
    fn valid_chunk<'a>(&self, src: &'a [u8]) -> (&'a [u8], bool) {
        let (chunk, valid) = utf8_chunk(src);
        (chunk, valid && (!self.strict_mb3 || chunk.len() <= 3))
    }
}

impl Encoding for EncodingUtf8 {
    /// 字符集名称固定为 utf8mb4（与 Go 侧 Name 一致）。
    fn Name(&self) -> &'static str {
        CharsetUTF8MB4
    }
    /// 返回编码类型枚举：严格 mb3 或普通 UTF-8。
    fn Tp(&self) -> EncodingTp {
        if self.strict_mb3 {
            EncodingTpUTF8MB3Strict
        } else {
            EncodingTpUTF8
        }
    }
    /// 窥视下一个字符的字节切片（不强制完整校验）。
    fn Peek<'a>(&self, src: &'a [u8]) -> &'a [u8] {
        peek_utf8(src)
    }
    /// 返回多字节字符长度；单字节 ASCII 或非法序列返回 0。
    fn MbLen(&self, src: &[u8]) -> usize {
        let (chunk, valid) = utf8_chunk(src);
        if valid && chunk.len() > 1 {
            chunk.len()
        } else {
            0
        }
    }
    /// 判断整段字节是否为合法 UTF-8；严格模式下还要求无 4 字节字符。
    fn IsValid(&self, src: &[u8]) -> bool {
        let Ok(text) = std::str::from_utf8(src) else {
            return false;
        };
        !self.strict_mb3 || text.chars().all(|character| character.len_utf8() <= 3)
    }
    /// 逐字符遍历：对每个码点调用回调，回调返回 false 时提前结束。
    fn Foreach(&self, src: &[u8], _: Op, callback: &mut dyn FnMut(&[u8], &[u8], bool) -> bool) {
        let mut offset = 0;
        while offset < src.len() {
            let (chunk, ok) = self.valid_chunk(&src[offset..]);
            if !callback(chunk, chunk, ok) {
                break;
            }
            offset += chunk.len();
        }
    }
    /// 按操作位 `op` 将源字节转换到目标缓冲；合法输入可直接拷贝。
    ///
    /// 非法码点由 `invalid_action` 按截断/替换等策略处理，最后经
    /// `finish_transform` 汇总首次错误信息。
    fn Transform(&self, dest: &mut Vec<u8>, src: &[u8], op: Op) -> Result<Vec<u8>, EncodingError> {
        // 整段合法时走快速路径：直接复制，避免逐字符扫描。
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
                // 记录首个非法片段，供错误报告使用。
                if first.is_none() {
                    first = Some((self.Name(), from.to_vec()));
                }
                invalid_action(&mut output, op)
            }
        });
        finish_transform(dest, output, first, op)
    }
    /// Unicode 大小写转换：转大写。
    fn ToUpper(&self, src: &str) -> String {
        src.to_uppercase()
    }
    /// Unicode 大小写转换：转小写。
    fn ToLower(&self, src: &str) -> String {
        src.to_lowercase()
    }
}
