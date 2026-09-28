// Copyright 2026 AsterSQL.
//! Local stand-ins for `database/sql` and `encoding/json`
//! (arm64-safe; no kv/domain/kvproto/grpcio).
//!
//! Production algorithms live in `testcase.rs` / `run.rs` and call these
//! boundaries the same way Go calls `database/sql` and `encoding/json`.

// 本文件对应 `tests/llmtest/testcase/stubs.rs`，本次任务只补中文解释，不改行为。
// 本文件提供轻量测试桩，而不是完整生产实现。
// 桩只覆盖当前测试真正触达的接口形状。
// 关键阅读点是全局开关、记录点和资源回收。
// 未覆盖的真实能力不会被假装支持。
// 中文注释会帮助区分桩职责与真实边界。
// 这类文件最怕隐式状态污染，因此会强调 reset 和 cleanup。
use std::fmt;
use std::sync::{Arc, Mutex};

/// JSON / SQL argument value corresponding to Go `any` / `[]any` elements.
#[derive(Clone, Debug, PartialEq)]
// `AnyValue` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
pub enum AnyValue {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<AnyValue>),
    Object(Vec<(String, AnyValue)>),
}

// 这里实现 `fmt::Display` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl fmt::Display for AnyValue {
    // `fmt` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", json::marshal(self))
    }
}

/// SQL error corresponding to Go `error` from `database/sql`.
#[derive(Debug, Clone, PartialEq, Eq)]
// `SqlError` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
pub struct SqlError {
    pub message: String,
}

// 这里实现 `SqlError` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl SqlError {
    // `new` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

// 这里实现 `fmt::Display` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl fmt::Display for SqlError {
    // `fmt` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

// 这里实现 `std::error::Error` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl std::error::Error for SqlError {}

/// One scripted query outcome for [`Db`].
#[derive(Clone, Debug)]
// `QueryOutcome` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
pub struct QueryOutcome {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    /// If set, surfaced via [`DbRows::err`] after iteration (Go `Rows.Err`).
    pub rows_err: Option<SqlError>,
}

// `QueryHandler` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
type QueryHandler = Arc<dyn Fn(&str, &[AnyValue]) -> Result<QueryOutcome, SqlError> + Send + Sync>;

/// Go `*sql.DB` stand-in.
#[derive(Clone)]
// `Db` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
pub struct Db {
    handler: QueryHandler,
    /// Observable queries for tests (order preserved).
    log: Arc<Mutex<Vec<(String, Vec<AnyValue>)>>>,
}

// 这里实现 `fmt::Debug` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl fmt::Debug for Db {
    // `fmt` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Db").finish_non_exhaustive()
    }
}

// 这里实现 `Db` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl Db {
    /// Build a DB whose every query is answered by `handler`.
    // `with_handler` 负责组织当前阶段的输入、状态或资源。
    // 阅读时重点看它如何约束调用顺序和失败返回。
    // 这里的行为需要尽量贴近 Go 版本。
    pub fn with_handler<F>(handler: F) -> Self
    where
        F: Fn(&str, &[AnyValue]) -> Result<QueryOutcome, SqlError> + Send + Sync + 'static,
    {
        Self {
            handler: Arc::new(handler),
            log: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Fixed successful result for every query.
    // `always_ok` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn always_ok(outcome: QueryOutcome) -> Self {
        Self::with_handler(move |_q, _a| Ok(outcome.clone()))
    }

    /// Fixed error for every query.
    // `always_err` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn always_err(err: SqlError) -> Self {
        Self::with_handler(move |_q, _a| Err(err.clone()))
    }

    /// Go `(*DB).Query(query string, args ...any) (*Rows, error)`.
    // `query` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn query(&self, query: &str, args: &[AnyValue]) -> Result<DbRows, SqlError> {
        if let Ok(mut g) = self.log.lock() {
            g.push((query.to_string(), args.to_vec()));
        }
        let outcome = (self.handler)(query, args)?;
        Ok(DbRows {
            columns: outcome.columns,
            rows: outcome.rows,
            idx: 0,
            rows_err: outcome.rows_err,
            iteration_done: false,
            closed: false,
        })
    }

    /// Test helper: recorded `(query, args)` pairs.
    // `query_log` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn query_log(&self) -> Vec<(String, Vec<AnyValue>)> {
        self.log.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

/// Go `*sql.Rows` stand-in with Close / Err lifecycle.
#[derive(Debug)]
// `DbRows` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
pub struct DbRows {
    columns: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
    idx: usize,
    rows_err: Option<SqlError>,
    iteration_done: bool,
    closed: bool,
}

// 这里实现 `DbRows` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl DbRows {
    /// Go `(*Rows).Columns() ([]string, error)`.
    // `columns` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn columns(&self) -> Result<Vec<String>, SqlError> {
        if self.closed {
            return Err(SqlError::new("sql: rows are closed"));
        }
        Ok(self.columns.clone())
    }

    /// Go `(*Rows).Next() bool`.
    // `next` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn next(&mut self) -> bool {
        if self.closed {
            return false;
        }
        if self.idx < self.rows.len() {
            self.idx += 1;
            true
        } else {
            self.iteration_done = true;
            false
        }
    }

    /// Scan current row as `sql.NullString` values (Go scan loop).
    // `scan_null_strings` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn scan_null_strings(&mut self) -> Result<Vec<Option<String>>, SqlError> {
        if self.closed {
            return Err(SqlError::new("sql: rows are closed"));
        }
        if self.idx == 0 || self.idx > self.rows.len() {
            return Err(SqlError::new("sql: Scan called without Next"));
        }
        let row = &self.rows[self.idx - 1];
        if row.len() != self.columns.len() {
            return Err(SqlError::new(format!(
                "sql: expected {} destination arguments in Scan, not {}",
                row.len(),
                self.columns.len()
            )));
        }
        Ok(row.clone())
    }

    /// Go `(*Rows).Err() error`.
    // `err` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn err(&self) -> Option<SqlError> {
        self.iteration_done.then(|| self.rows_err.clone()).flatten()
    }

    /// Go `(*Rows).Close() error`.
    // `close` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn close(&mut self) -> Result<(), SqlError> {
        self.closed = true;
        Ok(())
    }

    /// Test helper: whether Close completed.
    // `is_closed` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

/// Minimal `encoding/json` surface used by this package.
// 模块 `json` 在这里被显式接线，方便按既定边界编译。
// 阅读这一行时，可以把它看成当前 crate 的依赖入口说明。
// 保持导出关系稳定比改命名更重要。
pub mod json {
    use super::AnyValue;
    use std::collections::BTreeMap;
    use std::fmt::Write as _;

    /// Go `json.Marshal` for [`AnyValue`] (compact, HTML-escaped).
    // `marshal` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn marshal(v: &AnyValue) -> String {
        let mut out = String::new();
        write_value(&mut out, v, 0, false);
        out
    }

    /// Go `json.MarshalIndent(v, "", "  ")`.
    // `marshal_indent` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn marshal_indent(v: &AnyValue) -> String {
        let mut out = String::new();
        write_value(&mut out, v, 0, true);
        out
    }

    /// Go `json.Unmarshal` into [`AnyValue`].
    // `unmarshal` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn unmarshal(bytes: &[u8]) -> Result<AnyValue, String> {
        // encoding/json replaces invalid UTF-8 with U+FFFD while decoding.
        let s = String::from_utf8_lossy(bytes);
        let mut p = Parser {
            chars: s.chars().collect(),
            i: 0,
        };
        let v = p.parse_value()?;
        p.skip_ws();
        if p.i != p.chars.len() {
            return Err("trailing junk after JSON value".to_string());
        }
        Ok(v)
    }

    // `write_value` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn write_value(out: &mut String, v: &AnyValue, indent: usize, pretty: bool) {
        match v {
            AnyValue::Null => out.push_str("null"),
            AnyValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            AnyValue::Number(n) => out.push_str(n),
            AnyValue::String(s) => write_string(out, s),
            AnyValue::Array(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push('[');
                for (idx, item) in items.iter().enumerate() {
                    if pretty {
                        out.push('\n');
                        push_indent(out, indent + 1);
                    }
                    write_value(out, item, indent + 1, pretty);
                    if idx + 1 != items.len() {
                        out.push(',');
                    }
                }
                if pretty {
                    out.push('\n');
                    push_indent(out, indent);
                }
                out.push(']');
            }
            AnyValue::Object(fields) => {
                // Go encoding/json sorts map keys.
                let mut sorted: BTreeMap<&str, &AnyValue> = BTreeMap::new();
                for (k, val) in fields {
                    sorted.insert(k.as_str(), val);
                }
                if sorted.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push('{');
                let len = sorted.len();
                for (idx, (k, val)) in sorted.into_iter().enumerate() {
                    if pretty {
                        out.push('\n');
                        push_indent(out, indent + 1);
                    }
                    write_string(out, k);
                    out.push(':');
                    if pretty {
                        out.push(' ');
                    }
                    write_value(out, val, indent + 1, pretty);
                    if idx + 1 != len {
                        out.push(',');
                    }
                }
                if pretty {
                    out.push('\n');
                    push_indent(out, indent);
                }
                out.push('}');
            }
        }
    }

    // `push_indent` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn push_indent(out: &mut String, n: usize) {
        for _ in 0..n {
            out.push_str("  ");
        }
    }

    // `write_string` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn write_string(out: &mut String, s: &str) {
        out.push('"');
        for ch in s.chars() {
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\u{0008}' => out.push_str("\\b"),
                '\u{000c}' => out.push_str("\\f"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                // Go encoding/json EscapeHTML default.
                '<' => out.push_str("\\u003c"),
                '>' => out.push_str("\\u003e"),
                '&' => out.push_str("\\u0026"),
                '\u{2028}' => out.push_str("\\u2028"),
                '\u{2029}' => out.push_str("\\u2029"),
                c if (c as u32) < 0x20 => {
                    let _ = write!(out, "\\u{:04x}", c as u32);
                }
                c => out.push(c),
            }
        }
        out.push('"');
    }

    // `Parser` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    // 理解它的边界有助于区分测试桩与真实实现。
    struct Parser {
        chars: Vec<char>,
        i: usize,
    }

    // 这里实现 `Parser` 的行为方法和资源回收语义。
    // 阅读这一段时，优先关注进入和离开方法时的状态变化。
    // 很多 parity 断言都会依赖这里保留下来的生命周期行为。
    impl Parser {
        // `skip_ws` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        fn skip_ws(&mut self) {
            while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
                self.i += 1;
            }
        }

        // `peek` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        fn peek(&self) -> Option<char> {
            self.chars.get(self.i).copied()
        }

        // `bump` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        fn bump(&mut self) -> Option<char> {
            let c = self.peek()?;
            self.i += 1;
            Some(c)
        }

        // `parse_value` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        fn parse_value(&mut self) -> Result<AnyValue, String> {
            self.skip_ws();
            match self.peek() {
                Some('n') => self.parse_null(),
                Some('t') | Some('f') => self.parse_bool(),
                Some('"') => Ok(AnyValue::String(self.parse_string()?)),
                Some('[') => self.parse_array(),
                Some('{') => self.parse_object(),
                Some(c) if c == '-' || c.is_ascii_digit() => self.parse_number(),
                Some(c) => Err(format!("unexpected JSON char {c:?}")),
                None => Err("unexpected end of JSON".to_string()),
            }
        }

        // `parse_null` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        fn parse_null(&mut self) -> Result<AnyValue, String> {
            for expected in ['n', 'u', 'l', 'l'] {
                if self.bump() != Some(expected) {
                    return Err("invalid null".to_string());
                }
            }
            Ok(AnyValue::Null)
        }

        // `parse_bool` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        fn parse_bool(&mut self) -> Result<AnyValue, String> {
            if self.peek() == Some('t') {
                for expected in ['t', 'r', 'u', 'e'] {
                    if self.bump() != Some(expected) {
                        return Err("invalid true".to_string());
                    }
                }
                Ok(AnyValue::Bool(true))
            } else {
                for expected in ['f', 'a', 'l', 's', 'e'] {
                    if self.bump() != Some(expected) {
                        return Err("invalid false".to_string());
                    }
                }
                Ok(AnyValue::Bool(false))
            }
        }

        // `parse_number` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        fn parse_number(&mut self) -> Result<AnyValue, String> {
            let start = self.i;
            if self.peek() == Some('-') {
                self.i += 1;
            }
            match self.peek() {
                Some('0') => self.i += 1,
                Some('1'..='9') => {
                    self.i += 1;
                    while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                        self.i += 1;
                    }
                }
                _ => return Err("invalid number".to_string()),
            }
            if self.peek() == Some('.') {
                self.i += 1;
                let fraction_start = self.i;
                while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                    self.i += 1;
                }
                if self.i == fraction_start {
                    return Err("invalid number".to_string());
                }
            }
            if matches!(self.peek(), Some('e') | Some('E')) {
                self.i += 1;
                if matches!(self.peek(), Some('+') | Some('-')) {
                    self.i += 1;
                }
                let exponent_start = self.i;
                while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                    self.i += 1;
                }
                if self.i == exponent_start {
                    return Err("invalid number".to_string());
                }
            }
            let s: String = self.chars[start..self.i].iter().collect();
            let number = s.parse::<f64>().map_err(|_| "invalid number".to_string())?;
            if !number.is_finite() {
                return Err("invalid number".to_string());
            }
            Ok(AnyValue::Number(s))
        }

        // `parse_string` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        fn parse_string(&mut self) -> Result<String, String> {
            if self.bump() != Some('"') {
                return Err("expected string".to_string());
            }
            let mut out = String::new();
            loop {
                match self.bump() {
                    None => return Err("unterminated string".to_string()),
                    Some('"') => return Ok(out),
                    Some('\\') => match self.bump() {
                        Some('"') => out.push('"'),
                        Some('\\') => out.push('\\'),
                        Some('/') => out.push('/'),
                        Some('b') => out.push('\u{0008}'),
                        Some('f') => out.push('\u{000c}'),
                        Some('n') => out.push('\n'),
                        Some('r') => out.push('\r'),
                        Some('t') => out.push('\t'),
                        Some('u') => self.parse_unicode_escape(&mut out)?,
                        Some(c) => return Err(format!("bad escape \\{c}")),
                        None => return Err("unterminated escape".to_string()),
                    },
                    Some(c) if (c as u32) < 0x20 => {
                        return Err("invalid control character in string".to_string());
                    }
                    Some(c) => out.push(c),
                }
            }
        }

        fn parse_unicode_escape(&mut self, out: &mut String) -> Result<(), String> {
            let code = self.parse_hex_quad()?;
            if (0xd800..=0xdbff).contains(&code) {
                if self.peek() == Some('\\')
                    && self.chars.get(self.i + 1) == Some(&'u')
                    && self.i + 6 <= self.chars.len()
                {
                    let low_hex: String = self.chars[self.i + 2..self.i + 6].iter().collect();
                    if let Ok(low) = u32::from_str_radix(&low_hex, 16)
                        && (0xdc00..=0xdfff).contains(&low)
                    {
                        self.i += 6;
                        let scalar = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
                        out.push(char::from_u32(scalar).expect("valid UTF-16 surrogate pair"));
                        return Ok(());
                    }
                }
                out.push('\u{fffd}');
            } else if (0xdc00..=0xdfff).contains(&code) {
                out.push('\u{fffd}');
            } else {
                out.push(char::from_u32(code).expect("non-surrogate UTF-16 code unit"));
            }
            Ok(())
        }

        fn parse_hex_quad(&mut self) -> Result<u32, String> {
            let mut hex = String::with_capacity(4);
            for _ in 0..4 {
                match self.bump() {
                    Some(c) if c.is_ascii_hexdigit() => hex.push(c),
                    _ => return Err("bad unicode escape".to_string()),
                }
            }
            u32::from_str_radix(&hex, 16).map_err(|_| "bad unicode escape".to_string())
        }

        // `parse_array` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        fn parse_array(&mut self) -> Result<AnyValue, String> {
            if self.bump() != Some('[') {
                return Err("expected [".to_string());
            }
            self.skip_ws();
            let mut items = Vec::new();
            if self.peek() == Some(']') {
                self.i += 1;
                return Ok(AnyValue::Array(items));
            }
            loop {
                items.push(self.parse_value()?);
                self.skip_ws();
                match self.bump() {
                    Some(',') => continue,
                    Some(']') => return Ok(AnyValue::Array(items)),
                    other => return Err(format!("expected , or ] got {other:?}")),
                }
            }
        }

        // `parse_object` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        fn parse_object(&mut self) -> Result<AnyValue, String> {
            if self.bump() != Some('{') {
                return Err("expected {{".to_string());
            }
            self.skip_ws();
            let mut fields = Vec::new();
            if self.peek() == Some('}') {
                self.i += 1;
                return Ok(AnyValue::Object(fields));
            }
            loop {
                self.skip_ws();
                let key = self.parse_string()?;
                self.skip_ws();
                if self.bump() != Some(':') {
                    return Err("expected :".to_string());
                }
                let val = self.parse_value()?;
                fields.push((key, val));
                self.skip_ws();
                match self.bump() {
                    Some(',') => continue,
                    Some('}') => return Ok(AnyValue::Object(fields)),
                    other => return Err(format!("expected , or }} got {other:?}")),
                }
            }
        }
    }
}
