// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// Plan-cache SQL parameterization.
//
// TiDB keeps constants in SELECT fields, GROUP BY, ORDER BY and LIMIT because
// replacing them can change output names or plan selection. Other literals are
// replaced by ordered `?` markers. This stateless lexer/rewriter also preserves
// the Go implementation's concurrent-call isolation.

use crate::Datum;

#[derive(Clone, Debug, Eq, PartialEq)]
enum TokenKind {
    Word(String),
    QuotedIdentifier(String),
    StringLiteral(String),
    Number(String),
    Placeholder,
    Symbol(char),
    Operator(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Token {
    kind: TokenKind,
    start: usize,
    end: usize,
}

impl Token {
    fn raw<'a>(&self, sql: &'a str) -> &'a str {
        &sql[self.start..self.end]
    }
}

/// Return parameterized SQL and copied values without modifying the input.
pub fn GetParamSQLFromAST(sql: &str) -> (String, Vec<Datum>) {
    ParameterizeAST(sql)
}

/// Parameterize one SQL statement, retaining the historical tuple API.
pub fn ParameterizeAST(sql: &str) -> (String, Vec<Datum>) {
    TryParameterizeAST(sql).unwrap_or_else(|error| panic!("parameterize SQL: {error}"))
}

/// Fallible parameterization entry point preserving Go's error path.
pub fn TryParameterizeAST(sql: &str) -> Result<(String, Vec<Datum>), String> {
    let tokens = lex(sql)?;
    validate_single_statement(&tokens)?;
    let Some(first) = tokens.first() else {
        return Err("empty SQL".into());
    };
    if token_is_word(first, "select") {
        parameterize_select(sql, &tokens)
    } else if token_is_word(first, "insert") {
        parameterize_insert(&tokens)
    } else {
        let mut params = Vec::new();
        Ok((format_expression(&tokens, &mut params, true)?, params))
    }
}

fn parameterize_select(sql: &str, tokens: &[Token]) -> Result<(String, Vec<Datum>), String> {
    let from = find_top_level_word(tokens, 1, "from");
    let projection_end = from.unwrap_or(tokens.len());
    let projection = restore_projection(sql, tokens, 1, projection_end);
    let mut output = format!("SELECT {projection}");
    let mut params = Vec::new();
    let Some(from_index) = from else {
        return Ok((output, params));
    };

    let clause_start = [
        find_top_level_word(tokens, from_index + 1, "where"),
        find_top_level_pair(tokens, from_index + 1, "group", "by"),
        find_top_level_pair(tokens, from_index + 1, "order", "by"),
        find_top_level_word(tokens, from_index + 1, "limit"),
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or(tokens.len());
    output.push_str(" FROM ");
    output.push_str(&format_table_tokens(&tokens[from_index + 1..clause_start]));

    let mut cursor = clause_start;
    while cursor < tokens.len() {
        if token_is_word(&tokens[cursor], "where") {
            let end = next_select_clause(tokens, cursor + 1);
            output.push_str(" WHERE ");
            output.push_str(&format_expression(
                &tokens[cursor + 1..end],
                &mut params,
                true,
            )?);
            cursor = end;
        } else if token_is_word(&tokens[cursor], "group")
            && tokens
                .get(cursor + 1)
                .is_some_and(|token| token_is_word(token, "by"))
        {
            let end = next_select_clause(tokens, cursor + 2);
            output.push_str(" GROUP BY ");
            output.push_str(&format_expression(
                &tokens[cursor + 2..end],
                &mut params,
                false,
            )?);
            cursor = end;
        } else if token_is_word(&tokens[cursor], "order")
            && tokens
                .get(cursor + 1)
                .is_some_and(|token| token_is_word(token, "by"))
        {
            let end = next_select_clause(tokens, cursor + 2);
            output.push_str(" ORDER BY ");
            output.push_str(&format_expression(
                &tokens[cursor + 2..end],
                &mut params,
                false,
            )?);
            cursor = end;
        } else if token_is_word(&tokens[cursor], "limit") {
            let end = next_select_clause(tokens, cursor + 1);
            output.push_str(" LIMIT ");
            output.push_str(&format_expression(
                &tokens[cursor + 1..end],
                &mut params,
                false,
            )?);
            cursor = end;
        } else if matches!(tokens[cursor].kind, TokenKind::Symbol(';')) {
            cursor += 1;
        } else {
            return Err(format!(
                "unsupported SELECT clause near `{}`",
                tokens[cursor].raw(sql)
            ));
        }
    }
    Ok((output, params))
}

fn parameterize_insert(tokens: &[Token]) -> Result<(String, Vec<Datum>), String> {
    let into = find_top_level_word(tokens, 1, "into")
        .ok_or_else(|| "INSERT is missing INTO".to_string())?;
    let values = find_top_level_word(tokens, into + 1, "values")
        .ok_or_else(|| "INSERT is missing VALUES".to_string())?;
    let column_open = tokens[into + 1..values]
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Symbol('(')))
        .map(|offset| into + 1 + offset);
    let table_end = column_open.unwrap_or(values);
    let mut output = String::from("INSERT INTO ");
    output.push_str(&format_table_tokens(&tokens[into + 1..table_end]));

    if let Some(open) = column_open {
        let close = matching_paren(tokens, open, values)?;
        if close + 1 != values {
            return Err("unexpected tokens between INSERT columns and VALUES".into());
        }
        output.push_str(" (");
        output.push_str(&format_identifier_list(&tokens[open + 1..close])?);
        output.push(')');
    }

    let mut params = Vec::new();
    output.push_str(" VALUES ");
    output.push_str(&format_expression(
        &tokens[values + 1..],
        &mut params,
        true,
    )?);
    Ok((output, params))
}

fn restore_projection(sql: &str, tokens: &[Token], start: usize, end: usize) -> String {
    if start >= end {
        return String::new();
    }
    let mut depth = 0usize;
    let mut part_start = tokens[start].start;
    let mut fields = Vec::new();
    for token in &tokens[start..end] {
        match token.kind {
            TokenKind::Symbol('(') => depth += 1,
            TokenKind::Symbol(')') => depth = depth.saturating_sub(1),
            TokenKind::Symbol(',') if depth == 0 => {
                fields.push(sql[part_start..token.start].trim());
                part_start = token.end;
            }
            _ => {}
        }
    }
    fields.push(sql[part_start..tokens[end - 1].end].trim());
    fields.join(",")
}

fn next_select_clause(tokens: &[Token], start: usize) -> usize {
    [
        find_top_level_word(tokens, start, "where"),
        find_top_level_pair(tokens, start, "group", "by"),
        find_top_level_pair(tokens, start, "order", "by"),
        find_top_level_word(tokens, start, "limit"),
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or(tokens.len())
}

fn find_top_level_word(tokens: &[Token], start: usize, word: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(start) {
        match token.kind {
            TokenKind::Symbol('(') => depth += 1,
            TokenKind::Symbol(')') => depth = depth.saturating_sub(1),
            _ if depth == 0 && token_is_word(token, word) => return Some(index),
            _ => {}
        }
    }
    None
}

fn find_top_level_pair(tokens: &[Token], start: usize, first: &str, second: &str) -> Option<usize> {
    find_top_level_word(tokens, start, first).filter(|index| {
        tokens
            .get(index + 1)
            .is_some_and(|token| token_is_word(token, second))
    })
}

fn matching_paren(tokens: &[Token], open: usize, upper_bound: usize) -> Result<usize, String> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(upper_bound).skip(open) {
        match token.kind {
            TokenKind::Symbol('(') => depth += 1,
            TokenKind::Symbol(')') => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Ok(index);
                }
            }
            _ => {}
        }
    }
    Err("unclosed parenthesis".into())
}

fn format_identifier_list(tokens: &[Token]) -> Result<String, String> {
    let mut output = String::new();
    for token in tokens {
        match &token.kind {
            TokenKind::Word(name) | TokenKind::QuotedIdentifier(name) => {
                output.push_str(&quote_identifier(name));
            }
            TokenKind::Symbol(',') => output.push(','),
            _ => return Err("invalid identifier list".into()),
        }
    }
    Ok(output)
}

fn format_table_tokens(tokens: &[Token]) -> String {
    let mut output = String::new();
    for token in tokens {
        match &token.kind {
            TokenKind::Word(word) if table_keyword(word) => {
                push_spaced(&mut output, &word.to_ascii_uppercase());
            }
            TokenKind::Word(name) | TokenKind::QuotedIdentifier(name) => {
                if output.ends_with('`') {
                    output.push(' ');
                }
                output.push_str(&quote_identifier(name));
            }
            TokenKind::Symbol('.') => output.push('.'),
            TokenKind::Symbol(',') => output.push(','),
            TokenKind::Symbol(ch) => output.push(*ch),
            TokenKind::Operator(operator) => output.push_str(operator),
            TokenKind::Number(number) => output.push_str(number),
            TokenKind::StringLiteral(value) => output.push_str(&quote_string(value)),
            TokenKind::Placeholder => output.push('?'),
        }
    }
    output.trim().to_owned()
}

fn format_expression(
    tokens: &[Token],
    params: &mut Vec<Datum>,
    parameterize: bool,
) -> Result<String, String> {
    let mut output = String::new();
    let mut index = 0usize;
    while index < tokens.len() {
        let token = &tokens[index];
        if let TokenKind::Word(name) = &token.kind
            && tokens
                .get(index + 1)
                .is_some_and(|next| matches!(next.kind, TokenKind::Symbol('(')))
        {
            let close = matching_paren(tokens, index + 1, tokens.len())?;
            let special = is_format_function(name);
            output.push_str(name);
            output.push('(');
            let arguments = split_top_level(&tokens[index + 2..close], ',');
            for (argument_index, argument) in arguments.iter().enumerate() {
                if argument_index != 0 {
                    output.push_str(if special { ", " } else { "," });
                }
                output.push_str(&format_expression(
                    argument,
                    params,
                    parameterize && (!special || argument_index == 0),
                )?);
            }
            output.push(')');
            index = close + 1;
            continue;
        }

        match &token.kind {
            TokenKind::Word(word) if literal_keyword(word) => {
                if parameterize {
                    params.push(match word.to_ascii_lowercase().as_str() {
                        "null" => Datum::Null,
                        "true" => Datum::Bool(true),
                        "false" => Datum::Bool(false),
                        _ => unreachable!(),
                    });
                    output.push('?');
                } else {
                    output.push_str(&word.to_ascii_uppercase());
                }
            }
            TokenKind::Word(word) if expression_keyword(word) => {
                push_spaced(&mut output, &word.to_ascii_uppercase());
            }
            TokenKind::Word(name) | TokenKind::QuotedIdentifier(name) => {
                output.push_str(&quote_identifier(name));
            }
            TokenKind::StringLiteral(value) => {
                if parameterize {
                    params.push(Datum::String(value.clone()));
                    output.push('?');
                } else {
                    output.push_str(&quote_string(value));
                }
            }
            TokenKind::Number(number) => {
                if parameterize {
                    params.push(number_to_datum(number)?);
                    output.push('?');
                } else {
                    output.push_str(number);
                }
            }
            TokenKind::Placeholder => output.push('?'),
            TokenKind::Symbol('(') => {
                let close = matching_paren(tokens, index, tokens.len())?;
                output.push('(');
                output.push_str(&format_expression(
                    &tokens[index + 1..close],
                    params,
                    parameterize,
                )?);
                output.push(')');
                index = close + 1;
                continue;
            }
            TokenKind::Symbol(',') => output.push(','),
            TokenKind::Symbol(';') => {}
            TokenKind::Symbol(ch) => output.push(*ch),
            TokenKind::Operator(operator) => output.push_str(operator),
        }
        index += 1;
    }
    Ok(output.trim().to_owned())
}

fn split_top_level(tokens: &[Token], separator: char) -> Vec<&[Token]> {
    if tokens.is_empty() {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::Symbol('(') => depth += 1,
            TokenKind::Symbol(')') => depth = depth.saturating_sub(1),
            TokenKind::Symbol(ch) if ch == separator && depth == 0 => {
                result.push(&tokens[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    result.push(&tokens[start..]);
    result
}

fn number_to_datum(number: &str) -> Result<Datum, String> {
    if number.contains(['.', 'e', 'E']) {
        return number
            .parse::<f64>()
            .map(Datum::Float)
            .map_err(|error| format!("invalid numeric literal `{number}`: {error}"));
    }
    number
        .parse::<i64>()
        .map(Datum::Int)
        .or_else(|_| number.parse::<u64>().map(Datum::UInt))
        .map_err(|error| format!("invalid integer literal `{number}`: {error}"))
}

fn token_is_word(token: &Token, expected: &str) -> bool {
    matches!(&token.kind, TokenKind::Word(word) if word.eq_ignore_ascii_case(expected))
}

fn literal_keyword(word: &str) -> bool {
    word.eq_ignore_ascii_case("null")
        || word.eq_ignore_ascii_case("true")
        || word.eq_ignore_ascii_case("false")
}

fn expression_keyword(word: &str) -> bool {
    [
        "and", "or", "xor", "is", "not", "like", "in", "between", "exists", "collate", "asc",
        "desc",
    ]
    .iter()
    .any(|keyword| word.eq_ignore_ascii_case(keyword))
}

fn table_keyword(word: &str) -> bool {
    [
        "as",
        "join",
        "left",
        "right",
        "inner",
        "outer",
        "cross",
        "straight_join",
        "on",
        "using",
        "partition",
        "use",
        "ignore",
        "force",
        "index",
        "key",
    ]
    .iter()
    .any(|keyword| word.eq_ignore_ascii_case(keyword))
}

fn is_format_function(name: &str) -> bool {
    ["date_format", "str_to_date", "time_format", "from_unixtime"]
        .iter()
        .any(|function| name.eq_ignore_ascii_case(function))
}

fn push_spaced(output: &mut String, text: &str) {
    if !output.is_empty() && !output.ends_with(' ') && !output.ends_with('(') {
        output.push(' ');
    }
    output.push_str(text);
    output.push(' ');
}

fn quote_identifier(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}

fn quote_string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
}

/// Restore markers outside quoted text with SQL literals.
pub fn RestoreASTWithParams(sql: &str, params: &[Datum]) -> Result<String, String> {
    let mut output = String::with_capacity(sql.len());
    let mut params = params.iter();
    let mut chars = sql.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        match ch {
            '\'' | '"' | '`' => {
                output.push(ch);
                while let Some((_, current)) = chars.next() {
                    output.push(current);
                    if current == '\\' {
                        if let Some((_, escaped)) = chars.next() {
                            output.push(escaped);
                        }
                    } else if current == ch {
                        if chars.peek().is_some_and(|(_, next)| *next == ch) {
                            output.push(chars.next().expect("peeked quote").1);
                        } else {
                            break;
                        }
                    }
                }
            }
            '?' => output.push_str(&datum_sql_literal(
                params.next().ok_or("not enough parameters")?,
            )),
            _ => output.push(ch),
        }
    }
    if params.next().is_some() {
        return Err("too many parameters".into());
    }
    Ok(output)
}

fn datum_sql_literal(datum: &Datum) -> String {
    match datum {
        Datum::Null => "NULL".into(),
        Datum::Int(value) => value.to_string(),
        Datum::UInt(value) => value.to_string(),
        Datum::Float(value) => value.to_string(),
        Datum::Bytes(value) => format!(
            "x'{}'",
            value
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ),
        Datum::String(value) | Datum::Json(value) => quote_string(value),
        Datum::Bool(value) => {
            if *value {
                "TRUE".into()
            } else {
                "FALSE".into()
            }
        }
    }
}

/// Convert parameters to this crate's expression-value representation.
pub fn Params2Expressions(params: &[Datum]) -> Vec<Datum> {
    params.to_vec()
}

/// Validate one parameterized statement and return its text.
pub fn ParseParameterizedSQL(sql: &str) -> Result<String, String> {
    let tokens = lex(sql)?;
    validate_single_statement(&tokens)?;
    if tokens.is_empty() {
        Err("empty SQL".into())
    } else {
        Ok(sql.into())
    }
}

fn validate_single_statement(tokens: &[Token]) -> Result<(), String> {
    let mut depth = 0usize;
    let mut ended = false;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::Symbol('(') => depth += 1,
            TokenKind::Symbol(')') => {
                if depth == 0 {
                    return Err("unmatched closing parenthesis".into());
                }
                depth -= 1;
            }
            TokenKind::Symbol(';') if depth == 0 => {
                if index + 1 != tokens.len() || ended {
                    return Err("unexpected multiple statements".into());
                }
                ended = true;
            }
            _ if ended => return Err("unexpected tokens after statement end".into()),
            _ => {}
        }
    }
    if depth != 0 {
        return Err("unclosed parenthesis".into());
    }
    Ok(())
}

fn lex(sql: &str) -> Result<Vec<Token>, String> {
    let bytes = sql.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        let start = index;
        let byte = bytes[index];
        if byte == b'`' {
            let (value, end) = scan_quoted(sql, index, b'`')?;
            tokens.push(Token {
                kind: TokenKind::QuotedIdentifier(value),
                start,
                end,
            });
            index = end;
        } else if byte == b'\'' || byte == b'"' {
            let (value, end) = scan_quoted(sql, index, byte)?;
            tokens.push(Token {
                kind: TokenKind::StringLiteral(value),
                start,
                end,
            });
            index = end;
        } else if byte.is_ascii_digit() {
            index += 1;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric()
                    || matches!(bytes[index], b'.' | b'+' | b'-'))
            {
                if matches!(bytes[index], b'+' | b'-') && !matches!(bytes[index - 1], b'e' | b'E') {
                    break;
                }
                index += 1;
            }
            tokens.push(Token {
                kind: TokenKind::Number(sql[start..index].to_owned()),
                start,
                end: index,
            });
        } else if byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$') {
            index += 1;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric()
                    || matches!(bytes[index], b'_' | b'$' | b'#'))
            {
                index += 1;
            }
            tokens.push(Token {
                kind: TokenKind::Word(sql[start..index].to_owned()),
                start,
                end: index,
            });
        } else if byte == b'?' {
            index += 1;
            tokens.push(Token {
                kind: TokenKind::Placeholder,
                start,
                end: index,
            });
        } else {
            let two = bytes
                .get(index..index + 2)
                .and_then(|slice| std::str::from_utf8(slice).ok());
            let three = bytes
                .get(index..index + 3)
                .and_then(|slice| std::str::from_utf8(slice).ok());
            let operator = if three == Some("<=>") {
                Some("<=>")
            } else if matches!(
                two,
                Some("<=" | ">=" | "<>" | "!=" | ":=" | "||" | "&&" | "<<" | ">>")
            ) {
                two
            } else {
                None
            };
            if let Some(operator) = operator {
                index += operator.len();
                tokens.push(Token {
                    kind: TokenKind::Operator(operator.to_owned()),
                    start,
                    end: index,
                });
            } else {
                let ch = sql[index..]
                    .chars()
                    .next()
                    .ok_or_else(|| "invalid UTF-8 boundary".to_string())?;
                index += ch.len_utf8();
                let kind = if matches!(ch, '(' | ')' | ',' | '.' | ';') {
                    TokenKind::Symbol(ch)
                } else {
                    TokenKind::Operator(ch.to_string())
                };
                tokens.push(Token {
                    kind,
                    start,
                    end: index,
                });
            }
        }
    }
    Ok(tokens)
}

fn scan_quoted(sql: &str, start: usize, quote: u8) -> Result<(String, usize), String> {
    let bytes = sql.as_bytes();
    let mut value = String::new();
    let mut index = start + 1;
    let mut segment_start = index;
    while index < bytes.len() {
        if bytes[index] == quote {
            value.push_str(&sql[segment_start..index]);
            if bytes.get(index + 1) == Some(&quote) {
                value.push(quote as char);
                index += 2;
                segment_start = index;
                continue;
            }
            return Ok((value, index + 1));
        }
        if bytes[index] == b'\\' {
            value.push_str(&sql[segment_start..index]);
            let escaped = bytes
                .get(index + 1)
                .copied()
                .ok_or_else(|| "unterminated escape sequence".to_string())?;
            value.push(escaped as char);
            index += 2;
            segment_start = index;
            continue;
        }
        index += 1;
    }
    Err(format!("unterminated {} quote", quote as char))
}
