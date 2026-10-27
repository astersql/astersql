// Copyright 2026 AsterSQL.

//! Validated PostgreSQL JSON text, independent of MySQL BinaryJSON.
use std::{fmt, str::FromStr};

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PgJsonError {
    #[error("invalid input syntax for type json")]
    InvalidSyntax,
    #[error("invalid byte sequence for encoding UTF8")]
    InvalidEncoding,
}
impl PgJsonError {
    pub const fn sqlstate(self) -> &'static str {
        match self {
            Self::InvalidSyntax => "22P02",
            Self::InvalidEncoding => "22021",
        }
    }
}

/// Validated original spelling. SQL NULL belongs to the outer value container.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct PgJson(String);
impl PgJson {
    /// Decode UTF-8 before JSON syntax, rejecting raw NUL as an encoding error.
    pub fn from_bytes(input: &[u8]) -> Result<Self, PgJsonError> {
        let input = std::str::from_utf8(input).map_err(|_| PgJsonError::InvalidEncoding)?;
        if input.contains('\0') {
            return Err(PgJsonError::InvalidEncoding);
        }
        input.parse()
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}
impl FromStr for PgJson {
    type Err = PgJsonError;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if input.contains('\0') {
            return Err(PgJsonError::InvalidEncoding);
        }
        validate(input.as_bytes())?;
        Ok(Self(input.to_owned()))
    }
}
impl fmt::Display for PgJson {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy)]
enum State {
    Value,
    ArrayFirst,
    ArrayNext,
    ObjectFirst,
    ObjectKey,
    Colon,
    ObjectNext,
    End,
}

/// Validate without constructing decoded values: json accepts lexical Unicode
/// escapes and numbers that cannot be represented by jsonb or machine floats.
/// An explicit continuation stack keeps nesting off the Rust call stack.
fn validate(input: &[u8]) -> Result<(), PgJsonError> {
    let mut lexer = Lexer { input, pos: 0 };
    let mut states = vec![State::End, State::Value];
    while let Some(state) = states.pop() {
        lexer.whitespace();
        match state {
            State::Value => match lexer.peek() {
                Some(b'{') => {
                    lexer.pos += 1;
                    states.push(State::ObjectFirst);
                }
                Some(b'[') => {
                    lexer.pos += 1;
                    states.push(State::ArrayFirst);
                }
                Some(b'"') => lexer.string()?,
                Some(b'-' | b'0'..=b'9') => lexer.number()?,
                Some(b't') => lexer.literal(b"true")?,
                Some(b'f') => lexer.literal(b"false")?,
                Some(b'n') => lexer.literal(b"null")?,
                _ => return Err(PgJsonError::InvalidSyntax),
            },
            State::ArrayFirst => {
                if !lexer.take(b']') {
                    states.extend([State::ArrayNext, State::Value]);
                }
            }
            State::ArrayNext => {
                if lexer.take(b',') {
                    states.extend([State::ArrayNext, State::Value]);
                } else {
                    lexer.require(b']')?;
                }
            }
            State::ObjectFirst => {
                if !lexer.take(b'}') {
                    states.push(State::ObjectKey);
                }
            }
            State::ObjectKey => {
                lexer.string()?;
                states.push(State::Colon);
            }
            State::Colon => {
                lexer.require(b':')?;
                states.extend([State::ObjectNext, State::Value]);
            }
            State::ObjectNext => {
                if lexer.take(b',') {
                    states.push(State::ObjectKey);
                } else {
                    lexer.require(b'}')?;
                }
            }
            State::End => {
                if lexer.peek().is_some() {
                    return Err(PgJsonError::InvalidSyntax);
                }
            }
        }
    }
    Ok(())
}

struct Lexer<'a> {
    input: &'a [u8],
    pos: usize,
}

impl Lexer<'_> {
    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn take(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn require(&mut self, byte: u8) -> Result<(), PgJsonError> {
        if self.take(byte) {
            Ok(())
        } else {
            Err(PgJsonError::InvalidSyntax)
        }
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.pos += 1;
        }
    }

    fn literal(&mut self, text: &[u8]) -> Result<(), PgJsonError> {
        if self.input[self.pos..].starts_with(text) {
            self.pos += text.len();
            Ok(())
        } else {
            Err(PgJsonError::InvalidSyntax)
        }
    }

    fn string(&mut self) -> Result<(), PgJsonError> {
        self.require(b'"')?;
        while let Some(byte) = self.peek() {
            self.pos += 1;
            match byte {
                b'"' => return Ok(()),
                0..=0x1f => return Err(PgJsonError::InvalidSyntax),
                b'\\' => {
                    match self.peek() {
                        Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => {
                            self.pos += 1
                        }
                        Some(b'u') => {
                            self.pos += 1;
                            // Check four hex digits only. json deliberately does not
                            // decode code points or require surrogate pairing here.
                            for _ in 0..4 {
                                if !self.peek().is_some_and(|b| b.is_ascii_hexdigit()) {
                                    return Err(PgJsonError::InvalidSyntax);
                                }
                                self.pos += 1;
                            }
                        }
                        _ => return Err(PgJsonError::InvalidSyntax),
                    }
                }
                _ => {}
            }
        }
        Err(PgJsonError::InvalidSyntax)
    }

    fn digits(&mut self) -> Result<(), PgJsonError> {
        let start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        if self.pos == start {
            Err(PgJsonError::InvalidSyntax)
        } else {
            Ok(())
        }
    }

    fn number(&mut self) -> Result<(), PgJsonError> {
        self.take(b'-');
        if !self.take(b'0') {
            self.digits()?;
        }
        if self.take(b'.') {
            self.digits()?;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if !self.take(b'+') {
                self.take(b'-');
            }
            self.digits()?;
        }
        Ok(())
    }
}
