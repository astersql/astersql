// Copyright 2026 AsterSQL.
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

// 轻量字节匹配器与 Parser 对象池。
//
// 对应 Go `pkg/util/parser`：按谓词消费输入前缀（空格/数字/标点等），
// 以及线程局部池上的 GetParser/DestroyParser（复位后复用，避免跨线程移动非 Send AST）。

use std::cell::RefCell;

pub use crate::parser_core::Parser;

/// 轻量匹配解析错误：模式不匹配或数字转换失败。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseError {
    /// 输入不满足最小匹配次数。
    PatternNotMatch,
    /// 数字前缀无法解析为 `isize`。
    InvalidNumber(String),
}

/// 与 Go `ErrPatternNotMatch` 同名的模式不匹配错误常量。
#[allow(non_upper_case_globals)]
pub const ErrPatternNotMatch: ParseError = ParseError::PatternNotMatch;

thread_local! {
    // The migrated Parser contains non-Send AST trait objects. A thread-local
    // pool preserves Go's reset-and-reuse behavior without unsafely asserting
    // that parser state can move between threads.
    // 迁移后的 Parser 含非 Send AST；线程局部池保留 Go 的 reset-and-reuse，避免跨线程移动。
    static POOL: RefCell<Vec<Box<Parser>>> = const { RefCell::new(Vec::new()) };
}

/// Gets a reset parser from the pool, or constructs one when the pool is empty.
/// 从池中取出已复位的 Parser；池空时新建。
pub fn GetParser() -> Box<Parser> {
    POOL.with(|pool| pool.borrow_mut().pop())
        .unwrap_or_else(crate::parser_core::New)
}

/// Resets a parser before making it available for reuse.
/// 复位后放回池中供复用。
pub fn DestroyParser(mut parser: Box<Parser>) {
    parser.Reset();
    POOL.with(|pool| pool.borrow_mut().push(parser));
}

/// Matches Go string bytes and preserves `(match, rest, err)` on every branch.
#[allow(non_snake_case)]
pub fn Match<B, F>(buf: B, pat: F, times: isize) -> (Vec<u8>, Vec<u8>, Option<ParseError>)
where
    B: AsRef<[u8]>,
    F: Fn(u8) -> bool,
{
    let buf = buf.as_ref();
    let count = buf.iter().take_while(|byte| pat(**byte)).count();
    if (count as isize) < times {
        return (Vec::new(), buf.to_vec(), Some(ErrPatternNotMatch));
    }
    (buf[..count].to_vec(), buf[count..].to_vec(), None)
}

/// Matches exactly one Go string byte and preserves the original rest on error.
#[allow(non_snake_case)]
pub fn MatchOne<B, F>(buf: B, pat: F) -> (Vec<u8>, Option<ParseError>)
where
    B: AsRef<[u8]>,
    F: Fn(u8) -> bool,
{
    let buf = buf.as_ref();
    let Some(first) = buf.first().copied() else {
        return (buf.to_vec(), Some(ErrPatternNotMatch));
    };
    if !pat(first) {
        return (buf.to_vec(), Some(ErrPatternNotMatch));
    }
    (buf[1..].to_vec(), None)
}

fn is_go_byte_punctuation(byte: u8) -> bool {
    byte.is_ascii_punctuation() || matches!(byte, 0xA1 | 0xA7 | 0xAB | 0xB6 | 0xB7 | 0xBB | 0xBF)
}

/// Matches an arbitrary punctuation rune obtained from a single Go byte.
#[allow(non_snake_case)]
pub fn AnyPunct<B: AsRef<[u8]>>(buf: B) -> (Vec<u8>, Option<ParseError>) {
    MatchOne(buf, is_go_byte_punctuation)
}

/// Consumes one byte even when that byte splits a UTF-8 code point.
#[allow(non_snake_case)]
pub fn AnyChar<B: AsRef<[u8]>>(buf: B) -> (Vec<u8>, Option<ParseError>) {
    MatchOne(buf, |_| true)
}

/// Matches a single byte equal to `expected`.
#[allow(non_snake_case)]
pub fn Char<B: AsRef<[u8]>>(buf: B, expected: u8) -> (Vec<u8>, Option<ParseError>) {
    MatchOne(buf, |actual| actual == expected)
}

/// Matches at least `times` whitespace bytes and returns `(rest, err)`.
#[allow(non_snake_case)]
pub fn Space<B: AsRef<[u8]>>(buf: B, times: isize) -> (Vec<u8>, Option<ParseError>) {
    let (_, rest, error) = Match(buf, |byte| char::from(byte).is_whitespace(), times);
    (rest, error)
}

/// 消费零个或多个空白（`times=0`，不会失败）。
#[allow(non_snake_case)]
pub fn Space0<B: AsRef<[u8]>>(buf: B) -> Vec<u8> {
    let (rest, error) = Space(buf, 0);
    debug_assert!(error.is_none());
    rest
}

/// Matches at least `times` ASCII digit bytes and returns Go's three values.
#[allow(non_snake_case)]
pub fn Digit<B: AsRef<[u8]>>(buf: B, times: isize) -> (Vec<u8>, Vec<u8>, Option<ParseError>) {
    Match(buf, |byte| byte.is_ascii_digit(), times)
}

/// Parses a decimal prefix like Go `strconv.Atoi`, retaining rest on all errors.
#[allow(non_snake_case)]
pub fn Number<B: AsRef<[u8]>>(input: B) -> (isize, Vec<u8>, Option<ParseError>) {
    let input = input.as_ref();
    let (digits, rest, error) = Digit(input, 1);
    if let Some(error) = error {
        return (0, input.to_vec(), Some(error));
    }
    let digits = std::str::from_utf8(&digits).expect("Digit only returns ASCII");
    match digits.parse::<isize>() {
        Ok(number) => (number, rest, None),
        Err(error) => (
            isize::MAX,
            rest,
            Some(ParseError::InvalidNumber(error.to_string())),
        ),
    }
}
