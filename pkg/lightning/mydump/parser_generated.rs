// Code generated from parser.rl; keep semantic changes in that Ragel source.
//
// Copyright 2019 PingCAP, Inc.
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

// 由 parser.rl（Ragel）语义迁移而来的 INSERT 词法扫描器。
//
// `lex` 驱动显式游标状态机：跳过注释/分隔符，识别括号行边界、关键字、
// 整数、十六/二进制字面量与三类引号字符串。跨块时返回 NeedMore 并读下一块。
// 语义变更应先改 Ragel 源再同步本文件。

use crate::{ChunkParser, Error, EscapeFlavor, Token};

// 本文件承载 INSERT 数据文件的词法扫描，并以显式游标状态实现 Ragel 生成状态机的规则。
// ChunkParser 调用 lex 时通过 reader 补充缓冲，跨块 token 会保留到下一次扫描。
// 规则次序保持 parser.rl：注释与分隔符、括号、关键字、整数、十六/二进制、三类引号、普通文本。

const CHUNK_PARSER_START: i32 = 21;
const CHUNK_PARSER_FIRST_FINAL: i32 = 21;
const CHUNK_PARSER_ERROR: i32 = 0;
const CHUNK_PARSER_EN_MAIN: i32 = 21;

// ScanResult 对应生成代码中的 cs/ts/te/act 组合结果。
// NeedMore 表示 token 横跨块边界；Error 表示 Ragel 会进入 chunk_parser_error 的输入。
enum ScanResult {
    Token(Token, usize),
    Skip(usize),
    NeedMore,
    Eof,
    Error,
}

/// 对应 Go (*ChunkParser).lex：跳过注释后取出下一 token 并推进缓冲与逻辑位置。
// lex 对应 Go (*ChunkParser).lex。
// 它反复跳过 comment 规则，发现 token 后消费 buf[0..end] 并同步逻辑位置。
pub fn lex(parser: &mut ChunkParser) -> Result<(Token, Vec<u8>), Error> {
    // 常量在可读扫描器中不再直接驱动跳转，但保留生成文件的状态编号声明。
    let _initial_state = (
        CHUNK_PARSER_START,
        CHUNK_PARSER_FIRST_FINAL,
        CHUNK_PARSER_EN_MAIN,
    );
    loop {
        let scan = scan_one(
            &parser.block_parser.buf,
            parser.esc_flavor,
            parser.block_parser.is_last_chunk,
        );
        match scan {
            ScanResult::Token(token, end) => {
                let result = parser.block_parser.buf[..end].to_vec();
                parser.block_parser.buf.drain(..end);
                parser.block_parser.pos += end as i64;
                return Ok((token, result));
            }
            ScanResult::Skip(end) => {
                parser.block_parser.buf.drain(..end);
                parser.block_parser.pos += end as i64;
            }
            ScanResult::Eof => return Err(Error::Eof),
            ScanResult::Error => {
                debug_assert_eq!(CHUNK_PARSER_ERROR, 0);
                parser.block_parser.log_syntax_error();
                return Err(Error::Syntax("syntax error".into()));
            }
            ScanResult::NeedMore => {
                if parser.block_parser.is_last_chunk {
                    return Err(Error::Syntax("unexpected EOF".into()));
                }
                // 与 Go 生成代码相同：保留尚未定型的 token，再从 reader 追加下一块。
                parser.block_parser.read_block()?;
            }
        }
    }
}

// scan_one 从缓冲开头执行一次 Ragel main 规则；最长匹配和规则优先级在各辅助函数中显式体现。
fn scan_one(data: &[u8], flavor: EscapeFlavor, is_last: bool) -> ScanResult {
    if data.is_empty() {
        return if is_last {
            ScanResult::Eof
        } else {
            ScanResult::NeedMore
        };
    }

    // comment 规则：空白、逗号、分号都不产生 token。
    if data[0].is_ascii_whitespace() || matches!(data[0], b',' | b';') {
        return ScanResult::Skip(1);
    }
    if data.starts_with(b"/*") {
        return match find_block_comment_end(data) {
            Some(end) => ScanResult::Skip(end),
            None if is_last => ScanResult::Error,
            None => ScanResult::NeedMore,
        };
    }
    if data.starts_with(b"--") {
        let end = data
            .iter()
            .position(|b| matches!(b, b'\r' | b'\n'))
            .unwrap_or(data.len());
        if end == data.len() && !is_last {
            return ScanResult::NeedMore;
        }
        return ScanResult::Skip(end);
    }

    // mydumper JSON 输出可能包含 CONVERT(... USING UTF8MB4)，原词法器把这些固定片段当注释剥离。
    for ignored in [b"convert(".as_slice(), b"using utf8mb4)".as_slice()] {
        match ascii_prefix(data, ignored) {
            Prefix::Full => return ScanResult::Skip(ignored.len()),
            Prefix::Partial if !is_last => return ScanResult::NeedMore,
            _ => {}
        }
    }

    match data[0] {
        b'(' => return ScanResult::Token(Token::RowBegin, 1),
        b')' => return ScanResult::Token(Token::RowEnd, 1),
        b'\'' => {
            return scan_quoted(
                data,
                b'\'',
                flavor != EscapeFlavor::None,
                Token::SingleQuoted,
                is_last,
            );
        }
        b'"' => {
            return scan_quoted(
                data,
                b'"',
                flavor != EscapeFlavor::None,
                Token::DoubleQuoted,
                is_last,
            );
        }
        b'`' => return scan_quoted(data, b'`', false, Token::BackQuoted, is_last),
        _ => {}
    }

    // 关键字匹配大小写不敏感。Partial 必须等下一块，避免把 "val" 过早返回为 unquoted。
    for (keyword, token) in [
        (b"values".as_slice(), Token::Values),
        (b"null".as_slice(), Token::Null),
        (b"true".as_slice(), Token::True),
        (b"false".as_slice(), Token::False),
    ] {
        match ascii_prefix(data, keyword) {
            Prefix::Full => return prefer_over_unquoted(data, keyword.len(), token, is_last),
            Prefix::Partial if !is_last => return ScanResult::NeedMore,
            _ => {}
        }
    }

    // hex/bin 带引号形式允许空内容，0x/0b 形式至少需要一个合法数字。
    if matches!(data[0], b'x' | b'X') && data.get(1) == Some(&b'\'') {
        return scan_based_quoted(data, 16, Token::HexString, is_last);
    }
    if matches!(data[0], b'b' | b'B') && data.get(1) == Some(&b'\'') {
        return scan_based_quoted(data, 2, Token::BinString, is_last);
    }
    if data.starts_with(b"0x") || data.starts_with(b"0X") {
        return scan_based_digits(data, 2, 16, Token::HexString, is_last);
    }
    if data.starts_with(b"0b") || data.starts_with(b"0B") {
        return scan_based_digits(data, 2, 2, Token::BinString, is_last);
    }

    if data[0] == b'-' || data[0].is_ascii_digit() {
        if let Some(result) = scan_integer(data, is_last) {
            return result;
        }
    }

    scan_unquoted(data, is_last)
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Prefix {
    Full,
    Partial,
    None,
}

// ascii_prefix 对应 Ragel 的 'keyword'i，区分块尾的真前缀和确定不匹配。
fn ascii_prefix(data: &[u8], expected: &[u8]) -> Prefix {
    let common = data.len().min(expected.len());
    if !data[..common].eq_ignore_ascii_case(&expected[..common]) {
        Prefix::None
    } else if data.len() < expected.len() {
        Prefix::Partial
    } else {
        Prefix::Full
    }
}

// find_block_comment_end 实现 Ragel 的 '/*' any* :>> '*/'，返回闭合符之后的位置。
fn find_block_comment_end(data: &[u8]) -> Option<usize> {
    data.windows(2)
        .enumerate()
        .skip(1)
        .find_map(|(i, pair)| (pair == b"*/").then_some(i + 2))
}

// scan_quoted 同时覆盖 single_quoted、double_quoted 与 back_quoted。
// 连续两个定界符是转义；单/双引号还根据 SQL mode 接受反斜杠加任意字节。
fn scan_quoted(
    data: &[u8],
    quote: u8,
    backslash_escape: bool,
    token: Token,
    is_last: bool,
) -> ScanResult {
    let mut i = 1;
    while i < data.len() {
        if backslash_escape && data[i] == b'\\' {
            if i + 1 >= data.len() {
                return if is_last {
                    ScanResult::Error
                } else {
                    ScanResult::NeedMore
                };
            }
            i += 2;
            continue;
        }
        if data[i] == quote {
            if data.get(i + 1) == Some(&quote) {
                i += 2;
                continue;
            }
            return ScanResult::Token(token, i + 1);
        }
        i += 1;
    }
    if is_last {
        ScanResult::Error
    } else {
        ScanResult::NeedMore
    }
}

// scan_based_quoted 迁移 x'AB' 与 b'01'；遇到非法数字时按 Ragel 回退为 unquoted。
fn scan_based_quoted(data: &[u8], radix: u32, token: Token, is_last: bool) -> ScanResult {
    let mut i = 2;
    while i < data.len() && data[i] != b'\'' {
        if !is_radix_digit(data[i], radix) {
            return ScanResult::Error;
        }
        i += 1;
    }
    if i < data.len() {
        ScanResult::Token(token, i + 1)
    } else if is_last {
        ScanResult::Error
    } else {
        ScanResult::NeedMore
    }
}

// scan_based_digits 迁移 0xAB 与 0b01；没有数字时不会认作进制字面量。
fn scan_based_digits(
    data: &[u8],
    start: usize,
    radix: u32,
    token: Token,
    is_last: bool,
) -> ScanResult {
    let mut i = start;
    while i < data.len() && is_radix_digit(data[i], radix) {
        i += 1;
    }
    if i == start {
        return scan_unquoted(data, is_last);
    }
    if i == data.len() && !is_last {
        ScanResult::NeedMore
    } else {
        prefer_over_unquoted(data, i, token, is_last)
    }
}

fn is_radix_digit(byte: u8, radix: u32) -> bool {
    match radix {
        2 => matches!(byte, b'0' | b'1'),
        16 => byte.is_ascii_hexdigit(),
        _ => false,
    }
}

// scan_integer 保留 '-'? [0-9]+；只有减号不是整数，会落回 unquoted/行注释规则。
fn scan_integer(data: &[u8], is_last: bool) -> Option<ScanResult> {
    let mut i = usize::from(data[0] == b'-');
    let first_digit = i;
    while i < data.len() && data[i].is_ascii_digit() {
        i += 1;
    }
    if i == first_digit {
        return None;
    }
    Some(if i == data.len() && !is_last {
        ScanResult::NeedMore
    } else {
        prefer_over_unquoted(data, i, Token::Integer, is_last)
    })
}

// scan_unquoted 对应 ^([,;()'"`/*] | space)+，一直扫描到 SQL 分隔符或注释起始字符。
fn scan_unquoted(data: &[u8], is_last: bool) -> ScanResult {
    let mut i = 0;
    while i < data.len() && !is_unquoted_delimiter(data[i]) {
        i += 1;
    }
    if i == 0 {
        // 单独的 '/'、'*' 等不满足任何规则时，生成状态机进入 error state。
        return ScanResult::Error;
    }
    if i == data.len() && !is_last {
        ScanResult::NeedMore
    } else {
        ScanResult::Token(Token::Unquoted, i)
    }
}

// Ragel 的 `|* ... *|` 扫描采用最长匹配；只有长度相同时才由前面的专用规则获胜。
// 因而 `values` 是关键字，但 `valuesx`、`123abc`、`0x12g` 都是完整的 unquoted token。
fn prefer_over_unquoted(
    data: &[u8],
    matched_end: usize,
    token: Token,
    is_last: bool,
) -> ScanResult {
    let unquoted_end = data
        .iter()
        .position(|&byte| is_unquoted_delimiter(byte))
        .unwrap_or(data.len());
    if unquoted_end > matched_end {
        return if unquoted_end == data.len() && !is_last {
            ScanResult::NeedMore
        } else {
            ScanResult::Token(Token::Unquoted, unquoted_end)
        };
    }
    if matched_end == data.len() && !is_last {
        ScanResult::NeedMore
    } else {
        ScanResult::Token(token, matched_end)
    }
}

fn is_unquoted_delimiter(byte: u8) -> bool {
    byte.is_ascii_whitespace()
        || matches!(
            byte,
            b',' | b';' | b'(' | b')' | b'\'' | b'"' | b'`' | b'/' | b'*'
        )
}
