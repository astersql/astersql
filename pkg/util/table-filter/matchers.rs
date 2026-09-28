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

// 过滤规则匹配器：精确串、恒真、正则，以及表/列规则载体。
//
// `positive` 表示接受（true）或拒绝（false）；匹配按规则列表顺序取首条命中。

use std::error::Error;
use std::fmt::{self, Debug, Display};

/// Match Go's `strings.ToLower`, which applies a one-rune simple case mapping.
fn goToLower(value: &str) -> String {
    value
        .chars()
        .map(|ch| ch.to_lowercase().next().unwrap_or(ch))
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 过滤解析或匹配相关错误消息包装。
pub struct FilterError(pub String);

impl Display for FilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for FilterError {}

#[derive(Debug)]
/// 一条库表规则：schema/table 匹配器与正负向标志。
pub struct tableRule {
    pub schema: Box<dyn matcher>,
    pub table: Box<dyn matcher>,
    pub positive: bool,
}

#[derive(Debug)]
/// 一条列名规则：列匹配器与正负向标志。
pub struct columnRule {
    pub column: Box<dyn matcher>,
    pub positive: bool,
}

/// 名称匹配器接口：单串匹配、是否匹配任意串、转小写变体。
pub trait matcher: Debug {
    fn matchString(&self, name: &str) -> bool;
    fn matchAllStrings(&self) -> bool;
    fn toLower(&self) -> Box<dyn matcher>;
}

#[derive(Debug)]
/// 精确字符串相等匹配。
pub struct stringMatcher(pub String);

impl matcher for stringMatcher {
    fn matchString(&self, name: &str) -> bool {
        self.0 == name
    }

    fn matchAllStrings(&self) -> bool {
        false
    }

    fn toLower(&self) -> Box<dyn matcher> {
        Box::new(stringMatcher(goToLower(&self.0)))
    }
}

#[derive(Clone, Copy, Debug, Default)]
/// 恒匹配任意字符串的匹配器（通配 `*` 的优化形态）。
pub struct trueMatcher;

impl matcher for trueMatcher {
    fn matchString(&self, _name: &str) -> bool {
        true
    }

    fn matchAllStrings(&self) -> bool {
        true
    }

    fn toLower(&self) -> Box<dyn matcher> {
        Box::new(*self)
    }
}

#[derive(Debug)]
/// 基于 `regex` crate 的正则匹配器。
pub struct regexpMatcher {
    pattern: regex::Regex,
}

/// 编译正则；`(?)s^.*$` 直接退化为 `trueMatcher`。
pub fn newRegexpMatcher(pattern: &str) -> Result<Box<dyn matcher>, FilterError> {
    if pattern == "(?s)^.*$" {
        return Ok(Box::new(trueMatcher));
    }
    regex::Regex::new(pattern)
        .map(|pattern| Box::new(regexpMatcher { pattern }) as Box<dyn matcher>)
        .map_err(|error| regexp_error(pattern, error))
}

// Preserve Go's regexp/syntax diagnostic category and offending expression.
// Use structured parser spans, never infer positions from rendered diagnostics.
fn regexp_error(pattern: &str, error: regex::Error) -> FilterError {
    use regex_syntax::ast::ErrorKind;
    if let Err(parsed) = regex_syntax::ast::parse::Parser::new().parse(pattern) {
        let span = parsed.span();
        let fragment = &pattern[span.start.offset..span.end.offset];
        let tail = &pattern[span.start.offset..];
        let (kind, expression) = match parsed.kind() {
            ErrorKind::ClassUnclosed => ("missing closing ]", tail),
            ErrorKind::GroupUnclosed => ("missing closing )", pattern),
            ErrorKind::GroupUnopened => ("unexpected )", pattern),
            ErrorKind::UnsupportedLookAround if tail.starts_with("(?<") => {
                ("invalid named capture", tail)
            }
            ErrorKind::UnsupportedLookAround => ("invalid or unsupported Perl syntax", &tail[..3]),
            ErrorKind::ClassRangeInvalid => ("invalid character class range", fragment),
            ErrorKind::EscapeUnexpectedEof => ("trailing backslash at end of expression", ""),
            ErrorKind::EscapeUnrecognized
            | ErrorKind::ClassEscapeInvalid
            | ErrorKind::UnsupportedBackreference => ("invalid escape sequence", fragment),
            ErrorKind::RepetitionMissing => ("missing argument to repetition operator", &tail[..1]),
            ErrorKind::RepetitionCountInvalid => ("invalid repeat count", fragment),
            _ => return FilterError(format!("error parsing regexp: {error}")),
        };
        return FilterError(format!("error parsing regexp: {kind}: `{expression}`"));
    }
    FilterError(format!("error parsing regexp: {error}"))
}

impl matcher for regexpMatcher {
    fn matchString(&self, name: &str) -> bool {
        self.pattern.is_match(name)
    }

    fn matchAllStrings(&self) -> bool {
        false
    }

    fn toLower(&self) -> Box<dyn matcher> {
        Box::new(regexpMatcher {
            pattern: regex::Regex::new(&format!("(?i){}", self.pattern.as_str()))
                .expect("adding the case-insensitive flag must preserve a valid regexp"),
        })
    }
}
