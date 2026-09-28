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

// 过滤规则行解析：schema.table / 列模式、`!` 否定、`@file` 导入。
//
// 支持 `/regex/`、引号标识符、以及 `*?[]` 通配并转为正则。

use super::{FilterError, columnRule, matcher, newRegexpMatcher, stringMatcher, tableRule};
use std::fs::File;
use std::io::{BufRead, BufReader};

#[derive(Debug)]
/// 表规则解析器：累积 `tableRule`，并跟踪源文件位置。
pub struct tableRulesParser {
    pub rules: Vec<tableRule>,
    pub matcher_parser: matcherParser,
}

impl tableRulesParser {
    /// 解析一行表规则；`can_import=false` 时禁止嵌套 `@file`。
    pub fn parse(&mut self, line: &str, can_import: bool) -> Result<(), FilterError> {
        let mut line = line.trim_matches([' ', '\t']);
        if line.is_empty() || line.starts_with('#') {
            return Ok(());
        }
        // `!` 否定；`@path` 导入文件；其余为 schema.table 模式。
        let mut positive = true;
        if let Some(rest) = line.strip_prefix('!') {
            positive = false;
            line = rest;
        } else if let Some(file_name) = line.strip_prefix('@') {
            if !can_import {
                return Err(self
                    .matcher_parser
                    .errorf("importing filter files recursively is not allowed"));
            }
            return self.import_file(file_name);
        }

        let (schema, rest) = self.matcher_parser.parsePattern(line, true)?;
        if rest.is_empty() {
            return Err(self.matcher_parser.errorf("wrong table pattern"));
        }
        let Some(rest) = rest.strip_prefix('.') else {
            return Err(self
                .matcher_parser
                .errorf("syntax error: missing '.' between schema and table patterns"));
        };
        let (table, rest) = self.matcher_parser.parsePattern(rest, true)?;
        if !rest.is_empty() {
            return Err(self
                .matcher_parser
                .errorf("syntax error: stray characters after table pattern"));
        }
        self.rules.push(tableRule {
            schema,
            table,
            positive,
        });
        Ok(())
    }

    /// 打开外部规则文件并逐行解析（禁止再导入）。
    fn import_file(&mut self, file_name: &str) -> Result<(), FilterError> {
        let file = File::open(file_name).map_err(|error| {
            self.matcher_parser.annotatef(
                format!("open {file_name}: {error}"),
                "cannot open filter file",
            )
        })?;
        let old_file_name = std::mem::replace(&mut self.matcher_parser.fileName, file_name.into());
        let old_line_num = std::mem::replace(&mut self.matcher_parser.lineNum, 1);
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|error| {
                self.matcher_parser
                    .annotatef(error, "cannot read filter file")
            })?;
            self.parse(&line, false)?;
            self.matcher_parser.lineNum += 1;
        }
        self.matcher_parser.fileName = old_file_name;
        self.matcher_parser.lineNum = old_line_num;
        Ok(())
    }
}

#[derive(Debug)]
/// 列规则解析器：模式在入库前转为小写以统一大小写。
pub struct columnRulesParser {
    pub rules: Vec<columnRule>,
    pub matcher_parser: matcherParser,
}

impl columnRulesParser {
    /// 解析一行列规则；列名匹配器会 `toLower()`。
    pub fn parse(&mut self, line: &str, can_import: bool) -> Result<(), FilterError> {
        let mut line = line.trim_matches([' ', '\t']);
        if line.is_empty() || line.starts_with('#') {
            return Ok(());
        }
        let mut positive = true;
        if let Some(rest) = line.strip_prefix('!') {
            positive = false;
            line = rest;
        } else if let Some(file_name) = line.strip_prefix('@') {
            if !can_import {
                return Err(self
                    .matcher_parser
                    .errorf("importing filter files recursively is not allowed"));
            }
            return self.import_file(file_name);
        }

        let (column, rest) = self.matcher_parser.parsePattern(line, false)?;
        if !rest.is_empty() {
            return Err(self
                .matcher_parser
                .errorf("syntax error: stray characters after column pattern"));
        }
        self.rules.push(columnRule {
            column: column.toLower(),
            positive,
        });
        Ok(())
    }

    /// 打开外部列规则文件并逐行解析（禁止再导入）。
    fn import_file(&mut self, file_name: &str) -> Result<(), FilterError> {
        let file = File::open(file_name).map_err(|error| {
            self.matcher_parser.annotatef(
                format!("open {file_name}: {error}"),
                "cannot open filter file",
            )
        })?;
        let old_file_name = std::mem::replace(&mut self.matcher_parser.fileName, file_name.into());
        let old_line_num = std::mem::replace(&mut self.matcher_parser.lineNum, 1);
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|error| {
                self.matcher_parser
                    .annotatef(error, "cannot read filter file")
            })?;
            self.parse(&line, false)?;
            self.matcher_parser.lineNum += 1;
        }
        self.matcher_parser.fileName = old_file_name;
        self.matcher_parser.lineNum = old_line_num;
        Ok(())
    }
}

#[derive(Debug)]
/// 模式解析上下文：记录文件名与行号以便错误定位。
pub struct matcherParser {
    pub fileName: String,
    pub lineNum: i64,
}

impl matcherParser {
    /// 附加 `at file:line:` 前缀。
    fn wrap_error(&self, message: &str) -> String {
        format!("at {}:{}: {message}", self.fileName, self.lineNum)
    }

    /// 构造带源位置的 FilterError。
    pub fn errorf(&self, message: impl AsRef<str>) -> FilterError {
        FilterError(self.wrap_error(message.as_ref()))
    }

    /// 在错误消息上追加底层 cause。
    pub fn annotatef(&self, error: impl std::fmt::Display, message: &str) -> FilterError {
        self.errorf(format!("{message}: {error}"))
    }

    /// 编译正则并在失败时标注位置。
    fn regexp_matcher(&self, pattern: &str) -> Result<Box<dyn matcher>, FilterError> {
        newRegexpMatcher(pattern).map_err(|error| self.annotatef(error, "invalid pattern"))
    }

    /// 解析单个模式：`/re/`、双引号/` 标识符，或通配字面量。
    pub fn parsePattern<'a>(
        &self,
        line: &'a str,
        needs_dot_separator: bool,
    ) -> Result<(Box<dyn matcher>, &'a str), FilterError> {
        if line.is_empty() {
            return Err(self.errorf("syntax error: missing pattern"));
        }
        match line.as_bytes()[0] {
            b'/' => {
                let end = find_quoted_end(line.as_bytes(), b'/', false)
                    .ok_or_else(|| self.errorf("syntax error: incomplete regexp"))?;
                Ok((self.regexp_matcher(&line[1..end - 1])?, &line[end..]))
            }
            b'"' => {
                let end = find_quoted_end(line.as_bytes(), b'"', true)
                    .ok_or_else(|| self.errorf("syntax error: incomplete quoted identifier"))?;
                Ok((
                    Box::new(stringMatcher(line[1..end - 1].replace("\"\"", "\""))),
                    &line[end..],
                ))
            }
            b'`' => {
                let end = find_quoted_end(line.as_bytes(), b'`', true)
                    .ok_or_else(|| self.errorf("syntax error: incomplete quoted identifier"))?;
                Ok((
                    Box::new(stringMatcher(line[1..end - 1].replace("``", "`"))),
                    &line[end..],
                ))
            }
            _ => self.parseWildcardPattern(line, needs_dot_separator),
        }
    }

    /// 将 `*?[]` 与转义序列编译为字面量匹配器或锚定正则。
    pub fn parseWildcardPattern<'a>(
        &self,
        line: &'a str,
        needs_dot_separator: bool,
    ) -> Result<(Box<dyn matcher>, &'a str), FilterError> {
        let bytes = line.as_bytes();
        let mut literal = Vec::with_capacity(bytes.len());
        let mut wildcard = b"(?s)^".to_vec();
        let mut is_literal = true;
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => {
                    if i + 1 == bytes.len() {
                        return Err(self.errorf(r"syntax error: cannot place \ at end of line"));
                    }
                    let escaped = bytes[i + 1];
                    if escaped.is_ascii_alphanumeric() {
                        return Err(self.errorf(format!(
                            r"cannot escape a letter or number (\{}), it is reserved for future extension",
                            escaped as char
                        )));
                    }
                    if is_literal {
                        literal.push(escaped);
                    }
                    if escaped.is_ascii() {
                        wildcard.push(b'\\');
                    }
                    wildcard.push(escaped);
                    i += 2;
                }
                b'.' if needs_dot_separator => break,
                b'.' => return Err(self.errorf("unexpected special character '.'")),
                b'*' => {
                    is_literal = false;
                    wildcard.extend_from_slice(b".*");
                    i += 1;
                }
                b'?' => {
                    is_literal = false;
                    wildcard.push(b'.');
                    i += 1;
                }
                b'[' => {
                    is_literal = false;
                    let end = character_class_end(&bytes[i..]).ok_or_else(|| {
                        self.errorf("syntax error: failed to parse character class")
                    })? + i;
                    match bytes.get(i + 1) {
                        Some(b'!') => {
                            wildcard.extend_from_slice(b"[^");
                            wildcard.extend_from_slice(&bytes[i + 2..=end]);
                        }
                        Some(b'^') => {
                            wildcard.extend_from_slice(br"[\^");
                            wildcard.extend_from_slice(&bytes[i + 2..=end]);
                        }
                        _ => wildcard.extend_from_slice(&bytes[i..=end]),
                    }
                    i = end + 1;
                }
                byte => {
                    if !(byte == b'$'
                        || byte == b'_'
                        || byte.is_ascii_alphanumeric()
                        || !byte.is_ascii())
                    {
                        return Err(
                            self.errorf(format!("unexpected special character '{}'", byte as char))
                        );
                    }
                    literal.push(byte);
                    wildcard.push(byte);
                    i += 1;
                }
            }
        }
        if is_literal {
            let value = String::from_utf8(literal).expect("input was valid UTF-8");
            return Ok((Box::new(stringMatcher(value)), &line[i..]));
        }
        wildcard.push(b'$');
        let pattern = String::from_utf8(wildcard).expect("input was valid UTF-8");
        Ok((self.regexp_matcher(&pattern)?, &line[i..]))
    }
}

/// 查找引号/斜杠界定串的结束下标（不含起始符）；支持 `\` 或加倍转义。
fn find_quoted_end(bytes: &[u8], delimiter: u8, doubled_escape: bool) -> Option<usize> {
    let mut i = 1;
    while i < bytes.len() {
        if !doubled_escape && bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == delimiter {
            if doubled_escape && bytes.get(i + 1) == Some(&delimiter) {
                i += 2;
                continue;
            }
            // Go's delimiter regexps use `+`, so `//`, `""`, and ```` are
            // incomplete rather than valid empty patterns.
            if i == 1 {
                return None;
            }
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

/// 解析字符类 `[...]` 的结束位置；非法转义字母数字则失败。
fn character_class_end(bytes: &[u8]) -> Option<usize> {
    let mut i = 1;
    while i < bytes.len() {
        if bytes[i] == b']' {
            return (i > 1).then_some(i);
        }
        if bytes[i] == b'\\' {
            let escaped = *bytes.get(i + 1)?;
            if escaped.is_ascii_alphanumeric() {
                return None;
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    None
}
