// Copyright 2026 AsterSQL.

//! PostgreSQL 3.0/3.2 startup framing. This module does not authenticate, open a
//! session, or share MySQL packet framing. SSL and cancellation requests belong
//! to the later negotiation layer and are not startup messages.
use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read};

pub const PROTOCOL_VERSION_30: u32 = 3 << 16;
pub const PROTOCOL_VERSION: u32 = (3 << 16) | 2;
/// Includes the length word; checked before allocating the network payload.
pub const MAX_STARTUP_LENGTH: usize = 10_000;
const MIN_STARTUP_LENGTH: usize = 9;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupMessage {
    pub protocol_version: u32,
    /// Structural parsing preserves unknown parameters for the negotiation layer.
    /// The authentication layer must require a user and validate session options.
    pub parameters: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupError {
    InvalidLength(u32),
    LengthMismatch,
    UnsupportedVersion(u32),
    InvalidParameters,
    InvalidUtf8,
    DuplicateParameter(String),
    Io(io::ErrorKind),
}

impl fmt::Display for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength(n) => write!(f, "invalid PostgreSQL startup length {n}"),
            Self::LengthMismatch => f.write_str("PostgreSQL startup length does not match payload"),
            Self::UnsupportedVersion(v) => write!(
                f,
                "unsupported PostgreSQL protocol {}.{}; only 3.0 and 3.2 are supported",
                v >> 16,
                v & 0xffff
            ),
            Self::InvalidParameters => f.write_str("invalid PostgreSQL startup parameter framing"),
            Self::InvalidUtf8 => f.write_str("PostgreSQL startup parameters must be UTF-8"),
            Self::DuplicateParameter(name) => {
                write!(f, "duplicate PostgreSQL startup parameter {name}")
            }
            Self::Io(kind) => write!(f, "reading PostgreSQL startup failed: {kind}"),
        }
    }
}
impl std::error::Error for StartupError {}

fn checked_length(length: u32) -> Result<usize, StartupError> {
    let length_usize = length as usize;
    if !(MIN_STARTUP_LENGTH..=MAX_STARTUP_LENGTH).contains(&length_usize) {
        return Err(StartupError::InvalidLength(length));
    }
    Ok(length_usize)
}

/// Parse exactly one complete startup packet, including its length word.
/// No allocations are made until the packet length and version are validated.
pub fn parse_startup(packet: &[u8]) -> Result<StartupMessage, StartupError> {
    let header = packet.get(..4).ok_or(StartupError::LengthMismatch)?;
    let length = checked_length(u32::from_be_bytes(header.try_into().unwrap()))?;
    if packet.len() != length {
        return Err(StartupError::LengthMismatch);
    }
    let version = u32::from_be_bytes(packet[4..8].try_into().unwrap());
    if !matches!(version, PROTOCOL_VERSION_30 | PROTOCOL_VERSION) {
        return Err(StartupError::UnsupportedVersion(version));
    }
    let mut remaining = &packet[8..];
    let mut parameters = BTreeMap::new();
    loop {
        let name = take_string(&mut remaining)?;
        if name.is_empty() {
            if !remaining.is_empty() {
                return Err(StartupError::InvalidParameters);
            }
            return Ok(StartupMessage {
                protocol_version: version,
                parameters,
            });
        }
        let value = take_string(&mut remaining)?;
        if parameters
            .insert(name.to_owned(), value.to_owned())
            .is_some()
        {
            return Err(StartupError::DuplicateParameter(name.to_owned()));
        }
    }
}

fn take_string<'a>(remaining: &mut &'a [u8]) -> Result<&'a str, StartupError> {
    let end = remaining
        .iter()
        .position(|b| *b == 0)
        .ok_or(StartupError::InvalidParameters)?;
    let value = std::str::from_utf8(&remaining[..end]).map_err(|_| StartupError::InvalidUtf8)?;
    *remaining = &remaining[end + 1..];
    Ok(value)
}

/// Read one bounded packet without consuming the next packet. The caller owns
/// read deadlines and must close the connection on framing or version errors.
pub fn read_startup(reader: &mut impl Read) -> Result<StartupMessage, StartupError> {
    let mut header = [0; 4];
    reader
        .read_exact(&mut header)
        .map_err(|e| StartupError::Io(e.kind()))?;
    let length = checked_length(u32::from_be_bytes(header))?;
    let mut packet = vec![0; length];
    packet[..4].copy_from_slice(&header);
    reader
        .read_exact(&mut packet[4..])
        .map_err(|e| StartupError::Io(e.kind()))?;
    parse_startup(&packet)
}
