// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// AST 节点基类与二进制字符串字面量 UTF-8 规范化。
//
// `AstNode` 保存原始字节、编码与缓存的可读文本；`convertBinaryStringLiterals`
// 将不可打印的引号字符串转为 `0x...` 十六进制，便于日志与 Restore，并跳过普通注释。
use crate::ast::FieldType;

pub use parser_charset::encoding::EncodingRef;
use parser_charset::encoding::{OpDecode, OpDecodeReplace};

/// 十六进制编码所用小写数字表。
const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// Empty AST nodes do not allocate source storage; parser reductions fill it on demand.
#[derive(Clone, Default, Debug)]
pub struct AstNode {
    text: Option<Box<NodeText>>,
}

#[derive(Clone, Default)]
struct NodeText {
    utf8_text: std::cell::OnceCell<String>,
    encoding: Option<EncodingRef>,
    no_backslash_escapes: bool,
    text: Vec<u8>,
    offset: i32,
}

impl std::fmt::Debug for NodeText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeText")
            .field("text", &self.text)
            .field("encoding", &self.encoding.map(|encoding| encoding.Name()))
            .field("no_backslash_escapes", &self.no_backslash_escapes)
            .field("offset", &self.offset)
            .finish()
    }
}
impl PartialEq for NodeText {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
            && self.offset == other.offset
            && self.no_backslash_escapes == other.no_backslash_escapes
            && self.encoding.map(|encoding| encoding.Name())
                == other.encoding.map(|encoding| encoding.Name())
    }
}
impl PartialEq for AstNode {
    fn eq(&self, other: &Self) -> bool {
        match (&self.text, &other.text) {
            (Some(left), Some(right)) => left == right,
            (None, None) => true,
            (Some(text), None) | (None, Some(text)) => **text == NodeText::default(),
        }
    }
}
impl Eq for AstNode {}

impl AstNode {
    fn text_mut(&mut self) -> &mut NodeText {
        self.text.get_or_insert_with(Default::default)
    }

    /// 设置节点在源 SQL 中的起始偏移。
    pub fn SetOriginTextPosition(&mut self, offset: i32) {
        self.text_mut().offset = offset;
    }

    /// 返回源 SQL 中的起始偏移。
    pub fn OriginTextPosition(&self) -> i32 {
        self.text.as_ref().map_or(0, |text| text.offset)
    }

    /// 设置原文与编码，并清空 UTF-8 缓存。
    pub fn SetText(&mut self, encoding: impl Into<Option<EncodingRef>>, text: impl AsRef<[u8]>) {
        let data = self.text_mut();
        data.encoding = encoding.into();
        data.text.clear();
        data.text.extend_from_slice(text.as_ref());
        data.utf8_text.take();
    }

    /// SQL mode 变更时清缓存。
    pub fn SetNoBackslashEscapes(&mut self, value: bool) {
        let data = self.text_mut();
        if data.no_backslash_escapes != value {
            data.no_backslash_escapes = value;
            data.utf8_text.take();
        }
    }

    /// 返回规范化后的 UTF-8 文本（惰性计算并缓存）。
    pub fn Text(&self) -> String {
        let Some(data) = &self.text else {
            return String::new();
        };
        let Some(encoding) = data.encoding else {
            return String::from_utf8_lossy(&data.text).into_owned();
        };
        data.utf8_text
            .get_or_init(|| {
                convertBinaryStringLiterals(&data.text, encoding, data.no_backslash_escapes)
            })
            .clone()
    }

    /// 返回未经规范化的原始字节切片。
    pub fn OriginalText(&self) -> &[u8] {
        self.text.as_ref().map_or(&[], |data| data.text.as_slice())
    }
}

/// 按编码将字节解码为 UTF-8 字符串；`replace` 为真时非法序列用替换输出。
fn decode(bytes: &[u8], encoding: EncodingRef, replace: bool) -> Result<String, ()> {
    let operation = if replace { OpDecodeReplace } else { OpDecode };
    let transformed = match encoding.Transform(&mut Vec::new(), bytes, operation) {
        Ok(output) => output,
        Err(error) if replace => error.output().to_vec(),
        Err(_) => return Err(()),
    };
    String::from_utf8(transformed).map_err(|_| ())
}

/// 判断字符串是否无可打印控制字符（用于决定是否转十六进制）。
fn is_printable(value: &str) -> bool {
    value.chars().all(|character| !character.is_control())
}

/// 标识符字符：下划线或 ASCII 字母数字（用于 `_binary'x'` 前是否插空格）。
fn is_ident_char(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}

/// 若十六进制字面量紧跟标识符字符，需插入空格避免词法粘连。
fn needs_space_before_hex_literal(utf8_text: &[u8], quote_start: usize) -> bool {
    quote_start > 0 && is_ident_char(utf8_text[quote_start - 1])
}

/// 在原始字节中前进到下一个匹配字节，返回其位置。
fn advance_orig_to(src: &[u8], index: &mut usize, byte: u8) -> Option<usize> {
    while *index < src.len() {
        let position = *index;
        *index += 1;
        if src[position] == byte {
            return Some(position);
        }
    }
    None
}

/// 跳过到行尾（`--`/`#` 注释），同步推进原始字节中的引号位置。
fn skip_to_eol(utf8_text: &[u8], src: &[u8], index: &mut usize, orig_index: &mut usize) {
    while *index < utf8_text.len() {
        let byte = utf8_text[*index];
        if byte == b'\'' || byte == b'"' {
            let _ = advance_orig_to(src, orig_index, byte);
        }
        *index += 1;
        if byte == b'\n' {
            return;
        }
    }
}

/// 跳过到块注释结束 `*/`。
fn skip_to_block_end(utf8_text: &[u8], src: &[u8], index: &mut usize, orig_index: &mut usize) {
    *index += 2;
    while *index < utf8_text.len() {
        let byte = utf8_text[*index];
        if byte == b'\'' || byte == b'"' {
            let _ = advance_orig_to(src, orig_index, byte);
        }
        if byte == b'*' && utf8_text.get(*index + 1) == Some(&b'/') {
            *index += 2;
            return;
        }
        *index += 1;
    }
}

/// `--` 注释要求第三字节为空白才生效（MySQL 规则）。
fn dash_comment_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// 若当前位置是普通注释则跳过并返回 true；`/*!` / `/*+` 可执行注释不跳过。
fn skip_comment(utf8_text: &[u8], src: &[u8], index: &mut usize, orig_index: &mut usize) -> bool {
    let position = *index;
    let byte = utf8_text[position];
    if byte == b'-' && utf8_text.get(position + 1) == Some(&b'-') {
        if utf8_text
            .get(position + 2)
            .is_none_or(|next| dash_comment_whitespace(*next))
        {
            skip_to_eol(utf8_text, src, index, orig_index);
            return true;
        }
    }
    if byte == b'#' {
        skip_to_eol(utf8_text, src, index, orig_index);
        return true;
    }
    if byte == b'/' && utf8_text.get(position + 1) == Some(&b'*') {
        if matches!(utf8_text.get(position + 2), Some(b'!') | Some(b'+')) {
            return false;
        }
        skip_to_block_end(utf8_text, src, index, orig_index);
        return true;
    }
    false
}

/// 将不可打印的引号字符串字面量替换为 `0x` 十六进制形式；可打印则保持原样。
pub fn convertBinaryStringLiterals(
    text: &[u8],
    encoding: EncodingRef,
    no_backslash_escapes: bool,
) -> String {
    // 先整体解码；若无可疑引号则无需扫描替换。
    let decoded = decode(text, encoding, true).unwrap_or_default();
    if !decoded
        .as_bytes()
        .iter()
        .any(|byte| matches!(byte, b'\'' | b'"'))
    {
        return decoded;
    }

    let utf8_text = decoded.as_bytes();
    let mut output: Option<Vec<u8>> = None;
    let mut last_copied = 0;
    let mut orig_index = 0;
    let mut index = 0;

    // 扫描 UTF-8 文本：跳过注释，定位引号字符串并在原始字节上取内容。
    while index < utf8_text.len() {
        if skip_comment(utf8_text, text, &mut index, &mut orig_index) {
            continue;
        }
        let quote = utf8_text[index];
        if quote != b'\'' && quote != b'"' {
            index += 1;
            continue;
        }

        let utf8_quote_start = index;
        index += 1;
        let Some(orig_quote_start) = advance_orig_to(text, &mut orig_index, quote) else {
            break;
        };
        let mut orig_quote_end = None;

        while index < utf8_text.len() {
            let byte = utf8_text[index];
            if byte == quote {
                index += 1;
                let Some(orig_close) = advance_orig_to(text, &mut orig_index, quote) else {
                    break;
                };
                if utf8_text.get(index) != Some(&quote) {
                    orig_quote_end = Some(orig_close + 1);
                    break;
                }
                index += 1;
                if advance_orig_to(text, &mut orig_index, quote).is_none() {
                    break;
                }
            } else if byte == b'\\' && !no_backslash_escapes && index + 1 < utf8_text.len() {
                let next = utf8_text[index + 1];
                index += 2;
                if (next == b'\'' || next == b'"')
                    && advance_orig_to(text, &mut orig_index, next).is_none()
                {
                    break;
                }
            } else {
                index += 1;
            }
        }

        let Some(orig_quote_end) = orig_quote_end else {
            continue;
        };
        // 内容可打印则保留原字面量；否则展开转义后写成 0xHH...。
        let content_bytes = &text[orig_quote_start + 1..orig_quote_end - 1];
        if decode(content_bytes, encoding, false).is_ok_and(|value| is_printable(&value)) {
            continue;
        }

        let mut content = Vec::with_capacity(content_bytes.len());
        let mut position = 0;
        while position < content_bytes.len() {
            let byte = content_bytes[position];
            if byte == quote {
                position += 1;
                if content_bytes.get(position) == Some(&quote) {
                    content.push(quote);
                    position += 1;
                }
            } else if byte == b'\\' && !no_backslash_escapes && position + 1 < content_bytes.len() {
                position += 1;
                content.extend(crate::util::UnescapeChar(content_bytes[position]));
                position += 1;
            } else {
                content.push(byte);
                position += 1;
            }
        }

        let buffer = output.get_or_insert_with(|| Vec::with_capacity(utf8_text.len()));
        buffer.extend_from_slice(&utf8_text[last_copied..utf8_quote_start]);
        if needs_space_before_hex_literal(utf8_text, utf8_quote_start) {
            buffer.push(b' ');
        }
        buffer.extend_from_slice(b"0x");
        for byte in content {
            buffer.push(HEX_DIGITS[(byte >> 4) as usize]);
            buffer.push(HEX_DIGITS[(byte & 0x0f) as usize]);
        }
        last_copied = index;
    }

    match output {
        None => decoded,
        Some(mut buffer) => {
            buffer.extend_from_slice(&utf8_text[last_copied..]);
            String::from_utf8(buffer).expect("output is assembled from UTF-8 and ASCII hex")
        }
    }
}

#[allow(non_camel_case_types)]
/// Go 风格小写类型别名：node。
pub type node = AstNode;

#[derive(Clone, Default)]
/// 语句节点嵌入基类。
pub struct StmtNodeBase {
    pub embedded_node: AstNode,
}

impl StmtNodeBase {
    pub fn statement(&self) {}
}

#[derive(Clone, Default)]
/// DDL 语句嵌入基类。
pub struct DdlNodeBase {
    pub embedded_stmt_node: StmtNodeBase,
}

impl DdlNodeBase {
    pub fn ddlStatement(&self) {}
}

#[derive(Clone, Default)]
/// DML 语句嵌入基类。
pub struct DmlNodeBase {
    pub embedded_stmt_node: StmtNodeBase,
}

impl DmlNodeBase {
    pub fn dmlStatement(&self) {}
}

#[derive(Clone, Default)]
/// 表达式节点基类：嵌入 AstNode，并持有 FieldType 与 Flag。
pub struct ExprNodeBase {
    pub embedded_node: AstNode,
    field_type: FieldType,
    flag: u64,
}

impl ExprNodeBase {
    /// 设置表达式结果类型。
    pub fn SetType(&mut self, field_type: FieldType) {
        self.field_type = field_type;
    }

    /// 获取表达式结果类型。
    pub fn GetType(&self) -> &FieldType {
        &self.field_type
    }

    /// 设置表达式标志位。
    pub fn SetFlag(&mut self, flag: u64) {
        self.flag = flag;
    }

    /// 获取表达式标志位。
    pub fn GetFlag(&self) -> u64 {
        self.flag
    }
}

/// 表达式节点类型别名（Go TexprNode）。
pub type TexprNode = ExprNodeBase;

#[derive(Clone, Default)]
/// 函数表达式嵌入基类。
pub struct FuncNodeBase {
    pub embedded_expr_node: ExprNodeBase,
}

impl FuncNodeBase {
    pub fn functionExpression(&self) {}
}

#[allow(non_camel_case_types)]
/// Go 风格别名：stmtNode。
pub type stmtNode = StmtNodeBase;
#[allow(non_camel_case_types)]
/// Go 风格别名：ddlNode。
pub type ddlNode = DdlNodeBase;
#[allow(non_camel_case_types)]
/// Go 风格别名：dmlNode。
pub type dmlNode = DmlNodeBase;
#[allow(non_camel_case_types)]
/// Go 风格别名：exprNode。
pub type exprNode = ExprNodeBase;
#[allow(non_camel_case_types)]
/// Go 风格别名：funcNode。
pub type funcNode = FuncNodeBase;
