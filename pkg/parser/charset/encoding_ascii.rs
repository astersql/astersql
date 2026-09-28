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

// ASCII 字符集编码实现。
//
// 对应 Go `encoding_ascii.go`：提供 ASCII 字节探测、合法性校验、转换与逐字符遍历。
// 字符集（charset）决定字符串在存储与比较时如何解释字节序列；ASCII 仅接受 0x00–0x7F。

// ASCII 字节的探测、校验、转换和逐字符遍历，不执行数据库连接、SQL 或其它业务动作。
// bytes.Buffer、encoding.Nop 以及同包编码类型均保留为后续模块接线所需的外部依赖形状。

// EncodingASCIIImpl 对应 Go 的全局 ASCII 编码实例；OnceLock 表达 init 完成 self 回指后的共享只读实例。
/// 全局共享的 ASCII 编码单例；初始化后通过 OnceLock 保证只写一次。
pub static ENCODING_ASCII_IMPL: std::sync::OnceLock<EncodingAscii> = std::sync::OnceLock::new();

// init_encoding_ascii 对应 Go init：建立 encodingBase.self 指向实例自身的关系。
/// 初始化全局 ASCII 编码实例，并设置 encodingBase 对自身的回指。
pub fn init_encoding_ascii() {
    let mut encoding = EncodingAscii {
        encoding_base: EncodingBase::new(Encoding::Nop),
    };
    encoding.encoding_base.set_self(EncodingRef::Ascii);
    let _ = ENCODING_ASCII_IMPL.set(encoding);
}

/// EncodingAscii 对应 Go encodingASCII，复用 encodingBase 的通用转换逻辑。
pub struct EncodingAscii {
    pub encoding_base: EncodingBase,
}

impl EncodingAscii {
    /// name 对应 Encoding.Name，返回同包定义的 ASCII 字符集名称。
    pub fn name(&self) -> &'static str {
        CHARSET_ASCII
    }

    /// tp 对应 Encoding.Tp，返回 ASCII 编码枚举值。
    pub fn tp(&self) -> EncodingTp {
        EncodingTp::Ascii
    }

    /// mb_len 对应 Go 匿名嵌入的 encodingBase.MbLen；ASCII 没有多字节字符。
    pub fn mb_len(&self, src: &str) -> usize {
        self.encoding_base.mb_len(src)
    }

    /// to_upper 对应 Go 匿名嵌入的 encodingBase.ToUpper。
    pub fn to_upper(&self, src: &str) -> String {
        self.encoding_base.to_upper(src)
    }

    /// to_lower 对应 Go 匿名嵌入的 encodingBase.ToLower。
    pub fn to_lower(&self, src: &str) -> String {
        self.encoding_base.to_lower(src)
    }

    /// peek 对应 Encoding.Peek；空输入原样返回，否则只选取首字节。
    pub fn peek<'a>(&self, src: &'a [u8]) -> &'a [u8] {
        if src.is_empty() {
            return src;
        }
        &src[..1]
    }

    /// is_valid 对应 Encoding.IsValid，逐字节拒绝所有高于 ASCII 上限的值。
    pub fn is_valid(&self, src: &[u8]) -> bool {
        for byte in src {
            if *byte > 0x7f {
                return false;
            }
        }
        true
    }

    /// transform 对应 Encoding.Transform；合法 ASCII 直接借用原输入，非法输入交给通用转换器处理。
    pub fn transform<'a>(
        &self,
        dest: Option<&mut ByteBuffer>,
        src: &'a [u8],
        op: Op,
    ) -> Result<TransformResult<'a>, EncodingError> {
        if self.is_valid(src) {
            return Ok(TransformResult::Borrowed(src));
        }
        self.encoding_base.transform(dest, src, op)
    }

    /// foreach 对应 Encoding.Foreach，按 ASCII 字节边界回调；非法高位字节按 UTF-8 探测宽度整体报告。
    pub fn foreach<F>(&self, src: &[u8], _op: Op, mut callback: F)
    where
        F: FnMut(&[u8], &[u8], bool) -> bool,
    {
        let mut index = 0;
        while index < src.len() {
            let mut width = 1;
            let mut ok = true;
            if src[index] > 0x7f {
                // Go 借助 EncodingUTF8Impl.Peek 跳过一个完整 UTF-8 序列，避免逐个报告其续字节。
                width = encoding_utf8_impl().peek(&src[index..]).len();
                ok = false;
            }
            let end = index + width;
            if !callback(&src[index..end], &src[index..end], ok) {
                return;
            }
            index = end;
        }
    }
}
