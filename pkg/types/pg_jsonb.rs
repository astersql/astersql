// Copyright 2026 AsterSQL.

//! PostgreSQL JSONB canonical text, independent of raw JSON and MySQL BinaryJSON.
use crate::pg_json::{PgJson, PgJsonError};
use crate::pg_numeric::{PgNumeric, PgNumericError};
use std::{collections::HashMap, fmt, str::FromStr};

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PgJsonbError {
    #[error("invalid input syntax for type json")]
    InvalidSyntax,
    #[error("invalid byte sequence for encoding UTF8")]
    InvalidEncoding,
    #[error("unsupported Unicode escape sequence")]
    UntranslatableCharacter,
    #[error("{0}")]
    Numeric(PgNumericError),
}
impl PgJsonbError {
    pub fn sqlstate(self) -> &'static str {
        match self {
            Self::InvalidSyntax => "22P02",
            Self::InvalidEncoding => "22021",
            Self::UntranslatableCharacter => "22P05",
            Self::Numeric(error) => error.sqlstate(),
        }
    }
}
impl From<PgJsonError> for PgJsonbError {
    fn from(error: PgJsonError) -> Self {
        match error {
            PgJsonError::InvalidSyntax => Self::InvalidSyntax,
            PgJsonError::InvalidEncoding => Self::InvalidEncoding,
        }
    }
}

/// Validated canonical spelling. Equality compares that spelling, including
/// numeric display scale; this is not the SQL jsonb equality operator.
/// SQL NULL belongs to the outer value container, never the JSON null token.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct PgJsonb(String);
impl PgJsonb {
    pub const OID: u32 = 3802;

    /// Text input only; binary protocol framing belongs to the protocol codec.
    pub fn from_bytes(input: &[u8]) -> Result<Self, PgJsonbError> {
        let validated = PgJson::from_bytes(input)?;
        normalize(validated.as_str()).map(Self)
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}
impl FromStr for PgJsonb {
    type Err = PgJsonbError;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::from_bytes(input.as_bytes())
    }
}
impl fmt::Display for PgJsonb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// Children are arena indices so parsing, formatting AND dropping deeply nested
// documents never recurse on the Rust stack. Discarded duplicates are still
// decoded and validated before their parent can replace them.
enum Node {
    Scalar(String),
    Array(Vec<usize>),
    Object(HashMap<String, usize>),
}
struct Frame {
    node: Node,
    key: Option<String>,
}
impl Frame {
    fn insert(&mut self, child: usize) {
        match &mut self.node {
            Node::Array(children) => children.push(child),
            Node::Object(members) => {
                members.insert(self.key.take().expect("validated object key"), child);
            }
            Node::Scalar(_) => unreachable!("container frame"),
        }
    }
}

/// The shared JSON lexer checks encoding and complete lexical/structural syntax
/// first. This second pass decodes strings and numeric tokens without using a
/// serde_json Value tree, floats, or lexicographic map ordering.
fn normalize(input: &str) -> Result<String, PgJsonbError> {
    let bytes = input.as_bytes();
    let mut pos = 0;
    let mut arena = Vec::new();
    let mut frames: Vec<Frame> = Vec::new();
    let mut root = 0;
    while pos < bytes.len() {
        let node = match bytes[pos] {
            b' ' | b'\n' | b'\r' | b'\t' | b',' | b':' => {
                pos += 1;
                continue;
            }
            b'{' | b'[' => {
                let node = if bytes[pos] == b'{' {
                    Node::Object(HashMap::new())
                } else {
                    Node::Array(Vec::new())
                };
                frames.push(Frame { node, key: None });
                pos += 1;
                continue;
            }
            b'}' | b']' => {
                pos += 1;
                frames.pop().expect("validated closing delimiter").node
            }
            b'"' => {
                let decoded = decode_string(input, &mut pos)?;
                if let Some(frame) = frames.last_mut() {
                    if matches!(frame.node, Node::Object(_)) && frame.key.is_none() {
                        frame.key = Some(decoded);
                        continue;
                    }
                }
                Node::Scalar(quote(&decoded))
            }
            _ => {
                let start = pos;
                while pos < bytes.len()
                    && !matches!(
                        bytes[pos],
                        b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t'
                    )
                {
                    pos += 1;
                }
                let token = &input[start..pos];
                let text = match token {
                    "null" | "true" | "false" => token.to_owned(),
                    _ => token
                        .parse::<PgNumeric>()
                        .map_err(PgJsonbError::Numeric)?
                        .to_string(),
                };
                Node::Scalar(text)
            }
        };
        let id = arena.len();
        arena.push(node);
        if let Some(parent) = frames.last_mut() {
            parent.insert(id);
        } else {
            root = id;
        }
    }
    Ok(render(&arena, root))
}

fn decode_string(input: &str, pos: &mut usize) -> Result<String, PgJsonbError> {
    let bytes = input.as_bytes();
    let start = *pos;
    *pos += 1;
    while bytes[*pos] != b'"' {
        if bytes[*pos] == b'\\' {
            *pos += 1;
        }
        *pos += 1;
    }
    *pos += 1;
    // Lexical validity is already established, so string decoding can fail only
    // for unpaired surrogates. serde is used for strings alone, never numbers.
    let decoded: String =
        serde_json::from_str(&input[start..*pos]).map_err(|_| PgJsonbError::InvalidSyntax)?;
    if decoded.contains('\0') {
        return Err(PgJsonbError::UntranslatableCharacter);
    }
    Ok(decoded)
}
fn quote(input: &str) -> String {
    serde_json::to_string(input).expect("serializing a UTF-8 string cannot fail")
}

fn render(arena: &[Node], root: usize) -> String {
    enum Action<'a> {
        Value(usize),
        Text(&'a str),
        Key(&'a str),
    }
    let mut output = String::new();
    let mut actions = vec![Action::Value(root)];
    while let Some(action) = actions.pop() {
        match action {
            Action::Text(text) => output.push_str(text),
            Action::Key(key) => output.push_str(&quote(key)),
            Action::Value(id) => match &arena[id] {
                Node::Scalar(text) => output.push_str(text),
                Node::Array(children) => {
                    output.push('[');
                    actions.push(Action::Text("]"));
                    for (index, child) in children.iter().enumerate().rev() {
                        actions.push(Action::Value(*child));
                        if index != 0 {
                            actions.push(Action::Text(", "));
                        }
                    }
                }
                Node::Object(members) => {
                    let mut members: Vec<_> = members.iter().collect();
                    members.sort_unstable_by(|(a, _), (b, _)| {
                        a.len()
                            .cmp(&b.len())
                            .then_with(|| a.as_bytes().cmp(b.as_bytes()))
                    });
                    output.push('{');
                    actions.push(Action::Text("}"));
                    for (index, (key, child)) in members.into_iter().enumerate().rev() {
                        actions.push(Action::Value(*child));
                        actions.push(Action::Text(": "));
                        actions.push(Action::Key(key));
                        if index != 0 {
                            actions.push(Action::Text(", "));
                        }
                    }
                }
            },
        }
    }
    output
}
