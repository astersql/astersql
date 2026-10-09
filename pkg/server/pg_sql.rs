// Copyright 2026 AsterSQL.
//! Bounded PostgreSQL DDL adaptation for the PostgreSQL listener.

use crate::pg_catalog_query::ParseResult;
use std::collections::BTreeMap;

const MAX_SQL_LENGTH: usize = 1 << 20;
const MAX_TOKENS: usize = 16_384;
const MAX_DEPTH: usize = 128;

#[derive(Clone)]
enum Kind {
    Word(String),
    Identifier(String),
    String,
    Symbol(u8),
}
#[derive(Clone)]
struct Token {
    start: usize,
    end: usize,
    kind: Kind,
}
impl Token {
    fn word(&self, expected: &str) -> bool {
        matches!(&self.kind, Kind::Word(word) if word.eq_ignore_ascii_case(expected))
    }
    fn symbol(&self, expected: u8) -> bool {
        matches!(self.kind, Kind::Symbol(symbol) if symbol == expected)
    }
    fn identifier(&self) -> bool {
        matches!(self.kind, Kind::Word(_) | Kind::Identifier(_))
    }
}

fn error(state: &'static str, message: &str) -> (&'static str, String) {
    (state, message.into())
}
fn quote(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}
fn tokens(sql: &str) -> ParseResult<Vec<Token>> {
    if sql.len() > MAX_SQL_LENGTH {
        return Err(error("54000", "PG SQL exceeds the 1 MiB adaptation limit"));
    }
    let bytes = sql.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    let mut depth = 0usize;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if bytes[i..].starts_with(b"--") {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if bytes[i..].starts_with(b"/*") {
            i += 2;
            while i < bytes.len() && !bytes[i..].starts_with(b"*/") {
                if bytes[i..].starts_with(b"/*") {
                    return Err(error("0A000", "nested PG comments are unsupported"));
                }
                i += 1;
            }
            if i == bytes.len() {
                return Err(error("42601", "unterminated SQL comment"));
            }
            i += 2;
            continue;
        }
        let start = i;
        let kind = if bytes[i] == b'\'' || bytes[i] == b'"' || bytes[i] == b'`' {
            let delimiter = bytes[i];
            let identifier = delimiter != b'\'';
            i += 1;
            let content = i;
            loop {
                if i == bytes.len() {
                    return Err(error("42601", "unterminated SQL quote"));
                }
                if bytes[i] == delimiter {
                    if bytes.get(i + 1) == Some(&delimiter) {
                        i += 2;
                        continue;
                    }
                    let value = if identifier {
                        let doubled = format!("{0}{0}", delimiter as char);
                        Some(sql[content..i].replace(&doubled, &(delimiter as char).to_string()))
                    } else {
                        None
                    };
                    i += 1;
                    break match value {
                        Some(value) if value.is_empty() => {
                            return Err(error("42601", "empty quoted identifier"));
                        }
                        Some(value) => Kind::Identifier(value),
                        None => Kind::String,
                    };
                }
                if delimiter == b'\'' && bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' || bytes[i] >= 128 {
            i += 1;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric()
                    || matches!(bytes[i], b'_' | b'$')
                    || bytes[i] >= 128)
            {
                i += 1;
            }
            Kind::Word(sql[start..i].to_lowercase())
        } else {
            let symbol = bytes[i];
            i += 1;
            match symbol {
                b'(' => {
                    depth += 1;
                    if depth > MAX_DEPTH {
                        return Err(error("54001", "PG SQL nesting exceeds 128 levels"));
                    }
                }
                b')' => {
                    depth = depth
                        .checked_sub(1)
                        .ok_or_else(|| error("42601", "unmatched SQL parenthesis"))?;
                }
                _ => {}
            }
            Kind::Symbol(symbol)
        };
        tokens.push(Token {
            start,
            end: i,
            kind,
        });
        if tokens.len() > MAX_TOKENS {
            return Err(error("54000", "PG SQL exceeds the token limit"));
        }
    }
    if depth != 0 {
        return Err(error("42601", "unclosed SQL parenthesis"));
    }
    Ok(tokens)
}

fn column_type(
    tokens: &[Token],
    start: usize,
    end: usize,
) -> ParseResult<Option<(usize, usize, &'static str)>> {
    let Some(column) = tokens.get(start) else {
        return Ok(None);
    };
    if !column.identifier() {
        return Err(error("42601", "expected CREATE TABLE column name"));
    }
    if [
        "constraint",
        "primary",
        "unique",
        "check",
        "foreign",
        "exclude",
        "like",
    ]
    .iter()
    .any(|word| column.word(word))
    {
        return Ok(None);
    }
    let Some(ty) = tokens.get(start + 1).filter(|token| start + 1 < end) else {
        return Err(error("42601", "expected CREATE TABLE column type"));
    };
    if ty.word("serial") || ty.word("bigserial") {
        return Err(error(
            "0A000",
            "serial identity columns are unsupported until native identity metadata is available",
        ));
    }
    let (last, native) = if ty.word("smallint") {
        (start + 1, "SMALLINT")
    } else if ty.word("integer") {
        (start + 1, "INT")
    } else if ty.word("bigint") {
        (start + 1, "BIGINT")
    } else if ty.word("real") {
        (start + 1, "FLOAT")
    } else if ty.word("double") && tokens.get(start + 2).is_some_and(|t| t.word("precision")) {
        (start + 2, "DOUBLE")
    } else if ty.word("numeric") || ty.word("decimal") {
        (start + 1, "DECIMAL")
    } else if ty.word("boolean") {
        (start + 1, "BOOLEAN")
    } else if ty.word("char") {
        (start + 1, "CHAR")
    } else if ty.word("varchar") {
        (start + 1, "VARCHAR")
    } else if ty.word("text") {
        (start + 1, "TEXT")
    } else if ty.word("bytea") {
        (start + 1, "BLOB")
    } else if ty.word("date") {
        (start + 1, "DATE")
    } else if ty.word("time") {
        (start + 1, "TIME")
    } else if ty.word("timestamp") {
        (start + 1, "TIMESTAMP")
    } else {
        return Err(error(
            "0A000",
            "unsupported PostgreSQL CREATE TABLE column type",
        ));
    };
    Ok(Some((ty.start, tokens[last].end, native)))
}

pub(crate) fn adapt(sql: &str) -> ParseResult<String> {
    let tokens = tokens(sql)?;
    let Some(create) = tokens.first() else {
        return Ok(sql.to_owned());
    };
    let mut edits = BTreeMap::new();
    for token in &tokens {
        if let Kind::Identifier(name) = &token.kind {
            edits.insert(token.start, (token.end, quote(name)));
        }
    }
    if !create.word("create") {
        return apply(sql, edits);
    }
    let mut table = 1;
    if tokens
        .get(table)
        .is_some_and(|t| t.word("temporary") || t.word("temp"))
    {
        table += 1;
    }
    if !tokens.get(table).is_some_and(|t| t.word("table")) {
        return apply(sql, edits);
    }
    let open = tokens
        .iter()
        .enumerate()
        .skip(table + 1)
        .find_map(|(index, token)| token.symbol(b'(').then_some(index))
        .ok_or_else(|| error("42601", "CREATE TABLE requires a column list"))?;
    let mut depth = 1usize;
    let mut segment = open + 1;
    for index in open + 1..tokens.len() {
        if tokens[index].symbol(b'(') {
            depth += 1;
        } else if tokens[index].symbol(b')') {
            depth -= 1;
            if depth == 0 {
                if let Some((start, end, native)) = column_type(&tokens, segment, index)? {
                    edits.insert(start, (end, native.into()));
                }
                break;
            }
        } else if depth == 1 && tokens[index].symbol(b',') {
            if let Some((start, end, native)) = column_type(&tokens, segment, index)? {
                edits.insert(start, (end, native.into()));
            }
            segment = index + 1;
        }
    }
    apply(sql, edits)
}

fn apply(sql: &str, edits: BTreeMap<usize, (usize, String)>) -> ParseResult<String> {
    let mut output = String::with_capacity(sql.len());
    let mut previous = 0;
    for (start, (end, replacement)) in edits {
        if start < previous {
            continue;
        }
        output.push_str(&sql[previous..start]);
        output.push_str(&replacement);
        previous = end;
    }
    output.push_str(&sql[previous..]);
    Ok(output)
}
