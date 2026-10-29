// Copyright 2026 AsterSQL.
//! The int4range value, independent of SQL NULL and of textual presentation.
use std::{fmt, str::FromStr};

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct PgInt4RangeError {
    pub state: &'static str,
    pub message: String,
}
impl PgInt4RangeError {
    pub(crate) fn new(state: &'static str, message: impl Into<String>) -> Self {
        Self {
            state,
            message: message.into(),
        }
    }
}

/// Nonempty finite bounds always denote [lower, upper). Missing endpoints are
/// infinite, never inclusive. Private fields prevent uncanonical values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PgInt4Range {
    empty: bool,
    lower: Option<i32>,
    upper: Option<i32>,
}
impl PgInt4Range {
    pub const OID: u32 = 3904;
    pub const SUBTYPE_OID: u32 = 23;
    pub fn empty() -> Self {
        Self {
            empty: true,
            lower: None,
            upper: None,
        }
    }
    pub fn is_empty(self) -> bool {
        self.empty
    }
    pub fn lower(self) -> Option<i32> {
        self.lower
    }
    pub fn upper(self) -> Option<i32> {
        self.upper
    }
    pub fn new(
        lower: Option<i32>,
        upper: Option<i32>,
        flags: Option<&str>,
    ) -> Result<Self, PgInt4RangeError> {
        let flags = flags.ok_or_else(|| {
            PgInt4RangeError::new("22000", "range constructor flags argument must not be null")
        })?;
        if !matches!(flags, "[]" | "[)" | "(]" | "()") {
            return Err(PgInt4RangeError::new("42601", "invalid range bound flags"));
        }
        let lower_inc = flags.as_bytes()[0] == b'[';
        let upper_inc = flags.as_bytes()[1] == b']';
        if let (Some(a), Some(b)) = (lower, upper) {
            if a > b {
                return Err(PgInt4RangeError::new(
                    "22000",
                    "range lower bound must be less than or equal to range upper bound",
                ));
            }
            if a == b && !(lower_inc && upper_inc) {
                return Ok(Self::empty());
            }
        }
        let increment = |n: i32| {
            n.checked_add(1)
                .ok_or_else(|| PgInt4RangeError::new("22003", "integer out of range"))
        };
        let lower = if lower_inc {
            lower
        } else {
            lower.map(increment).transpose()?
        };
        let upper = if upper_inc {
            upper.map(increment).transpose()?
        } else {
            upper
        };
        if matches!((lower,upper),(Some(a),Some(b)) if a==b) {
            return Ok(Self::empty());
        }
        Ok(Self {
            empty: false,
            lower,
            upper,
        })
    }
    pub fn to_binary(self) -> Vec<u8> {
        if self.empty {
            return vec![1];
        }
        let mut bytes = vec![
            if self.lower.is_none() { 8 } else { 2 } | if self.upper.is_none() { 16 } else { 0 },
        ];
        for bound in [self.lower, self.upper].into_iter().flatten() {
            bytes.extend_from_slice(&4i32.to_be_bytes());
            bytes.extend_from_slice(&bound.to_be_bytes());
        }
        bytes
    }
    pub fn from_binary(bytes: &[u8]) -> Result<Self, PgInt4RangeError> {
        let truncated = || PgInt4RangeError::new("08P01", "insufficient data left in message");
        let (&flags, rest) = bytes.split_first().ok_or_else(truncated)?;
        // range_recv masks the reserved flag bits before receiving the bounds.
        let flags = flags & 31;
        let mut rest = rest;
        let mut bound = |absent: bool| -> Result<Option<i32>, PgInt4RangeError> {
            if absent {
                return Ok(None);
            }
            let len = i32::from_be_bytes(rest.get(..4).ok_or_else(truncated)?.try_into().unwrap());
            rest = &rest[4..];
            if len < 0 {
                return Err(PgInt4RangeError::new(
                    "08P01",
                    "insufficient data left in message",
                ));
            }
            let payload = rest.get(..len as usize).ok_or_else(truncated)?;
            rest = &rest[len as usize..];
            if payload.len() < 4 {
                return Err(truncated());
            }
            // int4recv reads four bytes; range_recv does not require its bound
            // buffer to be exhausted, but does require the outer one to end.
            Ok(Some(i32::from_be_bytes(payload[..4].try_into().unwrap())))
        };
        let lower = bound(flags & (1 | 8) != 0)?;
        let upper = bound(flags & (1 | 16) != 0)?;
        if !rest.is_empty() {
            return Err(PgInt4RangeError::new("22P03", "invalid message format"));
        }
        if flags & 1 != 0 {
            return Ok(Self::empty());
        }
        let flags = match (flags & 2 != 0, flags & 4 != 0) {
            (true, true) => "[]",
            (true, false) => "[)",
            (false, true) => "(]",
            _ => "()",
        };
        Self::new(lower, upper, Some(flags))
    }
}
impl fmt::Display for PgInt4Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.empty {
            return f.write_str("empty");
        }
        write!(f, "{}", if self.lower.is_some() { '[' } else { '(' })?;
        if let Some(n) = self.lower {
            write!(f, "{n}")?;
        }
        f.write_str(",")?;
        if let Some(n) = self.upper {
            write!(f, "{n}")?;
        }
        f.write_str(")")
    }
}
impl PgInt4Range {
    /// Decode container syntax and delegate each finite endpoint to the installed
    /// int4 input function, retaining its grammar and diagnostic precedence.
    pub fn from_text_with<F>(text: &str, mut scalar: F) -> Result<Self, PgInt4RangeError>
    where
        F: FnMut(&str) -> Result<i32, PgInt4RangeError>,
    {
        let text = text.trim_matches(|c: char| c.is_ascii_whitespace());
        if text == "empty" {
            return Ok(Self::empty());
        }
        let syntax =
            || PgInt4RangeError::new("22P02", format!("malformed range literal: \"{text}\""));
        let bytes = text.as_bytes();
        if bytes.len() < 3 || !matches!(bytes[0], b'[' | b'(') {
            return Err(syntax());
        }
        fn bound(bytes: &[u8], pos: &mut usize) -> Result<Option<String>, PgInt4RangeError> {
            let syntax = || PgInt4RangeError::new("22P02", "malformed range literal");
            let start = *pos;
            let mut quoted = false;
            let mut decoded = Vec::new();
            while *pos < bytes.len() {
                let ch = bytes[*pos];
                if !quoted && matches!(ch, b',' | b')' | b']') {
                    break;
                }
                *pos += 1;
                match ch {
                    b'\\' => {
                        decoded.push(*bytes.get(*pos).ok_or_else(syntax)?);
                        *pos += 1;
                    }
                    b'"' => {
                        if quoted && bytes.get(*pos) == Some(&b'"') {
                            decoded.push(b'"');
                            *pos += 1;
                        } else {
                            quoted = !quoted;
                        }
                    }
                    _ => decoded.push(ch),
                }
            }
            if quoted || *pos >= bytes.len() {
                return Err(syntax());
            }
            if *pos == start {
                return Ok(None);
            }
            String::from_utf8(decoded).map(Some).map_err(|_| syntax())
        }
        let mut pos = 1;
        let lower = bound(bytes, &mut pos)?;
        if bytes.get(pos) != Some(&b',') {
            return Err(syntax());
        }
        pos += 1;
        let upper = bound(bytes, &mut pos)?;
        if pos + 1 != bytes.len() || !matches!(bytes.get(pos), Some(b')' | b']')) {
            return Err(syntax());
        }
        // range_in validates the entire container before calling either
        // subtype input function, even when a bound itself would overflow.
        let lower = lower.as_deref().map(&mut scalar).transpose()?;
        let upper = upper.as_deref().map(&mut scalar).transpose()?;
        let flags = [bytes[0], bytes[pos]];
        Self::new(lower, upper, Some(std::str::from_utf8(&flags).unwrap()))
    }
}

/// Decimal canonical text is sufficient at the engine result boundary. Input
/// adapters use from_text_with to consume the complete PostgreSQL int4 grammar.
impl FromStr for PgInt4Range {
    type Err = PgInt4RangeError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::from_text_with(text, |value| {
            let value = value.trim_matches(|c: char| c.is_ascii_whitespace());
            value.parse::<i32>().map_err(|e| {
                if matches!(
                    e.kind(),
                    std::num::IntErrorKind::PosOverflow | std::num::IntErrorKind::NegOverflow
                ) {
                    PgInt4RangeError::new(
                        "22003",
                        format!("value \"{value}\" is out of range for type integer"),
                    )
                } else {
                    PgInt4RangeError::new(
                        "22P02",
                        format!("invalid input syntax for type integer: \"{value}\""),
                    )
                }
            })
        })
    }
}
