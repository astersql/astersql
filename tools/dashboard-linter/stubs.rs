// Copyright 2026 AsterSQL.

//! Dashboard 字段的本地 `encoding/json.Unmarshal` 兼容层。
//!
//! 模块内置最小 JSON 解析器，只把 linter 关心的字段映射到 `BasicDashboard`，
//! 同时保留 Go 在字段名匹配、空值、类型错误和无效 Unicode 处理上的关键语义。

use super::{BasicDashboard, GridPos, Panel};

#[derive(Debug, Clone)]
/// 暂存 JSON 语法树；数字保留原始文本，避免提前转换改变整数校验结果。
enum Value {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

/// 解析 dashboard JSON，并按 Go `json.Unmarshal` 的规则填充最小字段集合。
pub fn unmarshal_dashboard(content: &[u8]) -> Result<BasicDashboard, String> {
    // Go 会把 JSON 字符串中的无效 UTF-8 替换为 U+FFFD，而不是直接拒绝整份输入。
    let text = String::from_utf8_lossy(content);
    let mut parser = Parser::new(&text);
    let value = parser.parse_document()?;
    match value {
        Value::Null => Ok(BasicDashboard::default()),
        Value::Object(fields) => dashboard_from_fields(&fields),
        other => Err(type_error("basicDashboard", &other)),
    }
}

fn dashboard_from_fields(fields: &[(String, Value)]) -> Result<BasicDashboard, String> {
    let mut board = BasicDashboard::default();
    for (name, value) in fields {
        if field_matches(name, "panels") {
            board.panels = panels_from_value(value)?;
        }
    }
    Ok(board)
}

fn panels_from_value(value: &Value) -> Result<Vec<Panel>, String> {
    match value {
        Value::Null => Ok(Vec::new()),
        Value::Array(values) => values.iter().map(panel_from_value).collect(),
        other => Err(type_error("[]main.panel", other)),
    }
}

fn panel_from_value(value: &Value) -> Result<Panel, String> {
    let Value::Object(fields) = value else {
        return if matches!(value, Value::Null) {
            Ok(Panel::default())
        } else {
            Err(type_error("main.panel", value))
        };
    };
    let mut panel = Panel::default();
    for (name, value) in fields {
        if field_matches(name, "id") {
            if !matches!(value, Value::Null) {
                panel.id = int_from_value(value)?;
            }
        } else if field_matches(name, "panels") {
            panel.panels = panels_from_value(value)?;
        } else if field_matches(name, "type") {
            assign_string(&mut panel.panel_type, value)?;
        } else if field_matches(name, "title") {
            assign_string(&mut panel.title, value)?;
        } else if field_matches(name, "collapsed") {
            if !matches!(value, Value::Null) {
                panel.collapsed = bool_from_value(value)?;
            }
        } else if field_matches(name, "datasource") {
            assign_string(&mut panel.datasource, value)?;
        } else if field_matches(name, "gridPos") && !matches!(value, Value::Null) {
            panel.grid_pos = grid_pos_from_value(value)?;
        }
    }
    Ok(panel)
}

fn grid_pos_from_value(value: &Value) -> Result<GridPos, String> {
    let Value::Object(fields) = value else {
        return Err(type_error("struct { H int `json:\"h\"` }", value));
    };
    let mut grid_pos = GridPos::default();
    for (name, value) in fields {
        if field_matches(name, "h") && !matches!(value, Value::Null) {
            grid_pos.h = int_from_value(value)?;
        }
    }
    Ok(grid_pos)
}

fn int_from_value(value: &Value) -> Result<i64, String> {
    // Go 的 int 字段不接受小数或指数形式，并在超出目标整数范围时报类型错误。
    match value {
        Value::Number(raw) if !raw.contains(['.', 'e', 'E']) => {
            raw.parse::<i64>().map_err(|_| type_error("int", value))
        }
        _ => Err(type_error("int", value)),
    }
}

fn assign_string(target: &mut String, value: &Value) -> Result<(), String> {
    match value {
        Value::Null => Ok(()),
        Value::String(value) => {
            target.clone_from(value);
            Ok(())
        }
        other => Err(type_error("string", other)),
    }
}

fn bool_from_value(value: &Value) -> Result<bool, String> {
    match value {
        Value::Bool(value) => Ok(*value),
        other => Err(type_error("bool", other)),
    }
}

fn field_matches(actual: &str, tagged: &str) -> bool {
    // encoding/json 优先精确匹配标签；折叠匹配还包含 Unicode simple-fold
    // 中能折叠到 ASCII 的 LONG S 与 KELVIN SIGN。
    actual == tagged
        || (actual.chars().count() == tagged.chars().count()
            && actual
                .chars()
                .zip(tagged.chars())
                .all(|(left, right)| match (left, right) {
                    ('ſ', 's' | 'S') | ('K', 'k' | 'K') => true,
                    _ => left.eq_ignore_ascii_case(&right),
                }))
}

/// 生成与 Go 反序列化错误相同形状的类型不匹配信息。
fn type_error(target: &str, value: &Value) -> String {
    let source = match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    };
    format!("json: cannot unmarshal {source} into Go value of type {target}")
}

/// 面向当前兼容需求的递归下降 JSON 解析器。
struct Parser<'a> {
    source: &'a str,
    /// 当前 UTF-8 字节偏移；切片和错误定位都以此为准。
    offset: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self { source, offset: 0 }
    }

    fn parse_document(&mut self) -> Result<Value, String> {
        // 顶层值之后只能出现 JSON 空白，防止悄悄接受拼接的第二个值。
        self.skip_space();
        let value = self.parse_value()?;
        self.skip_space();
        if self.offset == self.source.len() {
            Ok(value)
        } else {
            Err("invalid character after top-level value".to_string())
        }
    }

    fn parse_value(&mut self) -> Result<Value, String> {
        self.skip_space();
        match self.peek() {
            Some('n') => self.literal("null", Value::Null),
            Some('t') => self.literal("true", Value::Bool(true)),
            Some('f') => self.literal("false", Value::Bool(false)),
            Some('"') => self.parse_string().map(Value::String),
            Some('[') => self.parse_array(),
            Some('{') => self.parse_object(),
            Some('-' | '0'..='9') => self.parse_number(),
            Some(ch) => Err(format!(
                "invalid character '{ch}' looking for beginning of value"
            )),
            None => Err("unexpected end of JSON input".to_string()),
        }
    }

    fn parse_array(&mut self) -> Result<Value, String> {
        self.expect('[')?;
        self.skip_space();
        let mut values = Vec::new();
        if self.consume(']') {
            return Ok(Value::Array(values));
        }
        loop {
            values.push(self.parse_value()?);
            self.skip_space();
            if self.consume(']') {
                return Ok(Value::Array(values));
            }
            self.expect(',')?;
        }
    }

    fn parse_object(&mut self) -> Result<Value, String> {
        self.expect('{')?;
        self.skip_space();
        let mut fields = Vec::new();
        if self.consume('}') {
            return Ok(Value::Object(fields));
        }
        loop {
            self.skip_space();
            if self.peek() != Some('"') {
                return Err("object key must be a string".to_string());
            }
            let name = self.parse_string()?;
            self.skip_space();
            self.expect(':')?;
            fields.push((name, self.parse_value()?));
            self.skip_space();
            if self.consume('}') {
                return Ok(Value::Object(fields));
            }
            self.expect(',')?;
        }
    }

    fn parse_number(&mut self) -> Result<Value, String> {
        // 这里只验证 JSON 数字语法并保留词法文本，目标字段的整数约束稍后再检查。
        let start = self.offset;
        self.consume('-');
        match self.peek() {
            Some('0') => {
                self.bump();
                if matches!(self.peek(), Some('0'..='9')) {
                    return Err("invalid number".to_string());
                }
            }
            Some('1'..='9') => {
                self.bump();
                while matches!(self.peek(), Some('0'..='9')) {
                    self.bump();
                }
            }
            _ => return Err("invalid number".to_string()),
        }
        if self.consume('.') {
            if !matches!(self.peek(), Some('0'..='9')) {
                return Err("invalid number".to_string());
            }
            while matches!(self.peek(), Some('0'..='9')) {
                self.bump();
            }
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            self.bump();
            if matches!(self.peek(), Some('+' | '-')) {
                self.bump();
            }
            if !matches!(self.peek(), Some('0'..='9')) {
                return Err("invalid number".to_string());
            }
            while matches!(self.peek(), Some('0'..='9')) {
                self.bump();
            }
        }
        Ok(Value::Number(self.source[start..self.offset].to_string()))
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.expect('"')?;
        let mut output = String::new();
        loop {
            match self.bump() {
                None => return Err("unexpected end of JSON input".to_string()),
                Some('"') => return Ok(output),
                Some('\\') => self.parse_escape(&mut output)?,
                Some(ch) if ch <= '\u{001f}' => {
                    return Err("invalid control character in string".to_string());
                }
                Some(ch) => output.push(ch),
            }
        }
    }

    fn parse_escape(&mut self, output: &mut String) -> Result<(), String> {
        match self.bump() {
            Some('"') => output.push('"'),
            Some('\\') => output.push('\\'),
            Some('/') => output.push('/'),
            Some('b') => output.push('\u{0008}'),
            Some('f') => output.push('\u{000c}'),
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some('t') => output.push('\t'),
            Some('u') => {
                let first = self.parse_hex_quad()?;
                // 合法代理项对合成一个标量；孤立或错配代理项按 Go 行为替换为 U+FFFD。
                let scalar = if (0xD800..=0xDBFF).contains(&first)
                    && self.source[self.offset..].starts_with("\\u")
                {
                    let saved = self.offset;
                    self.offset += 2;
                    let second = self.parse_hex_quad()?;
                    if (0xDC00..=0xDFFF).contains(&second) {
                        0x10000 + (((first - 0xD800) as u32) << 10) + (second - 0xDC00) as u32
                    } else {
                        self.offset = saved;
                        0xFFFD
                    }
                } else if (0xD800..=0xDFFF).contains(&first) {
                    0xFFFD
                } else {
                    first as u32
                };
                output.push(char::from_u32(scalar).unwrap_or('\u{FFFD}'));
            }
            Some(other) => return Err(format!("invalid character '{other}' in string escape")),
            None => return Err("unexpected end of JSON input".to_string()),
        }
        Ok(())
    }

    fn parse_hex_quad(&mut self) -> Result<u16, String> {
        let mut value = 0_u16;
        for _ in 0..4 {
            let digit = self
                .bump()
                .and_then(|ch| ch.to_digit(16))
                .ok_or_else(|| "invalid unicode escape".to_string())?;
            value = (value << 4) | digit as u16;
        }
        Ok(value)
    }

    fn literal(&mut self, literal: &str, value: Value) -> Result<Value, String> {
        if self.source[self.offset..].starts_with(literal) {
            self.offset += literal.len();
            Ok(value)
        } else {
            Err("invalid JSON literal".to_string())
        }
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\r' | '\n')) {
            self.bump();
        }
    }

    fn expect(&mut self, expected: char) -> Result<(), String> {
        if self.consume(expected) {
            Ok(())
        } else {
            Err(format!("expected '{expected}'"))
        }
    }

    fn consume(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<char> {
        self.source[self.offset..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.offset += ch.len_utf8();
        Some(ch)
    }
}
