// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 二进制（binary）字符集编码实现。
//
// 对应 Go `encoding_bin.go`：按原始字节探测、遍历与透传，不做字符语义校验。
// 二进制字符集常用于 BLOB 等按字节比较的列，任意字节序列均视为合法。

// 这段逻辑只按原始字节探测、遍历和透传数据。
// bytes.Buffer 与 encoding.Nop 的依赖形状由同包后续统一接线。
/// EncodingBinImpl 对应 Go 的全局二进制编码实例。
pub static ENCODING_BIN_IMPL: std::sync::OnceLock<EncodingBin> = std::sync::OnceLock::new();

/// init_encoding_bin 对应 Go init，补齐 encodingBase.self 的实例回指。
pub fn init_encoding_bin() {
    let mut encoding = EncodingBin {
        encoding_base: EncodingBase::new(Encoding::Nop),
    };
    encoding.encoding_base.set_self(EncodingRef::Bin);
    let _ = ENCODING_BIN_IMPL.set(encoding);
}

/// EncodingBin 对应 Go encodingBin；二进制字符始终以单字节处理。
pub struct EncodingBin {
    pub encoding_base: EncodingBase,
}

impl EncodingBin {
    /// name 对应 Encoding.Name。
    pub fn name(&self) -> &'static str {
        CHARSET_BIN
    }

    /// tp 对应 Encoding.Tp。
    pub fn tp(&self) -> EncodingTp {
        EncodingTp::Bin
    }

    /// peek 对应 Encoding.Peek；空输入保持为空，非空输入返回首字节。
    pub fn peek<'a>(&self, src: &'a [u8]) -> &'a [u8] {
        if src.is_empty() {
            return src;
        }
        &src[..1]
    }

    /// is_valid 对应 Go 恒真实现：二进制编码接受任意字节序列。
    pub fn is_valid(&self, _src: &[u8]) -> bool {
        true
    }

    /// foreach 对应 Go 的逐字节回调；from/to 均指向相同字节并始终标记有效。
    pub fn foreach<F>(&self, src: &[u8], _op: Op, mut callback: F)
    where
        F: FnMut(&[u8], &[u8], bool) -> bool,
    {
        for index in 0..src.len() {
            let byte = &src[index..index + 1];
            if !callback(byte, byte, true) {
                return;
            }
        }
    }

    /// transform 对应 Go 的零转换路径，忽略目标缓冲区和操作位并直接借用原输入。
    pub fn transform<'a>(
        &self,
        _dest: Option<&mut ByteBuffer>,
        src: &'a [u8],
        _op: Op,
    ) -> Result<TransformResult<'a>, EncodingError> {
        Ok(TransformResult::Borrowed(src))
    }
}
