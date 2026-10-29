// Copyright 2026 AsterSQL.
//! The numrange value, independent of SQL NULL and of textual presentation.
pub use crate::pg_int4range::PgInt4RangeError as PgNumRangeError;
use crate::pg_numeric::PgNumeric;
use std::{fmt, str::FromStr};

/// Continuous numeric bounds retain their inclusion flags and exact decimal scale.
/// Missing endpoints are infinite, never inclusive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PgNumRange {
    empty: bool,
    lower: Option<PgNumeric>,
    upper: Option<PgNumeric>,
    lower_inc: bool,
    upper_inc: bool,
}
impl PgNumRange {
    pub const OID: u32 = 3906;
    pub const SUBTYPE_OID: u32 = 1700;
    pub fn empty() -> Self {
        Self {
            empty: true,
            lower: None,
            upper: None,
            lower_inc: false,
            upper_inc: false,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.empty
    }
    pub fn lower(&self) -> Option<&PgNumeric> {
        self.lower.as_ref()
    }
    pub fn upper(&self) -> Option<&PgNumeric> {
        self.upper.as_ref()
    }
    pub fn new(
        lower: Option<PgNumeric>,
        upper: Option<PgNumeric>,
        flags: Option<&str>,
    ) -> Result<Self, PgNumRangeError> {
        let flags = flags.ok_or_else(|| {
            PgNumRangeError::new("22000", "range constructor flags argument must not be null")
        })?;
        if !matches!(flags, "[]" | "[)" | "(]" | "()") {
            return Err(PgNumRangeError::new("42601", "invalid range bound flags"));
        }
        let lower_inc = flags.as_bytes()[0] == b'[';
        let upper_inc = flags.as_bytes()[1] == b']';
        if let (Some(a), Some(b)) = (&lower, &upper) {
            if numeric_order(a, b).is_gt() {
                return Err(PgNumRangeError::new(
                    "22000",
                    "range lower bound must be less than or equal to range upper bound",
                ));
            }
            if numeric_order(a, b).is_eq() && !(lower_inc && upper_inc) {
                return Ok(Self::empty());
            }
        }
        Ok(Self {
            empty: false,
            lower_inc: lower.is_some() && lower_inc,
            upper_inc: upper.is_some() && upper_inc,
            lower,
            upper,
        })
    }
    pub fn to_binary(&self) -> Vec<u8> {
        if self.empty {
            return vec![1];
        }
        let mut bytes = vec![
            if self.lower.is_none() {
                8
            } else if self.lower_inc {
                2
            } else {
                0
            } | if self.upper.is_none() {
                16
            } else if self.upper_inc {
                4
            } else {
                0
            },
        ];
        for bound in [self.lower.as_ref(), self.upper.as_ref()]
            .into_iter()
            .flatten()
        {
            let payload = bound.to_binary();
            bytes.extend_from_slice(&(payload.len() as i32).to_be_bytes());
            bytes.extend_from_slice(&payload);
        }
        bytes
    }
    pub fn from_binary(bytes: &[u8]) -> Result<Self, PgNumRangeError> {
        let truncated = || PgNumRangeError::new("08P01", "insufficient data left in message");
        let (&flags, rest) = bytes.split_first().ok_or_else(truncated)?;
        // range_recv masks the reserved flag bits before receiving the bounds.
        let flags = flags & 31;
        let mut rest = rest;
        let mut bound = |absent: bool| -> Result<Option<PgNumeric>, PgNumRangeError> {
            if absent {
                return Ok(None);
            }
            let len = i32::from_be_bytes(rest.get(..4).ok_or_else(truncated)?.try_into().unwrap());
            rest = &rest[4..];
            if len < 0 {
                return Err(PgNumRangeError::new(
                    "08P01",
                    "insufficient data left in message",
                ));
            }
            let payload = rest.get(..len as usize).ok_or_else(truncated)?;
            rest = &rest[len as usize..];
            PgNumeric::from_binary(payload)
                .map(Some)
                .map_err(|e| PgNumRangeError::new(e.sqlstate(), e.to_string()))
        };
        let lower = bound(flags & (1 | 8) != 0)?;
        let upper = bound(flags & (1 | 16) != 0)?;
        if !rest.is_empty() {
            return Err(PgNumRangeError::new("22P03", "invalid message format"));
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
impl fmt::Display for PgNumRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.empty {
            return f.write_str("empty");
        }
        write!(f, "{}", if self.lower_inc { '[' } else { '(' })?;
        if let Some(n) = &self.lower {
            write!(f, "{n}")?;
        }
        f.write_str(",")?;
        if let Some(n) = &self.upper {
            write!(f, "{n}")?;
        }
        f.write_str(if self.upper_inc { "]" } else { ")" })
    }
}
impl PgNumRange {
    /// Decode container syntax and delegate each finite endpoint to the installed
    /// numeric input function, retaining its grammar and diagnostic precedence.
    pub fn from_text_with<F>(text: &str, mut scalar: F) -> Result<Self, PgNumRangeError>
    where
        F: FnMut(&str) -> Result<PgNumeric, PgNumRangeError>,
    {
        let text = text.trim_matches(|c: char| c.is_ascii_whitespace());
        if text == "empty" {
            return Ok(Self::empty());
        }
        let syntax =
            || PgNumRangeError::new("22P02", format!("malformed range literal: \"{text}\""));
        let bytes = text.as_bytes();
        if bytes.len() < 3 || !matches!(bytes[0], b'[' | b'(') {
            return Err(syntax());
        }
        fn bound(bytes: &[u8], pos: &mut usize) -> Result<Option<String>, PgNumRangeError> {
            let syntax = || PgNumRangeError::new("22P02", "malformed range literal");
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

impl FromStr for PgNumRange {
    type Err = PgNumRangeError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::from_text_with(text, |s| {
            s.parse::<PgNumeric>()
                .map_err(|e| PgNumRangeError::new(e.sqlstate(), e.to_string()))
        })
    }
}

fn numeric_order(left: &PgNumeric, right: &PgNumeric) -> std::cmp::Ordering {
    let l = left.to_string();
    let r = right.to_string();
    let left = l.as_str();
    let right = r.as_str();
    let rank = |s: &str| match s {
        "-Infinity" => 0,
        "Infinity" => 2,
        "NaN" => 3,
        _ => 1,
    };
    let order = rank(left).cmp(&rank(right));
    if !order.is_eq() || rank(left) != 1 {
        return order;
    }
    let negative_left = left.starts_with('-');
    let negative_right = right.starts_with('-');
    if negative_left != negative_right {
        return negative_right.cmp(&negative_left);
    }
    let digits = |s: &str| {
        let (whole, fraction) = s
            .trim_start_matches('-')
            .split_once('.')
            .unwrap_or((s.trim_start_matches('-'), ""));
        (
            whole.trim_start_matches('0').to_owned(),
            fraction.trim_end_matches('0').to_owned(),
        )
    };
    let (li, lf) = digits(left);
    let (ri, rf) = digits(right);
    let order = li
        .len()
        .cmp(&ri.len())
        .then_with(|| li.cmp(&ri))
        .then_with(|| lf.cmp(&rf));
    if negative_left {
        order.reverse()
    } else {
        order
    }
}
