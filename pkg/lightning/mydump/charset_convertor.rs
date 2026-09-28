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
// Copyright 2026 AsterSQL.

// mydump 字符集转换器：将源编码字节解码为 Unicode，再按需编码回源编码。
//
// 支持 binary/utf8mb4/ascii/gb18030/gbk/latin1；不可表示字符用替换串填充。
// 对应 Go `CharsetConvertor`，供导入前规范化 dump 文件文本。

use encoding_rs::{Encoding, GB18030, GBK, WINDOWS_1252};

use crate::MydumpError;

/// 导入源文件使用的字符集枚举。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Charset {
    Binary,
    Utf8Mb4,
    Ascii,
    Gb18030,
    Gbk,
    Latin1,
}

impl Charset {
    /// 解析配置字符串为 Charset；未知名称返回 Configuration 错误。
    fn parse(value: &str) -> Result<Self, MydumpError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "binary" => Ok(Self::Binary),
            "utf8" | "utf8mb4" => Ok(Self::Utf8Mb4),
            "ascii" => Ok(Self::Ascii),
            "gb18030" => Ok(Self::Gb18030),
            "gbk" => Ok(Self::Gbk),
            "latin1" => Ok(Self::Latin1),
            other => Err(MydumpError::Configuration(format!(
                "unknown charset {other}"
            ))),
        }
    }
    /// 返回需要经 encoding_rs 转换的编码；binary/utf8/ascii 为 None（直通）。
    fn encoding(self) -> Option<&'static Encoding> {
        match self {
            Self::Gb18030 => Some(GB18030),
            Self::Gbk => Some(GBK),
            // MySQL latin1 在实践中按 Windows-1252 处理
            Self::Latin1 => Some(WINDOWS_1252),
            _ => None,
        }
    }
}

/// 持有源字符集与非法字符替换串的转换器。
#[derive(Clone, Debug)]
pub struct CharsetConvertor {
    source_character_set: Charset,
    invalid_char_replacement: String,
}

/// 按字符集名与替换串创建转换器，并执行编解码器校验初始化。
pub fn NewCharsetConvertor(
    character_set: &str,
    replacement: &str,
) -> Result<CharsetConvertor, MydumpError> {
    let mut result = CharsetConvertor {
        source_character_set: Charset::parse(character_set)?,
        invalid_char_replacement: replacement.into(),
    };
    result.initDecoder()?;
    result.initEncoder()?;
    Ok(result)
}
/// 蛇形命名别名，转发到 `NewCharsetConvertor`。
pub fn new_charset_convertor(
    character_set: &str,
    replacement: &str,
) -> Result<CharsetConvertor, MydumpError> {
    NewCharsetConvertor(character_set, replacement)
}

impl CharsetConvertor {
    /// 初始化解码路径：当前仅校验字符集是否受支持。
    pub fn initDecoder(&mut self) -> Result<(), MydumpError> {
        self.validate()
    }
    /// 初始化编码路径：当前仅校验字符集是否受支持。
    pub fn initEncoder(&mut self) -> Result<(), MydumpError> {
        self.validate()
    }
    /// 确认枚举变体均合法（穷尽匹配）。
    fn validate(&self) -> Result<(), MydumpError> {
        match self.source_character_set {
            Charset::Binary
            | Charset::Utf8Mb4
            | Charset::Ascii
            | Charset::Gb18030
            | Charset::Gbk
            | Charset::Latin1 => Ok(()),
        }
    }
    /// 非空且源编码需要转换时才走 encoding_rs。
    fn precheck(&self, src: &[u8]) -> bool {
        !src.is_empty() && self.source_character_set.encoding().is_some()
    }
    /// 将源编码字节解码为 String；直通路径按 UTF-8 校验；替换 U+FFFD。
    pub fn Decode(&self, src: &[u8]) -> Result<String, MydumpError> {
        if !self.precheck(src) {
            return String::from_utf8(src.to_vec())
                .map_err(|e| MydumpError::Encoding(e.to_string()));
        }
        let encoding = self.source_character_set.encoding().unwrap();
        let decode_invalid = |bytes: &[u8]| {
            let (decoded, _, _) = encoding.decode(bytes);
            decoded.replace('\u{fffd}', &self.invalid_char_replacement)
        };

        // Go 的自定义 GB18030 解码器区分合法编码的 U+FFFD 与解码器为非法字节
        // 产生的 U+FFFD。encoding_rs 的便捷 API 会把二者合并，因此按合法四字节序列
        // 切段解码，并只替换各段中由错误产生的 U+FFFD。
        if self.source_character_set == Charset::Gb18030 {
            const ENCODED_REPLACEMENT_CHARACTER: &[u8] = &[0x84, 0x31, 0xa4, 0x37];
            let mut result = String::new();
            let mut rest = src;
            while let Some(index) = rest
                .windows(ENCODED_REPLACEMENT_CHARACTER.len())
                .position(|window| window == ENCODED_REPLACEMENT_CHARACTER)
            {
                result.push_str(&decode_invalid(&rest[..index]));
                result.push('\u{fffd}');
                rest = &rest[index + ENCODED_REPLACEMENT_CHARACTER.len()..];
            }
            result.push_str(&decode_invalid(rest));
            return Ok(result);
        }

        Ok(decode_invalid(src))
    }
    /// 将 Unicode 文本编码回源字符集；无法表示时返回 Encoding 错误。
    pub fn Encode(&self, src: &str) -> Result<Vec<u8>, MydumpError> {
        let Some(encoding) = self.source_character_set.encoding() else {
            return Ok(src.as_bytes().to_vec());
        };
        let (encoded, _, had_errors) = encoding.encode(src);
        if had_errors {
            return Err(MydumpError::Encoding(format!(
                "text is not representable in {:?}",
                self.source_character_set
            )));
        }
        Ok(encoded.into_owned())
    }
    /// 蛇形命名别名，转发到 `Decode`。
    pub fn decode(&self, src: &[u8]) -> Result<String, MydumpError> {
        self.Decode(src)
    }
    /// 蛇形命名别名，转发到 `Encode`。
    pub fn encode(&self, src: &str) -> Result<Vec<u8>, MydumpError> {
        self.Encode(src)
    }
}
