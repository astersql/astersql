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

// TiDB 向后兼容语义下的 Latin1 字符集编码实现。
//
// 对应 Go `encoding_latin1.go`：复用 UTF-8 的 MbLen、Foreach 与大小写方法，
// 但 Peek 固定取单字节，任意字节序列均合法，Transform 直接透传输入。

use crate::encoding::*;

/// EncodingLatin1 对应 Go encodingLatin1，单字节透传编码。
pub struct EncodingLatin1;
/// 全局共享的 Latin1 编码实例。
pub static ENCODING_LATIN1_IMPL: EncodingLatin1 = EncodingLatin1;

impl Encoding for EncodingLatin1 {
    /// 返回字符集名称 `latin1`。
    fn Name(&self) -> &'static str {
        CharsetLatin1
    }
    /// 返回编码类型枚举值。
    fn Tp(&self) -> EncodingTp {
        EncodingTpLatin1
    }
    /// 空输入原样返回，否则只取首字节。
    fn Peek<'a>(&self, src: &'a [u8]) -> &'a [u8] {
        if src.is_empty() { src } else { &src[..1] }
    }
    /// 对齐嵌入的 Go encodingUTF8.MbLen，返回首个合法多字节 UTF-8 码点宽度。
    fn MbLen(&self, src: &[u8]) -> usize {
        let (chunk, valid) = utf8_chunk(src);
        if valid && chunk.len() > 1 {
            chunk.len()
        } else {
            0
        }
    }
    /// 任意字节序列均合法。
    fn IsValid(&self, _: &[u8]) -> bool {
        true
    }
    /// 对齐嵌入的 Go encodingUTF8.Foreach，按 UTF-8 码点分块并标记非法字节。
    fn Foreach(&self, src: &[u8], _: Op, callback: &mut dyn FnMut(&[u8], &[u8], bool) -> bool) {
        let mut offset = 0;
        while offset < src.len() {
            let (chunk, valid) = utf8_chunk(&src[offset..]);
            if !callback(chunk, chunk, valid) {
                break;
            }
            offset += chunk.len();
        }
    }
    /// 不做字符转换；与 Go 一样忽略 dest，并返回等值输出。
    fn Transform(&self, _: &mut Vec<u8>, src: &[u8], _: Op) -> Result<Vec<u8>, EncodingError> {
        Ok(src.to_vec())
    }
    /// 使用 Unicode 大写映射。
    fn ToUpper(&self, src: &str) -> String {
        src.to_uppercase()
    }
    /// 使用 Unicode 小写映射。
    fn ToLower(&self, src: &str) -> String {
        src.to_lowercase()
    }
}
