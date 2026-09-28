// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 示例任务与子任务的 meta 协议。
//
// 用极简 JSON 字符串承载 `subtask_count` / `message`，便于演示
// Marshal/Unmarshal 在调度与执行之间的往返，而非生产级 protobuf。

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 任务级 meta：声明每个 step 要生成的子任务个数。
pub struct taskMeta {
    /// 每个 step 的子任务数量。
    pub SubtaskCount: i64,
}

#[derive(Debug)]
enum JsonValue {
    Null,
    Bool,
    Number(String),
    String(String),
    Array(Vec<JsonValue>),
    Object(Vec<(String, JsonValue)>),
}

struct JsonParser<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self { input, pos: 0 }
    }

    fn parse_object(mut self) -> Result<Vec<(String, JsonValue)>, String> {
        let value = self.parse_value()?;
        self.skip_whitespace();
        if self.pos != self.input.len() {
            return Err("invalid JSON".into());
        }
        match value {
            JsonValue::Object(fields) => Ok(fields),
            // Go encoding/json treats JSON null as having no effect when the
            // destination is a non-pointer struct, leaving all fields zeroed.
            JsonValue::Null => Ok(Vec::new()),
            _ => Err("expected JSON object".into()),
        }
    }

    fn parse_value(&mut self) -> Result<JsonValue, String> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'{') => self.parse_object_value(),
            Some(b'[') => self.parse_array_value(),
            Some(b'"') => Ok(JsonValue::String(self.parse_string()?)),
            Some(b't') => self.parse_literal(b"true", JsonValue::Bool),
            Some(b'f') => self.parse_literal(b"false", JsonValue::Bool),
            Some(b'n') => self.parse_literal(b"null", JsonValue::Null),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            _ => Err("invalid JSON value".into()),
        }
    }

    fn parse_object_value(&mut self) -> Result<JsonValue, String> {
        self.expect(b'{')?;
        let mut fields = Vec::new();
        self.skip_whitespace();
        if self.consume(b'}') {
            return Ok(JsonValue::Object(fields));
        }
        loop {
            self.skip_whitespace();
            let key = self.parse_string()?;
            self.skip_whitespace();
            self.expect(b':')?;
            fields.push((key, self.parse_value()?));
            self.skip_whitespace();
            if self.consume(b'}') {
                return Ok(JsonValue::Object(fields));
            }
            self.expect(b',')?;
        }
    }

    fn parse_array_value(&mut self) -> Result<JsonValue, String> {
        self.expect(b'[')?;
        let mut values = Vec::new();
        self.skip_whitespace();
        if self.consume(b']') {
            return Ok(JsonValue::Array(values));
        }
        loop {
            values.push(self.parse_value()?);
            self.skip_whitespace();
            if self.consume(b']') {
                return Ok(JsonValue::Array(values));
            }
            self.expect(b',')?;
        }
    }

    fn parse_literal(&mut self, literal: &[u8], value: JsonValue) -> Result<JsonValue, String> {
        if self.input.get(self.pos..self.pos + literal.len()) == Some(literal) {
            self.pos += literal.len();
            Ok(value)
        } else {
            Err("invalid JSON literal".into())
        }
    }

    fn parse_number(&mut self) -> Result<JsonValue, String> {
        let start = self.pos;
        self.consume(b'-');
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
            }
            Some(b'1'..=b'9') => {
                self.pos += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err("invalid JSON number".into()),
        }
        if self.consume(b'.') {
            let fraction_start = self.pos;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
            if self.pos == fraction_start {
                return Err("invalid JSON number".into());
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            self.consume(b'+');
            self.consume(b'-');
            let exponent_start = self.pos;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
            if self.pos == exponent_start {
                return Err("invalid JSON number".into());
            }
        }
        let number = String::from_utf8(self.input[start..self.pos].to_vec())
            .map_err(|_| "invalid JSON number".to_string())?;
        Ok(JsonValue::Number(number))
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut bytes = Vec::new();
        loop {
            match self.next() {
                Some(b'"') => return Ok(String::from_utf8_lossy(&bytes).into_owned()),
                Some(b'\\') => self.parse_escape(&mut bytes)?,
                Some(byte @ 0..=0x1f) => {
                    let _ = byte;
                    return Err("unescaped control character in JSON string".into());
                }
                Some(byte) => bytes.push(byte),
                None => return Err("unterminated JSON string".into()),
            }
        }
    }

    fn parse_escape(&mut self, output: &mut Vec<u8>) -> Result<(), String> {
        match self.next() {
            Some(b'"') => output.push(b'"'),
            Some(b'\\') => output.push(b'\\'),
            Some(b'/') => output.push(b'/'),
            Some(b'b') => output.push(8),
            Some(b'f') => output.push(12),
            Some(b'n') => output.push(b'\n'),
            Some(b'r') => output.push(b'\r'),
            Some(b't') => output.push(b'\t'),
            Some(b'u') => {
                let first = self.parse_hex_quad()?;
                let codepoint = if (0xd800..=0xdbff).contains(&first) {
                    if self.input.get(self.pos..self.pos + 2) == Some(b"\\u") {
                        let saved_pos = self.pos;
                        self.pos += 2;
                        match self.parse_hex_quad() {
                            Ok(second) if (0xdc00..=0xdfff).contains(&second) => {
                                0x1_0000 + ((first - 0xd800) << 10) + (second - 0xdc00)
                            }
                            _ => {
                                self.pos = saved_pos;
                                0xfffd
                            }
                        }
                    } else {
                        0xfffd
                    }
                } else if (0xdc00..=0xdfff).contains(&first) {
                    0xfffd
                } else {
                    first
                };
                let character = char::from_u32(codepoint)
                    .ok_or_else(|| "invalid Unicode code point".to_string())?;
                let mut encoded = [0; 4];
                output.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
            }
            _ => return Err("invalid JSON escape".into()),
        }
        Ok(())
    }

    fn parse_hex_quad(&mut self) -> Result<u32, String> {
        let mut value = 0;
        for _ in 0..4 {
            value = (value << 4)
                | self
                    .next()
                    .and_then(|digit| (digit as char).to_digit(16))
                    .ok_or_else(|| "invalid Unicode escape".to_string())?;
        }
        Ok(value)
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), String> {
        if self.next() == Some(expected) {
            Ok(())
        } else {
            Err("invalid JSON".into())
        }
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.pos += 1;
        Some(byte)
    }
}

fn parse_json_object(bytes: &[u8]) -> Result<Vec<(String, JsonValue)>, String> {
    JsonParser::new(bytes).parse_object()
}

fn last_field<'a>(fields: &'a [(String, JsonValue)], name: &str) -> Option<&'a JsonValue> {
    fields
        .iter()
        .rev()
        .find(|(field, _)| field_name_matches(field, name))
        .map(|(_, value)| value)
}

/// Match Go encoding/json's folded struct-field names. Its fold operation is
/// ASCII case-insensitive and also includes Unicode's two ASCII-equivalent
/// simple-fold runes: Kelvin sign (K) and long s (S).
fn field_name_matches(field: &str, name: &str) -> bool {
    fn folded(character: char) -> char {
        match character {
            'A'..='Z' => character.to_ascii_lowercase(),
            '\u{212a}' => 'k',
            '\u{017f}' => 's',
            character => character,
        }
    }

    field.chars().map(folded).eq(name.chars().map(folded))
}

/// taskMeta 的序列化/反序列化。
impl taskMeta {
    /// 编码为 `{"subtask_count":N}` 字节。
    pub fn Marshal(&self) -> Vec<u8> {
        format!("{{\"subtask_count\":{}}}", self.SubtaskCount).into_bytes()
    }

    /// 使用与 Go `encoding/json` 对象字段规则一致的 JSON 解析。
    pub fn Unmarshal(bytes: &[u8]) -> Result<Self, String> {
        let fields = parse_json_object(bytes)?;
        let subtask_count = match last_field(&fields, "subtask_count") {
            None | Some(JsonValue::Null) => 0,
            Some(JsonValue::Number(value)) => value
                .parse::<i64>()
                .map_err(|_| "invalid subtask_count".to_string())?,
            Some(_) => return Err("invalid subtask_count".into()),
        };
        Ok(Self {
            SubtaskCount: subtask_count,
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 子任务级 meta：携带可读 Message。
pub struct subtaskMeta {
    /// 子任务说明文本，执行时 Unmarshal 校验。
    pub Message: String,
}

/// 转义规则与 Go `encoding/json` 的字符串输出保持一致。
fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0c}' => escaped.push_str("\\f"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '<' => escaped.push_str("\\u003c"),
            '>' => escaped.push_str("\\u003e"),
            '&' => escaped.push_str("\\u0026"),
            '\u{2028}' => escaped.push_str("\\u2028"),
            '\u{2029}' => escaped.push_str("\\u2029"),
            character if character <= '\u{1f}' => {
                escaped.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => escaped.push(character),
        }
    }
    escaped
}

/// subtaskMeta 的序列化/反序列化。
impl subtaskMeta {
    /// 编码为 `{"message":"..."}` 字节。
    pub fn Marshal(&self) -> Vec<u8> {
        format!("{{\"message\":\"{}\"}}", escape(&self.Message)).into_bytes()
    }

    /// 使用与 Go `encoding/json` 字符串字段规则一致的 JSON 解析。
    pub fn Unmarshal(bytes: &[u8]) -> Result<Self, String> {
        let fields = parse_json_object(bytes)?;
        let message = match last_field(&fields, "message") {
            None | Some(JsonValue::Null) => String::new(),
            Some(JsonValue::String(value)) => value.clone(),
            Some(_) => return Err("invalid message".into()),
        };
        Ok(Self { Message: message })
    }
}
