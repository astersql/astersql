// Copyright 2021 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// Charset conversion used by TiDB's internal `to_binary` and `from_binary`
// scalar functions. The parser charset package is also used here so its
// supported-encoding table and unknown-charset fallback stay identical to Go.
//
// 字符集转换：实现内部标量函数 `to_binary` / `from_binary`。
// 负责在 UTF-8 与 GBK/GB18030/Big5/日文/韩文等编码间互转，
// 并为字符串类内置函数决定是否需要包一层二进制转换包装。

use parser_charset_dependency::{FindEncoding, OpDecode, OpEncode};
use std::fmt;

/// 内部函数名：字符串按源字符集编码为二进制字节。
pub const INTERNAL_FUNC_TO_BINARY: &str = "to_binary";
/// 内部函数名：二进制字节按目标字符集解码为字符串。
pub const INTERNAL_FUNC_FROM_BINARY: &str = "from_binary";
/// 错误信息中最多展示的原始字节数（十六进制）。
pub const MAX_BYTES_TO_SHOW: usize = 6;

/// MySQL 字符集名称常量。
pub const CHARSET_BIN: &str = "binary";
pub const CHARSET_UTF8: &str = "utf8";
pub const CHARSET_UTF8MB4: &str = "utf8mb4";
pub const CHARSET_ASCII: &str = "ascii";
pub const CHARSET_LATIN1: &str = "latin1";
pub const CHARSET_GBK: &str = "gbk";
pub const CHARSET_GB18030: &str = "gb18030";
pub const CHARSET_BIG5: &str = "big5";
pub const CHARSET_SHIFT_JIS: &str = "sjis";
pub const CHARSET_EUC_KR: &str = "euckr";
pub const COLLATION_GBK_CHINESE_CI: &str = "gbk_chinese_ci";

#[derive(Clone, Debug, Eq, PartialEq)]
/// 字符集转换失败时的错误，含截断后的字节展示与源/目标字符集。
pub struct ConvertError {
    pub displayed_bytes: String,
    pub from_charset: String,
    pub to_charset: String,
}

impl ConvertError {
    /// 构造转换错误，字节按十六进制截断展示。
    fn new(input: &[u8], from_charset: &str, to_charset: &str) -> Self {
        Self {
            displayed_bytes: format_bytes(input),
            from_charset: from_charset.into(),
            to_charset: to_charset.into(),
        }
    }
}

impl fmt::Display for ConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cannot convert string '{}' from {} to {}",
            self.displayed_bytes, self.from_charset, self.to_charset
        )
    }
}

impl std::error::Error for ConvertError {}

/// 把字节格式化为大写十六进制，超出上限追加省略号。
fn format_bytes(input: &[u8]) -> String {
    let shown = input.iter().take(MAX_BYTES_TO_SHOW);
    let mut result: String = shown.map(|byte| format!("{byte:02X}")).collect();
    if input.len() > MAX_BYTES_TO_SHOW {
        result.push_str("...");
    }
    result
}

/// `to_binary`：UTF-8 字符串按指定字符集编码为字节；无法表示时返回错误。
pub fn encode_to_binary(input: &str, charset: &str) -> Result<Vec<u8>, ConvertError> {
    let mut encoded = Vec::with_capacity(input.len());
    FindEncoding(charset)
        .Transform(&mut encoded, input.as_bytes(), OpEncode)
        .map_err(|_| ConvertError::new(input.as_bytes(), CHARSET_UTF8MB4, charset))
}

/// 按 Go `OpDecode` 解码；失败时保留首个非法序列之前的已转换前缀。
fn decode_lossy(input: &[u8], charset: &str) -> (Vec<u8>, bool) {
    let mut decoded = Vec::with_capacity(input.len());
    let result = FindEncoding(charset).Transform(&mut decoded, input, OpDecode);
    match result {
        Ok(value) => (value, false),
        Err(_) => (decoded, true),
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// `from_binary` 解码策略：是否把不可转换当作警告，以及严格模式下是否置 NULL。
pub struct DecodeOptions {
    pub cannot_convert_as_warning: bool,
    pub strict_mode: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 累积转换警告（SQL 会话级 warning 的本地载体）。
pub struct WarningContext {
    pub warnings: Vec<ConvertError>,
}

/// `from_binary` 单值解码；按选项决定报错、警告+NULL 或警告+有损结果。
pub fn decode_binary(
    input: &[u8],
    charset: &str,
    options: DecodeOptions,
    context: &mut WarningContext,
) -> Result<Option<Vec<u8>>, ConvertError> {
    let (decoded, had_errors) = decode_lossy(input, charset);
    // 无损成功直接返回；否则按警告/严格模式分支。
    if !had_errors {
        return Ok(Some(decoded));
    }
    let error = ConvertError::new(input, CHARSET_BIN, charset);
    if !options.cannot_convert_as_warning {
        return Err(error);
    }
    context.warnings.push(error);
    if options.strict_mode {
        Ok(None)
    } else {
        Ok(Some(decoded))
    }
}

/// 按行执行 `to_binary`；任一非 NULL 行失败则整批失败。
pub fn encode_to_binary_rows(
    rows: &[Option<String>],
    charset: &str,
) -> Result<Vec<Option<Vec<u8>>>, ConvertError> {
    rows.iter()
        .map(|row| {
            row.as_deref()
                .map(|value| encode_to_binary(value, charset))
                .transpose()
        })
        .collect()
}

/// Vector decoding preserves Go's special non-strict warning path: the
/// original binary cell is appended when conversion fails.
/// 向量化 `from_binary`：非严格警告路径下失败时保留原始二进制单元（与 Go 一致）。
pub fn decode_binary_rows(
    rows: &[Option<Vec<u8>>],
    charset: &str,
    options: DecodeOptions,
    context: &mut WarningContext,
) -> Result<Vec<Option<Vec<u8>>>, ConvertError> {
    let mut result = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(input) = row else {
            result.push(None);
            continue;
        };
        let (decoded, had_errors) = decode_lossy(input, charset);
        if !had_errors {
            result.push(Some(decoded));
            continue;
        }
        let error = ConvertError::new(input, CHARSET_BIN, charset);
        if !options.cannot_convert_as_warning {
            return Err(error);
        }
        context.warnings.push(error);
        // 严格模式：失败行置 NULL；非严格：保留原始二进制字节（Go 特殊路径）。
        if options.strict_mode {
            result.push(None);
        } else {
            result.push(Some(input.clone()));
        }
    }
    Ok(result)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 字符串内置函数对二进制/字符集包装的敏感类别。
pub enum FunctionProperty {
    None,
    BinaryAware,
    Auto,
}

/// 不需要自动插入 to_binary/from_binary 的函数名列表。
const PROPERTY_NONE: &[&str] = &[
    "bin",
    "char",
    "date_format",
    "oct",
    "space",
    "char_length",
    "character_length",
    "from_base64",
    "lcase",
    "left",
    "load_file",
    "lower",
    "ltrim",
    "mid",
    "ord",
    "quote",
    "repeat",
    "reverse",
    "right",
    "rtrim",
    "soundex",
    "substr",
    "substring",
    "ucase",
    "unhex",
    "upper",
    "weight_string",
];

/// 按字节处理参数：非传统字符集实参需先 to_binary。
const PROPERTY_BINARY_AWARE: &[&str] = &[
    "ascii",
    "bit_length",
    "hex",
    "length",
    "octet_length",
    "to_base64",
    "aes_decrypt",
    "decode",
    "encode",
    "password",
    "md5",
    "sha",
    "sha1",
    "sha2",
    "sm3",
    "compress",
    "aes_encrypt",
];

/// 结果字符集驱动：实参与结果 charset 不一致时自动包装。
const PROPERTY_AUTO: &[&str] = &[
    "concat",
    "concat_ws",
    "export_set",
    "field",
    "find_in_set",
    "insert",
    "instr",
    "lpad",
    "locate",
    "make_set",
    "position",
    "replace",
    "rpad",
    "substring_index",
    "trim",
    "elt",
    "ge",
    "le",
    "gt",
    "lt",
    "eq",
    "ne",
    "nulleq",
    "if",
    "ifnull",
    "in",
    "case",
    "cast",
    "like",
    "ilike",
    "strcmp",
    "regexp",
    "regexp_like",
    "regexp_instr",
    "regexp_substr",
    "regexp_replace",
    "crc32",
];

/// 按函数名查询字符集转换属性；未登记的函数视为 None。
pub fn conversion_property(function_name: &str) -> FunctionProperty {
    if PROPERTY_BINARY_AWARE.contains(&function_name) {
        FunctionProperty::BinaryAware
    } else if PROPERTY_AUTO.contains(&function_name) {
        FunctionProperty::Auto
    } else {
        // Go 对未登记函数同样返回 None（funcPropNone）。
        // Go map lookup returns funcPropNone for absent functions as well.
        let _known_none = PROPERTY_NONE.contains(&function_name);
        FunctionProperty::None
    }
}

/// 是否为无需外部编码库的“传统”字符集（utf8/ascii/latin1/binary）。
pub fn is_legacy_charset(charset: &str) -> bool {
    matches!(
        charset,
        CHARSET_UTF8 | CHARSET_UTF8MB4 | CHARSET_ASCII | CHARSET_LATIN1 | CHARSET_BIN
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 表达式结果的字符集与校对规则（collation）。
pub struct ExpressionCollation {
    pub charset: String,
    pub collation: String,
}

impl ExpressionCollation {
    /// 构造字符集/校对对。
    pub fn new(charset: &str, collation: &str) -> Self {
        Self {
            charset: charset.into(),
            collation: collation.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// `HandleBinaryLiteral` 的决策结果：保持不变、包 to_binary 或 from_binary。
pub enum WrapperAction {
    Unchanged,
    ToBinary,
    FromBinary {
        target: ExpressionCollation,
        cannot_convert_as_warning: bool,
    },
}

/// Mirrors `HandleBinaryLiteral`. `expression_is_null` prevents wrapping a
/// binary NULL with `from_binary`, matching the Go type check.
/// 对应 Go `HandleBinaryLiteral`：根据函数属性、实参 charset 与结果 collation 决定包装；
/// `expression_is_null` 为真时不对二进制 NULL 包 `from_binary`。
pub fn handle_binary_literal(
    argument_charset: &str,
    result_collation: &ExpressionCollation,
    function_name: &str,
    expression_is_null: bool,
    explicit_cast: bool,
) -> WrapperAction {
    match conversion_property(function_name) {
        FunctionProperty::None => WrapperAction::Unchanged,
        FunctionProperty::BinaryAware => {
            if is_legacy_charset(argument_charset) {
                WrapperAction::Unchanged
            } else {
                WrapperAction::ToBinary
            }
        }
        FunctionProperty::Auto => {
            if argument_charset != CHARSET_BIN && result_collation.charset == CHARSET_BIN {
                if is_legacy_charset(argument_charset) {
                    WrapperAction::Unchanged
                } else {
                    WrapperAction::ToBinary
                }
            } else if argument_charset == CHARSET_BIN
                && result_collation.charset != CHARSET_BIN
                && !expression_is_null
            {
                WrapperAction::FromBinary {
                    target: result_collation.clone(),
                    cannot_convert_as_warning: explicit_cast,
                }
            } else {
                WrapperAction::Unchanged
            }
        }
    }
}
