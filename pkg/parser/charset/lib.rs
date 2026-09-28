// Copyright 2026 AsterSQL.

// parser 字符集与编码 crate 入口。
//
// 字符集（charset）描述字符串的逻辑名称（如 `utf8mb4`、`gbk`、`binary`）；
// 编码（encoding）负责字节序列与 Unicode 之间的转换、校验与 Peek。
// 本 crate 汇总 ASCII/Binary/Latin1/GBK/GB18030/UTF-8 等实现，并导出
// 转换操作位（Op）、编码查找与测试子模块。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as parser_charset;

/// 错误类型别名，对齐 Go 侧 errors 包。
pub use astersql_errors as errors;
/// 嵌套 `parser::mysql` / `parser::terror`，方便 include! 子文件引用。
pub mod parser {
    /// MySQL 协议相关常量、字符集名与错误码再导出。
    pub mod mysql {
        pub use astersql_parser_mysql::*;
        pub use astersql_parser_mysql::{charset::*, errcode::*};
    }

    /// terror（类型化错误）再导出。
    pub mod terror {
        pub use astersql_parser_terror::*;
    }
}
pub use parser::{mysql, terror};

/// 可变字节缓冲，对应 Go 的 bytes.Buffer 用法场景。
pub type ByteBuffer = Vec<u8>;
pub use ::encoding::{DecoderTrap, EncoderTrap};
/// 编码转换错误类型。
pub type EncodingError = errors::SharedError;
/// 编码转换操作位集合（位标志）。
pub type Op = i16;
/// 从 UTF-8 方向转换。
pub const OP_FROM_UTF8: Op = 1 << 0;
/// 向 UTF-8 方向转换。
pub const OP_TO_UTF8: Op = 1 << 1;
/// 非法字节截断丢弃。
pub const OP_TRUNCATE_TRIM: Op = 1 << 2;
/// 非法字节替换为占位符。
pub const OP_TRUNCATE_REPLACE: Op = 1 << 3;
/// 收集源侧非法片段。
pub const OP_COLLECT_FROM: Op = 1 << 4;
/// 收集目标侧非法片段。
pub const OP_COLLECT_TO: Op = 1 << 5;
/// 遇到错误时跳过而不中止。
pub const OP_SKIP_ERROR: Op = 1 << 6;
/// 常用组合：从 UTF-8 替换并收集源侧错误，且跳过错误。
pub const OpReplaceNoErr: Op = OP_FROM_UTF8 | OP_TRUNCATE_REPLACE | OP_COLLECT_FROM | OP_SKIP_ERROR;

/// 精简版编码类型枚举（crate 入口侧占位，完整枚举在 encoding 子模块）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodingTp {
    /// ASCII 编码。
    Ascii,
    /// 二进制（透传）编码。
    Bin,
}

/// 编码实例引用句柄，用于惰性初始化后的查找。
#[derive(Clone, Copy)]
pub enum EncodingRef {
    /// ASCII 实例。
    Ascii,
    /// Binary 实例。
    Bin,
}

/// 转换结果：可借用原切片或拥有新缓冲。
pub enum TransformResult<'a> {
    /// 零拷贝借用源字节。
    Borrowed(&'a [u8]),
    /// 需要改写时的自有缓冲。
    Owned(Vec<u8>),
}

impl TransformResult<'_> {
    /// 统一以切片视图访问转换结果。
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::Borrowed(bytes) => bytes,
            Self::Owned(bytes) => bytes,
        }
    }
}

/// 编码工厂枚举；当前仅提供 Nop（恒等）编码器。
#[derive(Clone, Copy)]
pub enum Encoding {
    /// 空操作编码，字节原样拷贝。
    Nop,
}

/// 转换失败信息，记录已写入的字节数。
pub struct TransformFailure {
    /// 失败前已成功写入目标缓冲的字节数。
    written: usize,
}

impl TransformFailure {
    /// 返回失败前已写入的字节数。
    pub fn written(&self) -> usize {
        self.written
    }
}

/// 流式编码器/解码器接口，对应 Go encoding.Transformer。
pub trait Transformer {
    /// 将 `src` 转换写入 `dst`；`at_eof` 表示输入是否已结束。
    fn transform(
        &mut self,
        dst: &mut [u8],
        src: &[u8],
        at_eof: bool,
    ) -> Result<usize, TransformFailure>;

    /// 上一次输入是否以不完整的多字节序列（rune error）结尾。
    fn rune_error_is_last_input(&self) -> bool {
        false
    }
}

/// Nop 转换器：按目标缓冲容量逐字节拷贝。
struct NopTransformer;

impl Transformer for NopTransformer {
    fn transform(
        &mut self,
        dst: &mut [u8],
        src: &[u8],
        _at_eof: bool,
    ) -> Result<usize, TransformFailure> {
        let written = dst.len().min(src.len());
        dst[..written].copy_from_slice(&src[..written]);
        Ok(written)
    }
}

impl Encoding {
    /// 创建编码器（当前均为 Nop）。
    pub fn new_encoder(self) -> Box<dyn Transformer> {
        Box::new(NopTransformer)
    }

    /// 创建解码器（当前均为 Nop）。
    pub fn new_decoder(self) -> Box<dyn Transformer> {
        Box::new(NopTransformer)
    }
}

/// 返回轻量 UTF-8 Peek 辅助结构实例。
pub fn encoding_utf8_impl() -> Utf8Encoding {
    Utf8Encoding
}

/// 仅提供 Peek 的轻量 UTF-8 辅助类型（完整 Encoding 在 encoding_utf8 模块）。
pub struct Utf8Encoding;

impl Utf8Encoding {
    /// 按首字节合法 UTF-8 前缀宽度窥视下一个字符切片。
    pub fn peek<'a>(&self, src: &'a [u8]) -> &'a [u8] {
        if src.is_empty() {
            return src;
        }
        // 与 Go encodingUTF8.Peek 一致，只按首字节阈值决定宽度；即使首字节
        // 不是合法 UTF-8 引导字节，也保留 Go 用于错误分组的 2/3/4 字节边界。
        let width = match src[0] {
            0x00..=0x7f => 1,
            0x80..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xff => 4,
        };
        &src[..width.min(src.len())]
    }
}

/// 统一的编码只读视图：名称、Peek 与逐字符遍历。
pub trait EncodingView {
    /// 返回字符集/编码名称。
    fn name(&self) -> &'static str;
    /// 窥视下一个字符字节切片。
    fn peek<'a>(&self, src: &'a [u8]) -> &'a [u8];
    /// 逐字符回调遍历；回调返回 false 时停止。
    fn foreach(&self, src: &[u8], op: Op, callback: &mut dyn FnMut(&[u8], &[u8], bool) -> bool);
}

/// 根据编码引用取出已初始化的静态 EncodingView。
pub fn encoding_by_ref(encoding: EncodingRef) -> &'static dyn EncodingView {
    match encoding {
        EncodingRef::Ascii => encoding_ascii::ENCODING_ASCII_IMPL
            .get()
            .expect("ASCII initialized"),
        EncodingRef::Bin => encoding_bin::ENCODING_BIN_IMPL
            .get()
            .expect("binary initialized"),
    }
}

/// ASCII 字符集名称常量。
pub const CHARSET_ASCII: &str = "ascii";
/// Binary 字符集名称常量。
pub const CHARSET_BIN: &str = "binary";

/// 通用编码查找与操作位定义。
pub mod encoding;
/// GB18030 编码实现。
pub mod encoding_gb18030;
/// GBK 编码实现。
pub mod encoding_gbk;
/// Latin1 编码实现。
pub mod encoding_latin1;
/// 编码查找表。
pub mod encoding_table;
/// UTF-8 / utf8mb4 编码实现。
pub mod encoding_utf8;

pub use encoding::{
    CountValidBytes, CountValidBytesDecode, EncodingTpBin, FindEncoding,
    FindEncodingTakeUTF8AsNoop, IsSupportedEncoding, OpDecode, OpDecodeNoErr, OpDecodeReplace,
    OpEncode, OpEncodeNoErr, OpEncodeReplace, OpReplace,
};
pub use encoding_gb18030::*;
pub use encoding_gbk::*;
pub use encoding_latin1::*;
pub use encoding_table::*;
pub use encoding_utf8::*;

/// 字符集元数据（通过 include! 嵌入 charset.rs）。
pub mod charset {
    use crate::{errors, mysql, terror};
    include!("charset.rs");
}
pub use charset::{
    CharsetASCII, CharsetBin, CharsetGB18030, CharsetGBK, CharsetLatin1, CharsetUTF8,
    CharsetUTF8MB4,
};

/// 编码基类（EncodingBase）实现。
pub mod encoding_base {
    use crate::*;
    include!("encoding_base.rs");
}
pub use encoding_base::*;

/// ASCII 编码实现。
pub mod encoding_ascii {
    use crate::*;
    include!("encoding_ascii.rs");
}
pub use encoding_ascii::*;

/// Binary 编码实现。
pub mod encoding_bin {
    use crate::*;
    include!("encoding_bin.rs");
}
pub use encoding_bin::*;

/// GB18030 码表数据。
pub mod encoding_gb18030_data;
pub use encoding_gb18030_data::*;

impl EncodingView for encoding_ascii::EncodingAscii {
    fn name(&self) -> &'static str {
        self.name()
    }

    fn peek<'a>(&self, src: &'a [u8]) -> &'a [u8] {
        self.peek(src)
    }

    fn foreach(&self, src: &[u8], op: Op, callback: &mut dyn FnMut(&[u8], &[u8], bool) -> bool) {
        self.foreach(src, op, |from, to, ok| callback(from, to, ok))
    }
}

impl EncodingView for encoding_bin::EncodingBin {
    fn name(&self) -> &'static str {
        self.name()
    }

    fn peek<'a>(&self, src: &'a [u8]) -> &'a [u8] {
        self.peek(src)
    }

    fn foreach(&self, src: &[u8], op: Op, callback: &mut dyn FnMut(&[u8], &[u8], bool) -> bool) {
        self.foreach(src, op, |from, to, ok| callback(from, to, ok))
    }
}

#[cfg(test)]
#[path = "charset_1_aster_unit_test.rs"]
mod charset_1_aster_unit_test;
#[cfg(test)]
#[path = "charset_test.rs"]
mod charset_test;
#[cfg(test)]
#[path = "encoding_ascii_test.rs"]
mod encoding_ascii_test;
#[cfg(test)]
#[path = "encoding_gb18030_2_aster_unit_test.rs"]
mod encoding_gb18030_2_aster_unit_test;
#[cfg(test)]
#[path = "encoding_gb18030_data_test.rs"]
mod encoding_gb18030_data_test;
#[cfg(test)]
#[path = "encoding_gbk_test.rs"]
mod encoding_gbk_test;
#[cfg(test)]
#[path = "encoding_latin1_test.rs"]
mod encoding_latin1_test;
#[cfg(test)]
#[path = "encoding_table_test.rs"]
mod encoding_table_test;
#[cfg(test)]
#[path = "encoding_test.rs"]
mod encoding_test;
#[cfg(test)]
#[path = "encoding_utf8_test.rs"]
mod encoding_utf8_test;
