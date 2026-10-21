// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use super::router::{NameRouteError, ObjectName, Rule};

const MAX_DATABASE_NAME_LENGTH: usize = 64;
const MAX_TABLE_NAME_LENGTH: usize = 64;

pub fn parse_rules(specs: &[String]) -> Result<Vec<Rule>, NameRouteError> {
    specs
        .iter()
        .enumerate()
        .map(|(index, spec)| {
            SpecParser::new(spec).parse_rule().map_err(|error| {
                NameRouteError::new(format!(
                    "invalid rename rule {} {:?}: {error}",
                    index + 1,
                    spec
                ))
            })
        })
        .collect()
}

struct SpecParser {
    input: Vec<char>,
    pos: usize,
}

impl SpecParser {
    fn new(input: &str) -> Self {
        Self {
            input: input.chars().collect(),
            pos: 0,
        }
    }

    fn parse_rule(mut self) -> Result<Rule, NameRouteError> {
        self.skip_spaces();
        let source = self
            .parse_object_name()
            .map_err(|error| error.context("invalid source"))?;
        self.skip_spaces();
        if !self.consume(':') {
            return Err(NameRouteError::new(format!(
                "expected ':' at character {}",
                self.pos
            )));
        }
        self.skip_spaces();
        let target = self
            .parse_object_name()
            .map_err(|error| error.context("invalid target"))?;
        self.skip_spaces();
        if let Some(ch) = self.input.get(self.pos) {
            return Err(NameRouteError::new(format!(
                "unexpected character {ch:?} at character {}",
                self.pos
            )));
        }
        if source.is_table() != target.is_table() {
            return Err(NameRouteError::new(
                "source and target must both name a schema or both name a table",
            ));
        }
        Ok(Rule { source, target })
    }

    fn parse_object_name(&mut self) -> Result<ObjectName, NameRouteError> {
        let schema = self.parse_identifier(MAX_DATABASE_NAME_LENGTH)?;
        self.skip_spaces();
        if !self.consume('.') {
            return Ok(ObjectName::schema(schema));
        }
        self.skip_spaces();
        let table = self.parse_identifier(MAX_TABLE_NAME_LENGTH)?;
        Ok(ObjectName::table(schema, table))
    }

    fn parse_identifier(&mut self, max_length: usize) -> Result<String, NameRouteError> {
        let Some(&first) = self.input.get(self.pos) else {
            return Err(NameRouteError::new(format!(
                "expected identifier at character {}",
                self.pos
            )));
        };
        if first == '`' {
            return self.parse_quoted_identifier(max_length);
        }

        let start = self.pos;
        while let Some(&ch) = self.input.get(self.pos) {
            if ch == '.' || ch == ':' || ch.is_whitespace() {
                break;
            }
            if !is_unquoted_identifier_char(ch) {
                return Err(NameRouteError::new(format!(
                    "character {ch:?} at character {} must be inside backticks",
                    self.pos
                )));
            }
            self.pos += 1;
        }
        if self.pos == start {
            return Err(NameRouteError::new(format!(
                "expected identifier at character {}",
                self.pos
            )));
        }
        let identifier: String = self.input[start..self.pos].iter().collect();
        validate_identifier(&identifier, max_length)?;
        Ok(identifier)
    }

    fn parse_quoted_identifier(&mut self, max_length: usize) -> Result<String, NameRouteError> {
        let start = self.pos;
        self.pos += 1;
        let mut result = String::new();
        while let Some(&ch) = self.input.get(self.pos) {
            if ch == '`' {
                if self.input.get(self.pos + 1) == Some(&'`') {
                    result.push('`');
                    self.pos += 2;
                    continue;
                }
                self.pos += 1;
                validate_identifier(&result, max_length)?;
                return Ok(result);
            }
            result.push(ch);
            self.pos += 1;
        }
        Err(NameRouteError::new(format!(
            "unterminated quoted identifier starting at character {start}"
        )))
    }

    fn skip_spaces(&mut self) {
        while self
            .input
            .get(self.pos)
            .is_some_and(|character| character.is_whitespace())
        {
            self.pos += 1;
        }
    }

    fn consume(&mut self, expected: char) -> bool {
        if self.input.get(self.pos) != Some(&expected) {
            return false;
        }
        self.pos += 1;
        true
    }
}

fn is_unquoted_identifier_char(character: char) -> bool {
    if character.is_ascii() {
        character.is_ascii_alphanumeric() || matches!(character, '_' | '$')
    } else {
        !character.is_whitespace() && !character.is_control()
    }
}

pub(crate) fn validate_identifier(
    identifier: &str,
    max_length: usize,
) -> Result<(), NameRouteError> {
    if identifier.is_empty() {
        return Err(NameRouteError::new("identifier must not be empty"));
    }
    if identifier.contains('\0') {
        return Err(NameRouteError::new("identifier must not contain NUL"));
    }
    if identifier
        .chars()
        .next_back()
        .is_some_and(char::is_whitespace)
    {
        return Err(NameRouteError::new(
            "identifier must not end with whitespace",
        ));
    }
    if identifier.chars().count() > max_length {
        return Err(NameRouteError::new(format!(
            "identifier {identifier:?} exceeds the maximum length of {max_length} characters"
        )));
    }
    Ok(())
}
