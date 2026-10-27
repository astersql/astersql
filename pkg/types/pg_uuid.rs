// Copyright 2026 AsterSQL.

//! PostgreSQL UUID values, independent of text spelling and MySQL binary values.
use std::{fmt, str::FromStr};

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("invalid input syntax for type uuid")]
pub struct PgUuidError;

impl PgUuidError {
    pub fn sqlstate(self) -> &'static str {
        "22P02"
    }
}

/// All 128-bit patterns are valid; SQL NULL belongs to the outer value container.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct PgUuid([u8; 16]);

impl PgUuid {
    /// Bytes follow textual order, with no host-endian or GUID byte swapping.
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl FromStr for PgUuid {
    type Err = PgUuidError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let mut input = input.as_bytes();
        if let Some(inner) = input.strip_prefix(b"{") {
            input = inner.strip_suffix(b"}").ok_or(PgUuidError)?;
        }
        let mut bytes = [0; 16];
        let mut pos = 0;
        for (i, byte) in bytes.iter_mut().enumerate() {
            let high = hex_digit(*input.get(pos).ok_or(PgUuidError)?)?;
            let low = hex_digit(*input.get(pos + 1).ok_or(PgUuidError)?)?;
            *byte = (high << 4) | low;
            pos += 2;
            // A single hyphen is optional after each four digits, except at EOF.
            if i % 2 == 1 && i < 15 && input.get(pos) == Some(&b'-') {
                pos += 1;
            }
        }
        if pos != input.len() {
            return Err(PgUuidError);
        }
        Ok(Self(bytes))
    }
}

impl fmt::Display for PgUuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, byte) in self.0.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

fn hex_digit(byte: u8) -> Result<u8, PgUuidError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(PgUuidError),
    }
}
