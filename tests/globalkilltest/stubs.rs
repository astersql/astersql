// Copyright 2026 AsterSQL.
//! Local stand-ins for util/server/log/errors HTTP boundaries
//! (arm64-safe; no kv/domain/kvproto/grpcio).
//!
//! Mirrors the surfaces `tests/globalkilltest` needs from
//! `github.com/pingcap/tidb/pkg/util`, `pkg/server`, `pingcap/errors`,
//! and `pingcap/log` without pulling heavy crates.

// 本文件对应 `tests/globalkilltest/stubs.rs`，本次任务只补中文解释，不改行为。
// 本文件提供轻量测试桩，而不是完整生产实现。
// 桩只覆盖当前测试真正触达的接口形状。
// 关键阅读点是全局开关、记录点和资源回收。
// 未覆盖的真实能力不会被假装支持。
// 中文注释会帮助区分桩职责与真实边界。
// 这类文件最怕隐式状态污染，因此会强调 reset 和 cleanup。
use std::fmt;

// ---------------------------------------------------------------------------
// errors (pingcap/errors.Trace shape)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
// `Error` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
pub struct Error {
    pub msg: String,
}

// 这里实现 `Error` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl Error {
    // `new` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }

    // `Error` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn Error(&self) -> &str {
        &self.msg
    }
}

// 这里实现 `fmt::Display` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl fmt::Display for Error {
    // `fmt` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

// 这里实现 `std::error::Error` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl std::error::Error for Error {}

// `Result` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
pub type Result<T> = std::result::Result<T, Error>;

// 模块 `errors` 在这里被显式接线，方便按既定边界编译。
// 阅读这一行时，可以把它看成当前 crate 的依赖入口说明。
// 保持导出关系稳定比改命名更重要。
pub mod errors {
    use super::Error;
    use std::fmt;

    pub use super::Result;

    // `New` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn New(msg: impl Into<String>) -> Error {
        Error::new(msg)
    }

    // `Errorf` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn Errorf(msg: impl fmt::Display) -> Error {
        Error::new(msg.to_string())
    }

    /// Go `errors.Trace(err)` — preserve message for callers.
    // `Trace` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn Trace(err: Error) -> Error {
        err
    }
}

// ---------------------------------------------------------------------------
// log (pingcap/log subset)
// ---------------------------------------------------------------------------

// 模块 `log` 在这里被显式接线，方便按既定边界编译。
// 阅读这一行时，可以把它看成当前 crate 的依赖入口说明。
// 保持导出关系稳定比改命名更重要。
pub mod log {
    use super::Error;

    // `Debug` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn Debug(msg: &str, err: Option<&Error>, retry: i32) {
        let _ = (msg, err, retry);
    }

    // `Info` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn Info(msg: &str, detail: &str) {
        let _ = (msg, detail);
    }
}

// ---------------------------------------------------------------------------
// util: ComposeURL + InternalHTTPClient (injectable HTTP GET)
// ---------------------------------------------------------------------------

// 模块 `util` 在这里被显式接线，方便按既定边界编译。
// 阅读这一行时，可以把它看成当前 crate 的依赖入口说明。
// 保持导出关系稳定比改命名更重要。
pub mod util {
    use super::{Result, errors};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Duration;

    // `INTERNAL_HTTP_SCHEMA` 记录跨函数共享的固定约束、错误文本或全局状态。
    // 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
    // 中文注释强调它为什么需要稳定。
    static INTERNAL_HTTP_SCHEMA: OnceLock<Mutex<String>> = OnceLock::new();
    // `HTTP_GET` 记录跨函数共享的固定约束、错误文本或全局状态。
    // 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
    // 中文注释强调它为什么需要稳定。
    static HTTP_GET: OnceLock<Mutex<HttpGetFn>> = OnceLock::new();

    // `schema_slot` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn schema_slot() -> &'static Mutex<String> {
        INTERNAL_HTTP_SCHEMA.get_or_init(|| Mutex::new("http".to_string()))
    }

    /// Go `util.InternalHTTPSchema()` — default `http` (no cluster TLS in stub).
    // `InternalHTTPSchema` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn InternalHTTPSchema() -> String {
        schema_slot().lock().unwrap().clone()
    }

    /// Test/helper: override schema (`http` / `https`).
    // `set_internal_http_schema` 负责清理或覆写跨用例共享状态。
    // 这类辅助函数最关键的是调用顺序与作用域。
    // 和 Go 对齐时，状态恢复直接关系到可重复性。
    pub fn set_internal_http_schema(schema: impl Into<String>) {
        *schema_slot().lock().unwrap() = schema.into();
    }

    /// Go `util.ComposeURL(address, path)`.
    // `ComposeURL` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn ComposeURL(address: &str, path: &str) -> String {
        if address.starts_with("http://") || address.starts_with("https://") {
            format!("{address}{path}")
        } else {
            format!("{}://{address}{path}", InternalHTTPSchema())
        }
    }

    /// HTTP response body handle — Close records resource cleanup.
    #[derive(Clone, Debug)]
    // `Body` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    // 理解它的边界有助于区分测试桩与真实实现。
    pub struct Body {
        pub data: Vec<u8>,
        closed: Arc<Mutex<bool>>,
    }

    // 这里实现 `Body` 的行为方法和资源回收语义。
    // 阅读这一段时，优先关注进入和离开方法时的状态变化。
    // 很多 parity 断言都会依赖这里保留下来的生命周期行为。
    impl Body {
        // `new` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        pub fn new(data: impl Into<Vec<u8>>) -> Self {
            Self {
                data: data.into(),
                closed: Arc::new(Mutex::new(false)),
            }
        }

        // `Close` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        pub fn Close(&self) {
            *self.closed.lock().unwrap() = true;
        }

        // `is_closed` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        pub fn is_closed(&self) -> bool {
            *self.closed.lock().unwrap()
        }

        // `as_slice` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        pub fn as_slice(&self) -> &[u8] {
            &self.data
        }
    }

    /// Minimal HTTP response matching Go `http.Response` fields used here.
    #[derive(Clone, Debug)]
    // `Response` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    // 理解它的边界有助于区分测试桩与真实实现。
    pub struct Response {
        pub StatusCode: i32,
        pub Body: Body,
    }

    // `StatusOK` 记录跨函数共享的固定约束、错误文本或全局状态。
    // 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
    // 中文注释强调它为什么需要稳定。
    pub const StatusOK: i32 = 200;

    // `HttpGetFn` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    // 理解它的边界有助于区分测试桩与真实实现。
    pub type HttpGetFn = Arc<dyn Fn(&str) -> Result<Response> + Send + Sync>;

    // `get_slot` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn get_slot() -> &'static Mutex<HttpGetFn> {
        HTTP_GET.get_or_init(|| {
            Mutex::new(Arc::new(|url: &str| {
                Err(errors::Errorf(format!(
                    "no HTTP handler registered for GET {url}"
                )))
            }))
        })
    }

    /// Install the HTTP GET backend used by `InternalHTTPClient`.
    // `set_http_get` 负责清理或覆写跨用例共享状态。
    // 这类辅助函数最关键的是调用顺序与作用域。
    // 和 Go 对齐时，状态恢复直接关系到可重复性。
    pub fn set_http_get(f: HttpGetFn) {
        *get_slot().lock().unwrap() = f;
    }

    /// Reset GET to the default failing handler.
    // `reset_http_get` 负责清理或覆写跨用例共享状态。
    // 这类辅助函数最关键的是调用顺序与作用域。
    // 和 Go 对齐时，状态恢复直接关系到可重复性。
    pub fn reset_http_get() {
        set_http_get(Arc::new(|url: &str| {
            Err(errors::Errorf(format!(
                "no HTTP handler registered for GET {url}"
            )))
        }));
    }

    /// Go `util.InternalHTTPClient()` — returns a client whose Get uses the stub.
    // `HttpClient` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    // 理解它的边界有助于区分测试桩与真实实现。
    pub struct HttpClient;

    // 这里实现 `HttpClient` 的行为方法和资源回收语义。
    // 阅读这一段时，优先关注进入和离开方法时的状态变化。
    // 很多 parity 断言都会依赖这里保留下来的生命周期行为。
    impl HttpClient {
        /// Go `client.Get(url)`.
        // `Get` 承担当前文件中的一段辅助职责或状态转换。
        // 中文注释会提示它依赖哪些前置条件。
        // 保持这层拆分可以让后续定位回归更直接。
        pub fn Get(&self, url: &str) -> Result<Response> {
            let f = get_slot().lock().unwrap().clone();
            f(url)
        }
    }

    // `InternalHTTPClient` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn InternalHTTPClient() -> HttpClient {
        HttpClient
    }

    // --- test timeout overrides (keep Go defaults in production path) ---

    // `TIMEOUT_OVERRIDES` 记录跨函数共享的固定约束、错误文本或全局状态。
    // 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
    // 中文注释强调它为什么需要稳定。
    static TIMEOUT_OVERRIDES: OnceLock<Mutex<TimeoutOverrides>> = OnceLock::new();

    #[derive(Clone, Copy, Debug, Default)]
    // `TimeoutOverrides` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    // 理解它的边界有助于区分测试桩与真实实现。
    pub struct TimeoutOverrides {
        pub pd: Option<Duration>,
        pub tikv: Option<Duration>,
        pub tidb: Option<Duration>,
        pub retry_interval: Option<Duration>,
    }

    // `timeout_slot` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn timeout_slot() -> &'static Mutex<TimeoutOverrides> {
        TIMEOUT_OVERRIDES.get_or_init(|| Mutex::new(TimeoutOverrides::default()))
    }

    // `set_timeout_overrides` 负责清理或覆写跨用例共享状态。
    // 这类辅助函数最关键的是调用顺序与作用域。
    // 和 Go 对齐时，状态恢复直接关系到可重复性。
    pub fn set_timeout_overrides(o: TimeoutOverrides) {
        *timeout_slot().lock().unwrap() = o;
    }

    // `clear_timeout_overrides` 负责清理或覆写跨用例共享状态。
    // 这类辅助函数最关键的是调用顺序与作用域。
    // 和 Go 对齐时，状态恢复直接关系到可重复性。
    pub fn clear_timeout_overrides() {
        *timeout_slot().lock().unwrap() = TimeoutOverrides::default();
    }

    // `timeout_overrides` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn timeout_overrides() -> TimeoutOverrides {
        *timeout_slot().lock().unwrap()
    }
}

// ---------------------------------------------------------------------------
// JSON decoder subset used by the PD and TiDB status responses.
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum JsonValue {
    Object(Vec<(String, JsonValue)>),
    String(String),
    Number(String),
    Null,
    Other,
}

impl JsonValue {
    fn field(&self, name: &str) -> Option<&Self> {
        let Self::Object(fields) = self else {
            return None;
        };
        fields
            .iter()
            .rev()
            .find_map(|(key, value)| (key == name).then_some(value))
    }
}

fn decode_json(bytes: &[u8]) -> Result<JsonValue> {
    let mut parser = JsonParser { bytes, offset: 0 };
    // Go's json.Decoder.Decode consumes one value; callers do not request a
    // second token, so trailing values are deliberately left unread.
    parser.value()
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl JsonParser<'_> {
    fn value(&mut self) -> Result<JsonValue> {
        self.whitespace();
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => self.string().map(JsonValue::String),
            Some(b'-' | b'0'..=b'9') => self.number().map(JsonValue::Number),
            Some(b't') => self.literal(b"true"),
            Some(b'f') => self.literal(b"false"),
            Some(b'n') => self.null(),
            _ => Err(errors::Errorf(
                "invalid character looking for beginning of value",
            )),
        }
    }

    fn object(&mut self) -> Result<JsonValue> {
        self.expect(b'{')?;
        let mut fields = Vec::new();
        self.whitespace();
        if self.consume(b'}') {
            return Ok(JsonValue::Object(fields));
        }
        loop {
            let key = self.string()?;
            self.expect(b':')?;
            fields.push((key, self.value()?));
            self.whitespace();
            if self.consume(b'}') {
                return Ok(JsonValue::Object(fields));
            }
            self.expect(b',')?;
        }
    }

    fn array(&mut self) -> Result<JsonValue> {
        self.expect(b'[')?;
        self.whitespace();
        if self.consume(b']') {
            return Ok(JsonValue::Other);
        }
        loop {
            self.value()?;
            self.whitespace();
            if self.consume(b']') {
                return Ok(JsonValue::Other);
            }
            self.expect(b',')?;
        }
    }

    fn string(&mut self) -> Result<String> {
        self.expect(b'"')?;
        let mut result = String::new();
        loop {
            match self
                .peek()
                .ok_or_else(|| errors::Errorf("unexpected end of JSON input"))?
            {
                b'"' => {
                    self.offset += 1;
                    return Ok(result);
                }
                b'\\' => {
                    self.offset += 1;
                    let escaped = self.take()?;
                    match escaped {
                        b'"' => result.push('"'),
                        b'\\' => result.push('\\'),
                        b'/' => result.push('/'),
                        b'b' => result.push('\u{0008}'),
                        b'f' => result.push('\u{000c}'),
                        b'n' => result.push('\n'),
                        b'r' => result.push('\r'),
                        b't' => result.push('\t'),
                        b'u' => result.push(self.unicode_escape()?),
                        _ => return Err(errors::Errorf("invalid character in string escape code")),
                    }
                }
                0x00..=0x1f => return Err(errors::Errorf("invalid character in string literal")),
                _ => {
                    let remainder = std::str::from_utf8(&self.bytes[self.offset..])
                        .map_err(|e| errors::Errorf(e.to_string()))?;
                    let ch = remainder.chars().next().unwrap();
                    result.push(ch);
                    self.offset += ch.len_utf8();
                }
            }
        }
    }

    fn unicode_escape(&mut self) -> Result<char> {
        let first = self.hex_u16()?;
        let scalar = if (0xd800..=0xdbff).contains(&first) {
            if !self.consume(b'\\') || !self.consume(b'u') {
                return Err(errors::Errorf("invalid Unicode surrogate pair"));
            }
            let second = self.hex_u16()?;
            if !(0xdc00..=0xdfff).contains(&second) {
                return Err(errors::Errorf("invalid Unicode surrogate pair"));
            }
            0x10000 + ((u32::from(first) - 0xd800) << 10) + u32::from(second) - 0xdc00
        } else if (0xdc00..=0xdfff).contains(&first) {
            return Err(errors::Errorf("invalid Unicode surrogate pair"));
        } else {
            u32::from(first)
        };
        char::from_u32(scalar).ok_or_else(|| errors::Errorf("invalid Unicode code point"))
    }

    fn hex_u16(&mut self) -> Result<u16> {
        let mut value = 0u16;
        for _ in 0..4 {
            let digit = match self.take()? {
                b'0'..=b'9' => self.bytes[self.offset - 1] - b'0',
                b'a'..=b'f' => self.bytes[self.offset - 1] - b'a' + 10,
                b'A'..=b'F' => self.bytes[self.offset - 1] - b'A' + 10,
                _ => return Err(errors::Errorf("invalid Unicode escape")),
            };
            value = value * 16 + u16::from(digit);
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<String> {
        let start = self.offset;
        self.consume(b'-');
        match self.peek() {
            Some(b'0') => self.offset += 1,
            Some(b'1'..=b'9') => {
                self.offset += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.offset += 1;
                }
            }
            _ => return Err(errors::Errorf("invalid number literal")),
        }
        if self.consume(b'.') {
            self.digits()?;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.offset += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.offset += 1;
            }
            self.digits()?;
        }
        std::str::from_utf8(&self.bytes[start..self.offset])
            .map(str::to_owned)
            .map_err(|e| errors::Errorf(e.to_string()))
    }

    fn digits(&mut self) -> Result<()> {
        let start = self.offset;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.offset += 1;
        }
        (self.offset != start)
            .then_some(())
            .ok_or_else(|| errors::Errorf("invalid number literal"))
    }

    fn literal(&mut self, literal: &[u8]) -> Result<JsonValue> {
        if self.bytes.get(self.offset..self.offset + literal.len()) == Some(literal) {
            self.offset += literal.len();
            Ok(JsonValue::Other)
        } else {
            Err(errors::Errorf("invalid literal"))
        }
    }

    fn null(&mut self) -> Result<JsonValue> {
        if self.bytes.get(self.offset..self.offset + 4) == Some(b"null") {
            self.offset += 4;
            Ok(JsonValue::Null)
        } else {
            Err(errors::Errorf("invalid literal"))
        }
    }

    fn expect(&mut self, byte: u8) -> Result<()> {
        self.whitespace();
        self.consume(byte)
            .then_some(())
            .ok_or_else(|| errors::Errorf("unexpected character in JSON input"))
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.offset += 1;
        }
    }

    fn take(&mut self) -> Result<u8> {
        let byte = self
            .peek()
            .ok_or_else(|| errors::Errorf("unexpected end of JSON input"))?;
        self.offset += 1;
        Ok(byte)
    }

    fn consume(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.offset += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.offset).copied()
    }
}

// ---------------------------------------------------------------------------
// server.Status (pkg/server JSON /status body)
// ---------------------------------------------------------------------------

// 模块 `server` 在这里被显式接线，方便按既定边界编译。
// 阅读这一行时，可以把它看成当前 crate 的依赖入口说明。
// 保持导出关系稳定比改命名更重要。
pub mod server {
    use super::{Result, errors};

    /// Go `server.Status` — `/status` JSON response body.
    #[derive(Clone, Debug, Default, PartialEq)]
    // `Status` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    // 理解它的边界有助于区分测试桩与真实实现。
    pub struct Status {
        pub connections: usize,
        pub version: String,
        pub git_hash: String,
        pub status: DetailStatus,
    }

    /// Go `server.DetailStatus`.
    #[derive(Clone, Debug, Default, PartialEq)]
    // `DetailStatus` 承载这一层需要长期保存或暴露的状态。
    // 字段通常只覆盖当前测试真正依赖的最小语义闭包。
    // 理解它的边界有助于区分测试桩与真实实现。
    pub struct DetailStatus {
        pub init_stats_percentage: f64,
    }

    /// Decode Go `server.Status` JSON (fields used by globalkilltest).
    // `decode_status` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    pub fn decode_status(bytes: &[u8]) -> Result<Status> {
        let root = super::decode_json(bytes)?;
        if !matches!(root, super::JsonValue::Object(_) | super::JsonValue::Null) {
            return Err(errors::Errorf(
                "cannot unmarshal JSON value into server.Status",
            ));
        }
        let connections = number_field::<usize>(&root, "connections")?.unwrap_or_default();
        let version = string_field(&root, "version")?.unwrap_or_default();
        let git_hash = string_field(&root, "git_hash")?.unwrap_or_default();
        let pct = match root.field("status") {
            None | Some(super::JsonValue::Null) => 0.0,
            Some(status @ super::JsonValue::Object(_)) => {
                number_field::<f64>(status, "init_stats_percentage")?.unwrap_or_default()
            }
            Some(_) => return Err(errors::Errorf("cannot unmarshal status field")),
        };
        Ok(Status {
            connections,
            version,
            git_hash,
            status: DetailStatus {
                init_stats_percentage: pct,
            },
        })
    }

    fn string_field(value: &super::JsonValue, key: &str) -> Result<Option<String>> {
        match value.field(key) {
            None | Some(super::JsonValue::Null) => Ok(None),
            Some(super::JsonValue::String(value)) => Ok(Some(value.clone())),
            Some(_) => Err(errors::Errorf(format!("cannot unmarshal {key} as string"))),
        }
    }

    fn number_field<T>(value: &super::JsonValue, key: &str) -> Result<Option<T>>
    where
        T: std::str::FromStr,
    {
        match value.field(key) {
            None | Some(super::JsonValue::Null) => Ok(None),
            Some(super::JsonValue::Number(value)) => value
                .parse()
                .map(Some)
                .map_err(|_| errors::Errorf(format!("cannot unmarshal {key} as number"))),
            Some(_) => Err(errors::Errorf(format!("cannot unmarshal {key} as number"))),
        }
    }
}

/// Decode PD `/health` JSON body → health string value.
// `decode_pd_health` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
pub fn decode_pd_health(bytes: &[u8]) -> Result<String> {
    let root = decode_json(bytes)?;
    if !matches!(root, JsonValue::Object(_) | JsonValue::Null) {
        return Err(errors::Errorf(
            "cannot unmarshal JSON value into health status",
        ));
    }
    match root.field("health") {
        None | Some(JsonValue::Null) => Ok(String::new()),
        Some(JsonValue::String(value)) => Ok(value.clone()),
        Some(_) => Err(errors::Errorf("cannot unmarshal health as string")),
    }
}

// Re-export http status constant used by util.rs via util::StatusOK.
pub use util::StatusOK;
