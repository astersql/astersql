// Copyright 2026 AsterSQL.

// JSON 函数与路径表达式的轻量桩实现。
//
// 定义 binary JSON 类型码、修改模式与 `JSONPathExpression` 解析，
// 并通过 `include!` 引入完整 JSON 函数体；供独立子 crate 在无完整
// types 依赖时编译路径相关逻辑。

#![allow(non_snake_case, non_upper_case_globals, dead_code, private_interfaces)]

/// Binary JSON 类型码（与 MySQL JSON binary 协议一致）。
pub type JSONTypeCode = u8;
/// 对象。
pub const JSONTypeCodeObject: JSONTypeCode = 0x01;
/// 数组。
pub const JSONTypeCodeArray: JSONTypeCode = 0x03;
/// 字面量（null/true/false）。
pub const JSONTypeCodeLiteral: JSONTypeCode = 0x04;
/// 有符号 64 位整数。
pub const JSONTypeCodeInt64: JSONTypeCode = 0x09;
/// 无符号 64 位整数。
pub const JSONTypeCodeUint64: JSONTypeCode = 0x0a;
/// 双精度浮点。
pub const JSONTypeCodeFloat64: JSONTypeCode = 0x0b;
/// 字符串。
pub const JSONTypeCodeString: JSONTypeCode = 0x0c;
/// 不透明二进制（opaque）。
pub const JSONTypeCodeOpaque: JSONTypeCode = 0x0d;
/// DATE。
pub const JSONTypeCodeDate: JSONTypeCode = 0x0e;
/// DATETIME。
pub const JSONTypeCodeDatetime: JSONTypeCode = 0x0f;
/// TIMESTAMP。
pub const JSONTypeCodeTimestamp: JSONTypeCode = 0x10;
/// DURATION / TIME。
pub const JSONTypeCodeDuration: JSONTypeCode = 0x11;

/// 字面量：null。
pub const JSONLiteralNil: u8 = 0x00;
/// 字面量：true。
pub const JSONLiteralTrue: u8 = 0x01;
/// 字面量：false。
pub const JSONLiteralFalse: u8 = 0x02;

/// JSON_SET / INSERT / REPLACE 等修改语义。
pub type JSONModifyType = u8;
/// 仅当路径不存在时插入。
pub const JSONModifyInsert: JSONModifyType = 0x01;
/// 仅当路径存在时替换。
pub const JSONModifyReplace: JSONModifyType = 0x02;
/// 存在则替换，否则插入。
pub const JSONModifySet: JSONModifyType = 0x03;

/// JSON_CONTAINS_PATH 模式：任一路径存在即可。
pub const JSONContainsPathOne: &str = "one";
/// JSON_CONTAINS_PATH 模式：全部路径都必须存在。
pub const JSONContainsPathAll: &str = "all";

/// Binary JSON 值：类型码 + 编码字节。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BinaryJSON {
    pub TypeCode: JSONTypeCode,
    pub Value: Vec<u8>,
}

/// 数组路径选择：`*`、单下标或 `start to end` 区间。
#[derive(Clone, Debug, Eq, PartialEq)]
enum JsonPathArraySelection {
    Asterisk,
    Index(isize),
    Range(isize, isize),
}

/// JSON Path 的一段：对象键、数组选择或 `**` 递归通配。
#[derive(Clone, Debug, Eq, PartialEq)]
enum JsonPathLeg {
    Key(String),
    Array(JsonPathArraySelection),
    DoubleAsterisk,
}

/// 已解析的 JSON Path（以 `$` 为根，后跟若干 leg）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct JSONPathExpression {
    legs: Vec<JsonPathLeg>,
}

impl JSONPathExpression {
    fn root() -> Self {
        Self { legs: Vec::new() }
    }

    fn from_legs(legs: Vec<JsonPathLeg>) -> Self {
        Self { legs }
    }

    fn push_leg(&self, leg: JsonPathLeg) -> Self {
        let mut legs = self.legs.clone();
        legs.push(leg);
        Self { legs }
    }

    fn push_array(&self, index: isize) -> Self {
        self.push_leg(JsonPathLeg::Array(JsonPathArraySelection::Index(index)))
    }

    fn push_key(&self, key: String) -> Self {
        self.push_leg(JsonPathLeg::Key(key))
    }

    /// 路径是否可能匹配多个值（含 `*`、区间或 `**`）。
    pub fn CouldMatchMultipleValues(&self) -> bool {
        self.legs.iter().any(|leg| {
            matches!(leg, JsonPathLeg::DoubleAsterisk)
                || matches!(leg, JsonPathLeg::Key(key) if key == "*")
                || matches!(
                    leg,
                    JsonPathLeg::Array(
                        JsonPathArraySelection::Asterisk | JsonPathArraySelection::Range(_, _)
                    )
                )
        })
    }
}

impl std::fmt::Display for JSONPathExpression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("$")?;
        for leg in &self.legs {
            match leg {
                JsonPathLeg::Key(key) if key == "*" || is_ecmascript_identifier(key) => {
                    write!(f, ".{key}")?;
                }
                JsonPathLeg::Key(key) => {
                    write!(f, ".{}", serde_json::to_string(key).unwrap())?;
                }
                JsonPathLeg::Array(JsonPathArraySelection::Asterisk) => f.write_str("[*]")?,
                JsonPathLeg::Array(JsonPathArraySelection::Index(index)) if *index < 0 => {
                    write!(f, "[{}]", display_index(*index))?;
                }
                JsonPathLeg::Array(JsonPathArraySelection::Index(index)) => write!(f, "[{index}]")?,
                JsonPathLeg::Array(JsonPathArraySelection::Range(start, end)) => {
                    write!(f, "[{} to {}]", display_index(*start), display_index(*end))?;
                }
                JsonPathLeg::DoubleAsterisk => f.write_str("**")?,
            }
        }
        Ok(())
    }
}

/// 将下标格式化为数字或 `last` 相对形式。
fn display_index(index: isize) -> String {
    if index >= 0 {
        index.to_string()
    } else {
        format!("last-{}", -index - 1)
    }
}

#[derive(Clone)]
struct JsonPathStream {
    input: Vec<char>,
    position: usize,
}

impl JsonPathStream {
    fn new(input: &str) -> Self {
        Self {
            input: input.chars().collect(),
            position: 0,
        }
    }

    fn exhausted(&self) -> bool {
        self.position >= self.input.len()
    }

    fn peek(&self) -> Option<char> {
        self.input.get(self.position).copied()
    }

    fn read(&mut self) -> Option<char> {
        let value = self.peek()?;
        self.position += 1;
        Some(value)
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.position += 1;
        }
    }

    fn try_parse_number(&mut self) -> Option<isize> {
        let start = self.position;
        while self.peek().is_some_and(|value| value.is_ascii_digit()) {
            self.position += 1;
        }
        if start == self.position {
            return None;
        }
        let index = self.input[start..self.position]
            .iter()
            .collect::<String>()
            .parse::<u64>()
            .ok()?;
        (index <= u32::MAX as u64).then_some(index as isize)
    }

    fn try_parse_index(&mut self) -> Option<isize> {
        let start = self.position;
        if let Some(index) = self.try_parse_number() {
            return Some(index);
        }
        let last = ['l', 'a', 's', 't'];
        if self.input.get(self.position..self.position + last.len()) != Some(&last) {
            return None;
        }
        self.position += last.len();
        self.skip_whitespace();
        if self.peek() != Some('-') {
            return Some(-1);
        }
        self.position += 1;
        self.skip_whitespace();
        match self.try_parse_number() {
            Some(offset) => Some(-1 - offset),
            None => {
                self.position = start;
                None
            }
        }
    }
}

/// 解析以 `$` 开头的 JSON Path 表达式。
pub fn ParseJSONPathExpr(text: &str) -> Result<JSONPathExpression, JsonBinaryError> {
    let mut stream = JsonPathStream::new(text);
    stream.skip_whitespace();
    if stream.read() != Some('$') {
        return Err(JsonBinaryError::new("JSON path must start with '$'"));
    }
    stream.skip_whitespace();
    let mut legs = Vec::new();
    while !stream.exhausted() {
        match stream.peek() {
            Some('.') => {
                stream.position += 1;
                stream.skip_whitespace();
                let key = if stream.peek() == Some('*') {
                    stream.position += 1;
                    "*".to_owned()
                } else if stream.peek() == Some('"') {
                    let start = stream.position;
                    stream.position += 1;
                    let mut escaped = false;
                    while let Some(character) = stream.read() {
                        if character == '"' && !escaped {
                            break;
                        }
                        escaped = character == '\\' && !escaped;
                        if character != '\\' {
                            escaped = false;
                        }
                    }
                    if stream.input.get(stream.position.saturating_sub(1)) != Some(&'"') {
                        return Err(JsonBinaryError::new("unterminated JSON path key"));
                    }
                    let quoted = stream.input[start..stream.position]
                        .iter()
                        .collect::<String>();
                    serde_json::from_str(&quoted)
                        .map_err(|error| JsonBinaryError::new(error.to_string()))?
                } else {
                    let start = stream.position;
                    while stream.peek().is_some_and(|value| {
                        !value.is_whitespace() && !matches!(value, '.' | '[' | '*')
                    }) {
                        stream.position += 1;
                    }
                    let key = stream.input[start..stream.position]
                        .iter()
                        .collect::<String>();
                    if !is_ecmascript_identifier(&key) {
                        return Err(JsonBinaryError::new("invalid JSON path key"));
                    }
                    key
                };
                legs.push(JsonPathLeg::Key(key));
            }
            Some('[') => {
                stream.position += 1;
                stream.skip_whitespace();
                let selection = if stream.peek() == Some('*') {
                    stream.position += 1;
                    JsonPathArraySelection::Asterisk
                } else {
                    let start = stream
                        .try_parse_index()
                        .ok_or_else(|| JsonBinaryError::new("invalid JSON path array index"))?;
                    let mut selection = JsonPathArraySelection::Index(start);
                    if stream.peek().is_some_and(char::is_whitespace) {
                        stream.skip_whitespace();
                        if stream.input.get(stream.position..stream.position + 2)
                            == Some(&['t', 'o'])
                        {
                            stream.position += 2;
                            if !stream.peek().is_some_and(char::is_whitespace) {
                                return Err(JsonBinaryError::new("invalid JSON path range"));
                            }
                            stream.skip_whitespace();
                            let end = stream.try_parse_index().ok_or_else(|| {
                                JsonBinaryError::new("invalid JSON path range end")
                            })?;
                            if (start >= 0 && end >= 0 || start < 0 && end < 0) && start > end {
                                return Err(JsonBinaryError::new("invalid JSON path range order"));
                            }
                            selection = JsonPathArraySelection::Range(start, end);
                        }
                    }
                    selection
                };
                stream.skip_whitespace();
                if stream.read() != Some(']') {
                    return Err(JsonBinaryError::new("unterminated JSON path array leg"));
                }
                legs.push(JsonPathLeg::Array(selection));
            }
            Some('*') => {
                stream.position += 1;
                if stream.read() != Some('*') || stream.exhausted() || stream.peek() == Some('*') {
                    return Err(JsonBinaryError::new("invalid JSON path wildcard"));
                }
                legs.push(JsonPathLeg::DoubleAsterisk);
            }
            _ => return Err(JsonBinaryError::new("invalid JSON path leg")),
        }
        stream.skip_whitespace();
    }
    if matches!(legs.last(), Some(JsonPathLeg::DoubleAsterisk)) {
        return Err(JsonBinaryError::new(
            "JSON path cannot end with double asterisk",
        ));
    }
    Ok(JSONPathExpression { legs })
}

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../json_binary_functions.rs"
));

/// 从 JSON 文本解析并编码为 BinaryJSON。
pub fn ParseBinaryJSONFromString(text: &str) -> Result<BinaryJSON, JsonBinaryError> {
    let value =
        serde_json::from_str(text).map_err(|error| JsonBinaryError::new(error.to_string()))?;
    CreateBinaryJSON(value)
}
