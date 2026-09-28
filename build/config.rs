// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

use std::collections::HashMap;
use std::sync::LazyLock;

// configFile corresponds to Go's embedded nogo_config.json bytes.
static configFile: &[u8] = include_bytes!("nogo_config.json");

// NogoConfig is the nogo config file.
//
// Rust has no Go-style package init hook. LazyLock preserves one-time initialization,
// direct shared reads and the same panic-on-invalid-config contract without static mut.
pub static NogoConfig: LazyLock<NogoConfigFormat> = LazyLock::new(|| {
    parse_config(configFile).unwrap_or_else(|_| panic!("fail to parse nogo_config.json"))
});

// NogoConfigFormat is the format of the nogo config file.
pub type NogoConfigFormat = HashMap<String, AnalysisConfig>;

// AnalysisConfig represents the config of an analysis pass.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AnalysisConfig {
    pub ExcludeFiles: Option<HashMap<String, String>>,
    pub OnlyFiles: Option<HashMap<String, String>>,
}

// init eagerly forces the same one-time initialization when an explicit startup hook is used.
pub fn init() {
    LazyLock::force(&NogoConfig);
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ParseError {
    offset: usize,
    message: &'static str,
}

pub(crate) fn parse_config(input: &[u8]) -> Result<NogoConfigFormat, ParseError> {
    let mut parser = Parser { input, offset: 0 };
    let config = parser.parse_root()?;
    parser.skip_whitespace();
    if parser.offset != input.len() {
        return Err(parser.error("trailing data"));
    }
    Ok(config)
}

struct Parser<'a> {
    input: &'a [u8],
    offset: usize,
}

impl Parser<'_> {
    fn parse_root(&mut self) -> Result<NogoConfigFormat, ParseError> {
        self.skip_whitespace();
        if self.peek() == Some(b'n') {
            self.consume_literal(b"null")?;
            return Ok(HashMap::new());
        }

        self.expect(b'{')?;
        let mut config = HashMap::new();
        self.skip_whitespace();
        if self.consume(b'}') {
            return Ok(config);
        }

        loop {
            let analyzer = self.parse_string()?;
            self.skip_whitespace();
            self.expect(b':')?;
            let analysis_config = self.parse_analysis_config()?;
            config.insert(analyzer, analysis_config);
            self.skip_whitespace();
            if self.consume(b'}') {
                return Ok(config);
            }
            self.expect(b',')?;
        }
    }

    fn parse_analysis_config(&mut self) -> Result<AnalysisConfig, ParseError> {
        self.skip_whitespace();
        if self.peek() == Some(b'n') {
            self.consume_literal(b"null")?;
            return Ok(AnalysisConfig::default());
        }

        self.expect(b'{')?;
        let mut config = AnalysisConfig::default();
        self.skip_whitespace();
        if self.consume(b'}') {
            return Ok(config);
        }

        loop {
            let field = self.parse_string()?;
            self.skip_whitespace();
            self.expect(b':')?;
            if field.eq_ignore_ascii_case("exclude_files") {
                config.ExcludeFiles = self.parse_optional_string_map()?;
            } else if field.eq_ignore_ascii_case("only_files") {
                config.OnlyFiles = self.parse_optional_string_map()?;
            } else {
                self.skip_value()?;
            }
            self.skip_whitespace();
            if self.consume(b'}') {
                return Ok(config);
            }
            self.expect(b',')?;
        }
    }

    fn parse_optional_string_map(&mut self) -> Result<Option<HashMap<String, String>>, ParseError> {
        self.skip_whitespace();
        if self.peek() == Some(b'n') {
            self.consume_literal(b"null")?;
            return Ok(None);
        }

        self.expect(b'{')?;
        let mut values = HashMap::new();
        self.skip_whitespace();
        if self.consume(b'}') {
            return Ok(Some(values));
        }

        loop {
            let key = self.parse_string()?;
            self.skip_whitespace();
            self.expect(b':')?;
            let value = self.parse_string()?;
            values.insert(key, value);
            self.skip_whitespace();
            if self.consume(b'}') {
                return Ok(Some(values));
            }
            self.expect(b',')?;
        }
    }

    fn skip_value(&mut self) -> Result<(), ParseError> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'"') => {
                self.parse_string()?;
            }
            Some(b'{') => {
                self.offset += 1;
                self.skip_whitespace();
                if self.consume(b'}') {
                    return Ok(());
                }
                loop {
                    self.parse_string()?;
                    self.skip_whitespace();
                    self.expect(b':')?;
                    self.skip_value()?;
                    self.skip_whitespace();
                    if self.consume(b'}') {
                        break;
                    }
                    self.expect(b',')?;
                }
            }
            Some(b'[') => {
                self.offset += 1;
                self.skip_whitespace();
                if self.consume(b']') {
                    return Ok(());
                }
                loop {
                    self.skip_value()?;
                    self.skip_whitespace();
                    if self.consume(b']') {
                        break;
                    }
                    self.expect(b',')?;
                }
            }
            Some(b't') => self.consume_literal(b"true")?,
            Some(b'f') => self.consume_literal(b"false")?,
            Some(b'n') => self.consume_literal(b"null")?,
            Some(b'-' | b'0'..=b'9') => self.skip_number()?,
            _ => return Err(self.error("expected JSON value")),
        }
        Ok(())
    }

    fn parse_string(&mut self) -> Result<String, ParseError> {
        self.skip_whitespace();
        self.expect(b'"')?;
        let mut value = String::new();

        loop {
            let byte = self
                .peek()
                .ok_or_else(|| self.error("unterminated string"))?;
            match byte {
                b'"' => {
                    self.offset += 1;
                    return Ok(value);
                }
                b'\\' => {
                    self.offset += 1;
                    let escaped = self
                        .peek()
                        .ok_or_else(|| self.error("unterminated escape"))?;
                    self.offset += 1;
                    match escaped {
                        b'"' => value.push('"'),
                        b'\\' => value.push('\\'),
                        b'/' => value.push('/'),
                        b'b' => value.push('\u{0008}'),
                        b'f' => value.push('\u{000c}'),
                        b'n' => value.push('\n'),
                        b'r' => value.push('\r'),
                        b't' => value.push('\t'),
                        b'u' => value.push(self.parse_unicode_escape()?),
                        _ => return Err(self.error("invalid escape")),
                    }
                }
                0x00..=0x1f => return Err(self.error("control character in string")),
                0x20..=0x7f => {
                    value.push(char::from(byte));
                    self.offset += 1;
                }
                _ => {
                    let remainder = std::str::from_utf8(&self.input[self.offset..])
                        .map_err(|_| self.error("invalid UTF-8"))?;
                    let character = remainder
                        .chars()
                        .next()
                        .ok_or_else(|| self.error("invalid UTF-8"))?;
                    value.push(character);
                    self.offset += character.len_utf8();
                }
            }
        }
    }

    fn parse_unicode_escape(&mut self) -> Result<char, ParseError> {
        let first = self.parse_hex_u16()?;
        if (0xd800..=0xdbff).contains(&first) {
            if !self.consume(b'\\') || !self.consume(b'u') {
                return Err(self.error("missing low surrogate"));
            }
            let second = self.parse_hex_u16()?;
            if !(0xdc00..=0xdfff).contains(&second) {
                return Err(self.error("invalid low surrogate"));
            }
            let scalar =
                0x10000 + ((u32::from(first) - 0xd800) << 10) + (u32::from(second) - 0xdc00);
            return char::from_u32(scalar).ok_or_else(|| self.error("invalid Unicode scalar"));
        }
        if (0xdc00..=0xdfff).contains(&first) {
            return Err(self.error("unexpected low surrogate"));
        }
        char::from_u32(u32::from(first)).ok_or_else(|| self.error("invalid Unicode scalar"))
    }

    fn parse_hex_u16(&mut self) -> Result<u16, ParseError> {
        let mut value = 0_u16;
        for _ in 0..4 {
            let byte = self
                .peek()
                .ok_or_else(|| self.error("incomplete Unicode escape"))?;
            self.offset += 1;
            let digit = match byte {
                b'0'..=b'9' => u16::from(byte - b'0'),
                b'a'..=b'f' => u16::from(byte - b'a' + 10),
                b'A'..=b'F' => u16::from(byte - b'A' + 10),
                _ => return Err(self.error("invalid Unicode escape")),
            };
            value = value * 16 + digit;
        }
        Ok(value)
    }

    fn skip_number(&mut self) -> Result<(), ParseError> {
        self.consume(b'-');
        match self.peek() {
            Some(b'0') => self.offset += 1,
            Some(b'1'..=b'9') => {
                self.offset += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.offset += 1;
                }
            }
            _ => return Err(self.error("invalid number")),
        }

        if self.consume(b'.') {
            self.consume_digits()?;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.offset += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.offset += 1;
            }
            self.consume_digits()?;
        }
        Ok(())
    }

    fn consume_digits(&mut self) -> Result<(), ParseError> {
        let start = self.offset;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.offset += 1;
        }
        if self.offset == start {
            return Err(self.error("expected digit"));
        }
        Ok(())
    }

    fn consume_literal(&mut self, literal: &[u8]) -> Result<(), ParseError> {
        if self.input.get(self.offset..self.offset + literal.len()) == Some(literal) {
            self.offset += literal.len();
            Ok(())
        } else {
            Err(self.error("invalid literal"))
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), ParseError> {
        self.skip_whitespace();
        if self.consume(expected) {
            Ok(())
        } else {
            Err(self.error("unexpected token"))
        }
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.offset += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.offset).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.offset += 1;
        }
    }

    fn error(&self, message: &'static str) -> ParseError {
        ParseError {
            offset: self.offset,
            message,
        }
    }
}
