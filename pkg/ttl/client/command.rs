// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// TTL 节点间命令通道：基于 etcd 键空间的请求/响应与 watch。
//
// 发送方 `command` 写入带租约的 request 键并等待 response；接收方
// `watch_command` / `take_command` / `response_command` 完成认领与应答。
// 同时提供内存 `EtcdStore` 与 `MockClient` 便于单测。

use std::collections::{BTreeMap, HashMap};
use std::fmt::{Display, Formatter};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 命令 request/response 键的默认租约秒数。
pub const TTL_CMD_KEY_LEASE_SECONDS: u64 = 180;
/// etcd 中命令请求键前缀。
pub const TTL_CMD_KEY_REQUEST_PREFIX: &str = "/tidb/ttl/cmd/req/";
/// etcd 中命令响应键前缀。
pub const TTL_CMD_KEY_RESPONSE_PREFIX: &str = "/tidb/ttl/cmd/resp/";
/// 触发新 TTL job 的命令类型常量。
pub const TTL_CMD_TYPE_TRIGGER_TTL_JOB: &str = "trigger_ttl_job";

/// 无外部依赖的 JSON 值，对应 Go `json.RawMessage` 载荷，便于扩展新命令而不改传输层。
/// A dependency-free JSON value used for the raw command payloads carried by Go's
/// `json.RawMessage`. Keeping the complete JSON shape lets callers define new TTL
/// commands without changing this transport crate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

impl Default for JsonValue {
    fn default() -> Self {
        Self::Null
    }
}

impl JsonValue {
    /// 由键值迭代器构造 JSON 对象。
    pub fn object(entries: impl IntoIterator<Item = (impl Into<String>, JsonValue)>) -> Self {
        Self::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        )
    }

    /// 从 UTF-8 字节解析 JSON。
    pub fn parse(data: &[u8]) -> Result<Self, ClientError> {
        let text =
            std::str::from_utf8(data).map_err(|err| ClientError::Serialization(err.to_string()))?;
        JsonParser::new(text).parse()
    }

    /// 序列化为 JSON 字节。
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut output = String::new();
        self.write_json(&mut output);
        output.into_bytes()
    }

    /// 递归写入 JSON 文本。
    fn write_json(&self, output: &mut String) {
        match self {
            Self::Null => output.push_str("null"),
            Self::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
            Self::Number(value) => output.push_str(value),
            Self::String(value) => write_json_string(output, value),
            Self::Array(values) => {
                output.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        output.push(',');
                    }
                    value.write_json(output);
                }
                output.push(']');
            }
            Self::Object(values) => {
                output.push('{');
                for (index, (key, value)) in values.iter().enumerate() {
                    if index != 0 {
                        output.push(',');
                    }
                    write_json_string(output, key);
                    output.push(':');
                    value.write_json(output);
                }
                output.push('}');
            }
        }
    }

    /// 若为对象则取命名字段。
    fn field(&self, name: &str) -> Option<&Self> {
        match self {
            Self::Object(values) => values.get(name),
            _ => None,
        }
    }

    /// 若为字符串则返回内容。
    fn string(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    /// 若为数字则解析为 i64。
    fn i64(&self) -> Option<i64> {
        match self {
            Self::Number(value) => value.parse().ok(),
            _ => None,
        }
    }
}

impl From<String> for JsonValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for JsonValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<i64> for JsonValue {
    fn from(value: i64) -> Self {
        Self::Number(value.to_string())
    }
}

impl From<bool> for JsonValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

/// 写入带转义的 JSON 字符串字面量。
fn write_json_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character < '\u{20}' => {
                output.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

/// 手写 JSON 解析器（避免依赖第三方 serde）。
struct JsonParser<'a> {
    input: &'a [u8],
    position: usize,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            position: 0,
        }
    }

    fn parse(mut self) -> Result<JsonValue, ClientError> {
        let value = self.parse_value()?;
        self.skip_space();
        if self.position != self.input.len() {
            return self.error("trailing JSON data");
        }
        Ok(value)
    }

    fn parse_value(&mut self) -> Result<JsonValue, ClientError> {
        self.skip_space();
        match self.peek() {
            Some(b'n') => {
                self.literal(b"null")?;
                Ok(JsonValue::Null)
            }
            Some(b't') => {
                self.literal(b"true")?;
                Ok(JsonValue::Bool(true))
            }
            Some(b'f') => {
                self.literal(b"false")?;
                Ok(JsonValue::Bool(false))
            }
            Some(b'"') => Ok(JsonValue::String(self.parse_string()?)),
            Some(b'[') => self.parse_array(),
            Some(b'{') => self.parse_object(),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            _ => self.error("expected JSON value"),
        }
    }

    fn parse_array(&mut self) -> Result<JsonValue, ClientError> {
        self.position += 1;
        let mut values = Vec::new();
        self.skip_space();
        if self.consume(b']') {
            return Ok(JsonValue::Array(values));
        }
        loop {
            values.push(self.parse_value()?);
            self.skip_space();
            if self.consume(b']') {
                return Ok(JsonValue::Array(values));
            }
            if !self.consume(b',') {
                return self.error("expected ',' or ']' in array");
            }
        }
    }

    fn parse_object(&mut self) -> Result<JsonValue, ClientError> {
        self.position += 1;
        let mut values = BTreeMap::new();
        self.skip_space();
        if self.consume(b'}') {
            return Ok(JsonValue::Object(values));
        }
        loop {
            self.skip_space();
            if self.peek() != Some(b'"') {
                return self.error("expected JSON object key");
            }
            let key = self.parse_string()?;
            self.skip_space();
            if !self.consume(b':') {
                return self.error("expected ':' after object key");
            }
            values.insert(key, self.parse_value()?);
            self.skip_space();
            if self.consume(b'}') {
                return Ok(JsonValue::Object(values));
            }
            if !self.consume(b',') {
                return self.error("expected ',' or '}' in object");
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, ClientError> {
        self.position += 1;
        let mut output = String::new();
        while let Some(byte) = self.peek() {
            match byte {
                b'"' => {
                    self.position += 1;
                    return Ok(output);
                }
                b'\\' => {
                    self.position += 1;
                    let escaped = self
                        .next()
                        .ok_or_else(|| self.make_error("unterminated JSON escape"))?;
                    match escaped {
                        b'"' => output.push('"'),
                        b'\\' => output.push('\\'),
                        b'/' => output.push('/'),
                        b'b' => output.push('\u{08}'),
                        b'f' => output.push('\u{0c}'),
                        b'n' => output.push('\n'),
                        b'r' => output.push('\r'),
                        b't' => output.push('\t'),
                        b'u' => output.push(self.parse_unicode_escape()?),
                        _ => return self.error("invalid JSON escape"),
                    }
                }
                0..=0x1f => return self.error("control character in JSON string"),
                _ => {
                    let tail = std::str::from_utf8(&self.input[self.position..])
                        .map_err(|err| ClientError::Serialization(err.to_string()))?;
                    let character = tail
                        .chars()
                        .next()
                        .ok_or_else(|| self.make_error("unterminated JSON string"))?;
                    output.push(character);
                    self.position += character.len_utf8();
                }
            }
        }
        self.error("unterminated JSON string")
    }

    fn parse_unicode_escape(&mut self) -> Result<char, ClientError> {
        let first = self.parse_unicode_code_unit()?;
        if (0xd800..=0xdbff).contains(&first) {
            if self.input.get(self.position..self.position + 2) == Some(b"\\u") {
                let second_escape = self.position;
                self.position += 2;
                let second = self.parse_unicode_code_unit()?;
                if (0xdc00..=0xdfff).contains(&second) {
                    let scalar =
                        0x1_0000 + (((first - 0xd800) as u32) << 10) + (second - 0xdc00) as u32;
                    return char::from_u32(scalar)
                        .ok_or_else(|| self.make_error("invalid unicode scalar"));
                }
                self.position = second_escape;
                return Ok('\u{fffd}');
            }
            return Ok('\u{fffd}');
        }
        if (0xdc00..=0xdfff).contains(&first) {
            return Ok('\u{fffd}');
        }
        char::from_u32(first as u32).ok_or_else(|| self.make_error("invalid unicode scalar"))
    }

    fn parse_unicode_code_unit(&mut self) -> Result<u16, ClientError> {
        if self.position + 4 > self.input.len() {
            return self.error("short unicode escape");
        }
        let digits = std::str::from_utf8(&self.input[self.position..self.position + 4])
            .map_err(|err| ClientError::Serialization(err.to_string()))?;
        self.position += 4;
        let code = u16::from_str_radix(digits, 16)
            .map_err(|_| self.make_error("invalid unicode escape"))?;
        Ok(code)
    }

    fn parse_number(&mut self) -> Result<JsonValue, ClientError> {
        let start = self.position;
        if self.consume(b'-') && self.peek().is_none() {
            return self.error("invalid JSON number");
        }
        if self.consume(b'0') {
            if matches!(self.peek(), Some(b'0'..=b'9')) {
                return self.error("leading zero in JSON number");
            }
        } else {
            self.take_digits()?;
        }
        if self.consume(b'.') {
            self.take_digits()?;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.position += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.position += 1;
            }
            self.take_digits()?;
        }
        let number = std::str::from_utf8(&self.input[start..self.position]).unwrap();
        Ok(JsonValue::Number(number.to_owned()))
    }

    fn take_digits(&mut self) -> Result<(), ClientError> {
        let start = self.position;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.position += 1;
        }
        if self.position == start {
            self.error("expected digits")
        } else {
            Ok(())
        }
    }

    fn literal(&mut self, literal: &[u8]) -> Result<(), ClientError> {
        if self.input.get(self.position..self.position + literal.len()) == Some(literal) {
            self.position += literal.len();
            Ok(())
        } else {
            self.error("invalid JSON literal")
        }
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.position += 1;
        }
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.position).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.position += 1;
        Some(byte)
    }

    fn make_error(&self, message: &str) -> ClientError {
        ClientError::Serialization(format!("{message} at byte {}", self.position))
    }

    fn error<T>(&self, message: &str) -> Result<T, ClientError> {
        Err(self.make_error(message))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 命令客户端错误：取消、超时、序列化、后端与响应类失败。
pub enum ClientError {
    Cancelled,
    Timeout,
    Serialization(String),
    Backend(String),
    Response(String),
    ChannelClosed,
    WatcherBlocked,
    ResponseKeyNotFound(String),
    ResponseTypeMismatch,
}

impl Display for ClientError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "context canceled"),
            Self::Timeout => write!(f, "context deadline exceeded"),
            Self::Serialization(err) => write!(f, "json serialization failed: {err}"),
            Self::Backend(err) => write!(f, "backend error: {err}"),
            Self::Response(err) => write!(f, "{err}"),
            Self::ChannelClosed => write!(f, "response channel is closed"),
            Self::WatcherBlocked => write!(f, "command watcher is blocked"),
            Self::ResponseKeyNotFound(id) => write!(f, "response key not found for: {id}"),
            Self::ResponseTypeMismatch => write!(f, "response channel has unexpected type"),
        }
    }
}

impl std::error::Error for ClientError {}

#[derive(Clone, Debug)]
/// 可取消、可选截止时间的客户端上下文（对应 Go context）。
pub struct ClientContext {
    cancelled: Arc<AtomicBool>,
    deadline: Option<Instant>,
}

impl Default for ClientContext {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientContext {
    /// 创建未取消、无截止时间的上下文。
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline: None,
        }
    }

    /// 派生带超时的子上下文；截止时间取父与新超时的较早者。
    pub fn with_timeout(&self, timeout: Duration) -> Self {
        let requested = Instant::now() + timeout;
        let deadline = self
            .deadline
            .map_or(requested, |parent| parent.min(requested));
        Self {
            cancelled: Arc::clone(&self.cancelled),
            deadline: Some(deadline),
        }
    }

    /// 标记上下文已取消。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// 若已取消或超时则返回对应错误。
    pub fn error(&self) -> Option<ClientError> {
        if self.cancelled.load(Ordering::Acquire) {
            Some(ClientError::Cancelled)
        } else if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            Some(ClientError::Timeout)
        } else {
            None
        }
    }

    /// 计算下一次等待切片，不超过 maximum 且受截止时间约束。
    fn wait_slice(&self, maximum: Duration) -> Result<Duration, ClientError> {
        if let Some(err) = self.error() {
            return Err(err);
        }
        Ok(self
            .deadline
            .map(|deadline| {
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(maximum)
            })
            .unwrap_or(maximum))
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 一条命令请求：request_id、类型与 JSON 数据。
pub struct CmdRequest {
    pub request_id: String,
    pub cmd_type: String,
    pub data: JsonValue,
}

impl CmdRequest {
    /// 若类型为 trigger_ttl_job 则解析专用请求结构。
    pub fn get_trigger_ttl_job_request(&self) -> Option<TriggerNewTtlJobRequest> {
        if self.cmd_type != TTL_CMD_TYPE_TRIGGER_TTL_JOB {
            return None;
        }
        TriggerNewTtlJobRequest::from_json(&self.data)
    }

    fn to_json(&self) -> JsonValue {
        JsonValue::object([
            ("request_id", self.request_id.clone().into()),
            ("cmd_type", self.cmd_type.clone().into()),
            ("data", self.data.clone()),
        ])
    }

    fn from_bytes(data: &[u8]) -> Result<Self, ClientError> {
        let value = JsonValue::parse(data)?;
        Ok(Self {
            request_id: go_struct_string_field(&value, "request_id").ok_or_else(|| {
                ClientError::Serialization("request_id must be a string".to_owned())
            })?,
            cmd_type: go_struct_string_field(&value, "cmd_type").ok_or_else(|| {
                ClientError::Serialization("cmd_type must be a string".to_owned())
            })?,
            data: value.field("data").cloned().unwrap_or_default(),
        })
    }
}

#[derive(Clone, Debug, Default)]
/// 命令响应：可选错误信息与 JSON 数据。
struct CmdResponse {
    request_id: String,
    error_message: String,
    data: JsonValue,
}

impl CmdResponse {
    fn to_json(&self) -> JsonValue {
        JsonValue::object([
            ("request_id", self.request_id.clone().into()),
            ("error_message", self.error_message.clone().into()),
            ("data", self.data.clone()),
        ])
    }

    fn from_bytes(data: &[u8]) -> Result<Self, ClientError> {
        let value = JsonValue::parse(data)?;
        Ok(Self {
            request_id: go_struct_string_field(&value, "request_id").ok_or_else(|| {
                ClientError::Serialization("request_id must be a string".to_owned())
            })?,
            error_message: go_struct_string_field(&value, "error_message").ok_or_else(|| {
                ClientError::Serialization("error_message must be a string".to_owned())
            })?,
            data: value.field("data").cloned().unwrap_or_default(),
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 触发 TTL job 的请求：库名与表名。
pub struct TriggerNewTtlJobRequest {
    pub db_name: String,
    pub table_name: String,
}

impl TriggerNewTtlJobRequest {
    pub fn to_json(&self) -> JsonValue {
        JsonValue::object([
            ("db_name", self.db_name.clone().into()),
            ("table_name", self.table_name.clone().into()),
        ])
    }

    pub fn from_json(value: &JsonValue) -> Option<Self> {
        Some(Self {
            db_name: go_struct_string_field(value, "db_name")?,
            table_name: go_struct_string_field(value, "table_name")?,
        })
    }
}

impl From<TriggerNewTtlJobRequest> for JsonValue {
    fn from(value: TriggerNewTtlJobRequest) -> Self {
        value.to_json()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 单表触发结果：物理表标识、job_id 或错误信息。
pub struct TriggerNewTtlJobTableResult {
    pub table_id: i64,
    pub db_name: String,
    pub table_name: String,
    pub partition_name: String,
    pub job_id: String,
    pub error_message: String,
}

impl TriggerNewTtlJobTableResult {
    fn to_json(&self) -> JsonValue {
        let mut values = BTreeMap::from([
            ("table_id".to_owned(), self.table_id.into()),
            ("db_name".to_owned(), self.db_name.clone().into()),
            ("table_name".to_owned(), self.table_name.clone().into()),
        ]);
        for (key, value) in [
            ("partition_name", &self.partition_name),
            ("job_id", &self.job_id),
            ("error_message", &self.error_message),
        ] {
            if !value.is_empty() {
                values.insert(key.to_owned(), value.clone().into());
            }
        }
        JsonValue::Object(values)
    }

    fn from_json(value: &JsonValue) -> Option<Self> {
        Some(Self {
            table_id: go_struct_i64_field(value, "table_id")?,
            db_name: go_struct_string_field(value, "db_name")?,
            table_name: go_struct_string_field(value, "table_name")?,
            partition_name: go_struct_string_field(value, "partition_name")?,
            job_id: go_struct_string_field(value, "job_id")?,
            error_message: go_struct_string_field(value, "error_message")?,
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 触发 TTL job 的汇总响应（多表/多分区结果列表）。
pub struct TriggerNewTtlJobResponse {
    pub table_result: Vec<TriggerNewTtlJobTableResult>,
}

impl TriggerNewTtlJobResponse {
    pub fn to_json(&self) -> JsonValue {
        JsonValue::object([(
            "table_result",
            JsonValue::Array(
                self.table_result
                    .iter()
                    .map(TriggerNewTtlJobTableResult::to_json)
                    .collect(),
            ),
        )])
    }

    pub fn from_json(value: &JsonValue) -> Option<Self> {
        let results = match value.field("table_result") {
            None | Some(JsonValue::Null) => return Some(Self::default()),
            Some(JsonValue::Array(results)) => results,
            Some(_) => return None,
        };
        Some(Self {
            table_result: results
                .iter()
                .map(TriggerNewTtlJobTableResult::from_json)
                .collect::<Option<_>>()?,
        })
    }
}

impl From<TriggerNewTtlJobResponse> for JsonValue {
    fn from(value: TriggerNewTtlJobResponse) -> Self {
        value.to_json()
    }
}

/// Go `encoding/json` leaves a missing struct string field at its zero value,
/// while a present non-string field is a type error.
fn go_struct_string_field(value: &JsonValue, field: &str) -> Option<String> {
    match value.field(field) {
        None | Some(JsonValue::Null) => Some(String::new()),
        Some(JsonValue::String(value)) => Some(value.clone()),
        Some(_) => None,
    }
}

/// Go `encoding/json` also preserves the zero value for missing/null integer fields.
fn go_struct_i64_field(value: &JsonValue, field: &str) -> Option<i64> {
    match value.field(field) {
        None | Some(JsonValue::Null) => Some(0),
        Some(value) => value.i64(),
    }
}

#[derive(Clone, Debug)]
/// 响应方回传的结果：成功数据或错误字符串。
pub enum CommandResult {
    Data(JsonValue),
    Error(String),
}

impl CommandResult {
    /// 构造成功数据结果。
    pub fn data(value: impl Into<JsonValue>) -> Self {
        Self::Data(value.into())
    }
}

/// 命令订阅端接收句柄。
pub struct CommandReceiver {
    receiver: mpsc::Receiver<CmdRequest>,
}

impl CommandReceiver {
    /// 阻塞接收下一条命令请求。
    pub fn recv(&self) -> Result<CmdRequest, mpsc::RecvError> {
        self.receiver.recv()
    }

    /// 限时等待下一条命令请求。
    pub fn recv_timeout(&self, timeout: Duration) -> Result<CmdRequest, mpsc::RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }

    /// 非阻塞尝试接收。
    pub fn try_recv(&self) -> Result<CmdRequest, mpsc::TryRecvError> {
        self.receiver.try_recv()
    }
}

/// 命令客户端：发送、监视、认领与应答。
pub trait CommandClient: Send + Sync {
    /// 发送命令并等待响应，返回 (request_id, 结果)。
    fn command(
        &self,
        ctx: &ClientContext,
        command_type: &str,
        request: JsonValue,
    ) -> (String, Result<JsonValue, ClientError>);
    /// 订阅命令请求流。
    fn watch_command(&self, ctx: ClientContext) -> CommandReceiver;
    /// 认领指定 request；成功 true，已被认领或不存在 false。
    fn take_command(&self, ctx: &ClientContext, request_id: &str) -> Result<bool, ClientError>;
    /// 向发送方写入命令响应。
    fn response_command(
        &self,
        ctx: &ClientContext,
        request_id: &str,
        result: CommandResult,
    ) -> Result<(), ClientError>;
}

/// 便捷封装：发送 trigger_ttl_job 并解析结构化响应。
pub fn trigger_new_ttl_job(
    ctx: &ClientContext,
    client: &dyn CommandClient,
    database_name: impl Into<String>,
    table_name: impl Into<String>,
) -> Result<TriggerNewTtlJobResponse, ClientError> {
    let request = TriggerNewTtlJobRequest {
        db_name: database_name.into(),
        table_name: table_name.into(),
    }
    .to_json();
    let (_, response) = client.command(ctx, TTL_CMD_TYPE_TRIGGER_TTL_JOB, request);
    TriggerNewTtlJobResponse::from_json(&response?)
        .ok_or_else(|| ClientError::Serialization("invalid trigger TTL job response".to_owned()))
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// etcd watch 事件类型。
pub(crate) enum EtcdEventKind {
    Put,
    Delete,
}

#[derive(Clone, Debug)]
/// 一条 etcd watch 事件。
pub(crate) struct EtcdEvent {
    pub kind: EtcdEventKind,
    pub key: String,
    pub value: Vec<u8>,
}

/// 存储条目：值与可选过期时刻。
struct EtcdEntry {
    value: Vec<u8>,
    expires_at: Option<Instant>,
}

/// 注册的 watch 订阅者。
struct EtcdWatcher {
    id: u64,
    key: String,
    prefix: bool,
    sender: mpsc::Sender<EtcdEvent>,
}

#[derive(Default)]
/// 内存 etcd 状态：键值、watchers 与注入失败队列。
struct EtcdState {
    entries: HashMap<String, EtcdEntry>,
    watchers: Vec<EtcdWatcher>,
    failures: Vec<ClientError>,
}

#[derive(Default)]
/// 进程内模拟 etcd：支持 put/get/delete/watch 与租约过期。
pub struct EtcdStore {
    state: Mutex<EtcdState>,
    next_watcher: AtomicU64,
}

impl EtcdStore {
    /// 注入下一次操作将返回的错误（测试用）。
    pub fn fail_next(&self, error: ClientError) {
        self.state.lock().unwrap().failures.push(error);
    }

    fn operation_failure(state: &mut EtcdState) -> Result<(), ClientError> {
        if state.failures.is_empty() {
            Ok(())
        } else {
            Err(state.failures.remove(0))
        }
    }

    // 删除已过租约的条目。
    fn purge_expired(state: &mut EtcdState) {
        let now = Instant::now();
        state
            .entries
            .retain(|_, entry| entry.expires_at.map_or(true, |deadline| deadline > now));
    }

    pub(crate) fn put(
        &self,
        ctx: &ClientContext,
        key: String,
        value: Vec<u8>,
        lease_seconds: Option<u64>,
    ) -> Result<(), ClientError> {
        if let Some(err) = ctx.error() {
            return Err(err);
        }
        let event = EtcdEvent {
            kind: EtcdEventKind::Put,
            key: key.clone(),
            value: value.clone(),
        };
        let mut state = self.state.lock().unwrap();
        Self::operation_failure(&mut state)?;
        Self::purge_expired(&mut state);
        state.entries.insert(
            key.clone(),
            EtcdEntry {
                value,
                expires_at: lease_seconds
                    .map(|seconds| Instant::now() + Duration::from_secs(seconds)),
            },
        );
        state.watchers.retain(|watcher| {
            let matches = if watcher.prefix {
                key.starts_with(&watcher.key)
            } else {
                key == watcher.key
            };
            !matches || watcher.sender.send(event.clone()).is_ok()
        });
        Ok(())
    }

    pub(crate) fn get(
        &self,
        ctx: &ClientContext,
        key: &str,
    ) -> Result<Option<Vec<u8>>, ClientError> {
        if let Some(err) = ctx.error() {
            return Err(err);
        }
        let mut state = self.state.lock().unwrap();
        Self::operation_failure(&mut state)?;
        Self::purge_expired(&mut state);
        Ok(state.entries.get(key).map(|entry| entry.value.clone()))
    }

    fn delete(&self, ctx: &ClientContext, key: &str) -> Result<bool, ClientError> {
        if let Some(err) = ctx.error() {
            return Err(err);
        }
        let mut state = self.state.lock().unwrap();
        Self::operation_failure(&mut state)?;
        Self::purge_expired(&mut state);
        let deleted = state.entries.remove(key).is_some();
        if deleted {
            let event = EtcdEvent {
                kind: EtcdEventKind::Delete,
                key: key.to_owned(),
                value: Vec::new(),
            };
            state.watchers.retain(|watcher| {
                let matches = if watcher.prefix {
                    key.starts_with(&watcher.key)
                } else {
                    key == watcher.key
                };
                !matches || watcher.sender.send(event.clone()).is_ok()
            });
        }
        Ok(deleted)
    }

    pub(crate) fn watch(
        self: &Arc<Self>,
        ctx: ClientContext,
        key: String,
        prefix: bool,
    ) -> mpsc::Receiver<EtcdEvent> {
        let (sender, receiver) = mpsc::channel();
        let id = self.next_watcher.fetch_add(1, Ordering::Relaxed);
        self.state.lock().unwrap().watchers.push(EtcdWatcher {
            id,
            key,
            prefix,
            sender,
        });
        let store = Arc::clone(self);
        thread::spawn(move || {
            while ctx.error().is_none() {
                thread::sleep(Duration::from_millis(10));
            }
            store
                .state
                .lock()
                .unwrap()
                .watchers
                .retain(|watcher| watcher.id != id);
        });
        receiver
    }
}

#[derive(Clone)]
/// 基于 EtcdStore 的 CommandClient / NotificationClient 实现。
pub struct EtcdClient {
    pub(crate) store: Arc<EtcdStore>,
}

impl EtcdClient {
    pub fn new(store: Arc<EtcdStore>) -> Self {
        Self { store }
    }

    fn send_command(
        &self,
        ctx: &ClientContext,
        command_type: &str,
        data: JsonValue,
    ) -> (String, Result<(), ClientError>) {
        let request_id = new_request_id();
        let request = CmdRequest {
            request_id: request_id.clone(),
            cmd_type: command_type.to_owned(),
            data,
        };
        let encoded = request.to_json().to_bytes();
        let result = self.store.put(
            ctx,
            format!("{TTL_CMD_KEY_REQUEST_PREFIX}{request_id}"),
            encoded,
            Some(TTL_CMD_KEY_LEASE_SECONDS),
        );
        (request_id, result)
    }

    fn wait_command_response(
        &self,
        ctx: &ClientContext,
        request_id: &str,
    ) -> Result<JsonValue, ClientError> {
        let ctx = ctx.with_timeout(Duration::from_secs(TTL_CMD_KEY_LEASE_SECONDS));
        let key = format!("{TTL_CMD_KEY_RESPONSE_PREFIX}{request_id}");
        let watcher = self.store.watch(ctx.clone(), key.clone(), false);
        loop {
            let wait = ctx.wait_slice(Duration::from_secs(1))?;
            match watcher.recv_timeout(wait) {
                Ok(event) if event.kind == EtcdEventKind::Put => {
                    return decode_response(&event.value);
                }
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    thread::sleep(ctx.wait_slice(Duration::from_secs(1))?);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if let Some(value) = self.store.get(&ctx, &key)? {
                return decode_response(&value);
            }
        }
    }
}

/// 构造 etcd-backed 命令客户端。
pub fn new_command_client(store: Arc<EtcdStore>) -> Arc<dyn CommandClient> {
    Arc::new(EtcdClient::new(store))
}

impl CommandClient for EtcdClient {
    fn command(
        &self,
        ctx: &ClientContext,
        command_type: &str,
        request: JsonValue,
    ) -> (String, Result<JsonValue, ClientError>) {
        let (request_id, sent) = self.send_command(ctx, command_type, request);
        if let Err(err) = sent {
            return (request_id, Err(err));
        }
        let response = self.wait_command_response(ctx, &request_id);
        (request_id, response)
    }

    fn watch_command(&self, ctx: ClientContext) -> CommandReceiver {
        let events = self
            .store
            .watch(ctx.clone(), TTL_CMD_KEY_REQUEST_PREFIX.to_owned(), true);
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            while ctx.error().is_none() {
                match events.recv_timeout(Duration::from_millis(20)) {
                    Ok(event) if event.kind == EtcdEventKind::Put => {
                        // Go logs malformed JSON and still publishes the zero-value request.
                        let request = CmdRequest::from_bytes(&event.value).unwrap_or_default();
                        if sender.send(request).is_err() {
                            break;
                        }
                    }
                    Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        CommandReceiver { receiver }
    }

    fn take_command(&self, ctx: &ClientContext, request_id: &str) -> Result<bool, ClientError> {
        self.store
            .delete(ctx, &format!("{TTL_CMD_KEY_REQUEST_PREFIX}{request_id}"))
    }

    fn response_command(
        &self,
        ctx: &ClientContext,
        request_id: &str,
        result: CommandResult,
    ) -> Result<(), ClientError> {
        let (data, error_message) = match result {
            CommandResult::Data(data) => (data, String::new()),
            CommandResult::Error(error) => (JsonValue::Null, error),
        };
        let response = CmdResponse {
            request_id: request_id.to_owned(),
            error_message,
            data,
        }
        .to_json()
        .to_bytes();
        self.store.put(
            ctx,
            format!("{TTL_CMD_KEY_RESPONSE_PREFIX}{request_id}"),
            response,
            Some(TTL_CMD_KEY_LEASE_SECONDS),
        )
    }
}

/// 解码响应 JSON：有 error_message 则转为 Response 错误。
fn decode_response(data: &[u8]) -> Result<JsonValue, ClientError> {
    let response = CmdResponse::from_bytes(data)?;
    if response.error_message.is_empty() {
        Ok(response.data)
    } else {
        Err(ClientError::Response(response.error_message))
    }
}

struct MockResponseSender(mpsc::SyncSender<CmdResponse>);

struct MockWatcher {
    id: u64,
    sender: mpsc::SyncSender<CmdRequest>,
}

pub(crate) struct MockNotificationWatcher {
    pub id: u64,
    pub sender: mpsc::SyncSender<super::notification::NotificationEvent>,
}

enum MockValue {
    CommandResponse(MockResponseSender),
}

#[derive(Default)]
pub(crate) struct MockState {
    store: HashMap<String, MockValue>,
    requests: HashMap<String, CmdRequest>,
    command_watchers: Vec<MockWatcher>,
    pub(crate) notification_watchers: HashMap<String, Vec<MockNotificationWatcher>>,
}

#[derive(Default)]
/// 纯内存 mock 命令客户端，不经 EtcdStore。
pub struct MockClient {
    state: Arc<Mutex<MockState>>,
    next_watcher: AtomicU64,
}

impl MockClient {
    pub fn new() -> Self {
        Self::default()
    }

    fn send_command(
        &self,
        ctx: &ClientContext,
        command_type: &str,
        data: JsonValue,
    ) -> (String, Result<mpsc::Receiver<CmdResponse>, ClientError>) {
        let request_id = new_request_id();
        let request = CmdRequest {
            request_id: request_id.clone(),
            cmd_type: command_type.to_owned(),
            data,
        };
        let (response_sender, response_receiver) = mpsc::sync_channel(1);
        let mut state = self.state.lock().unwrap();
        state.store.insert(
            request_id.clone(),
            MockValue::CommandResponse(MockResponseSender(response_sender)),
        );
        state.requests.insert(request_id.clone(), request.clone());
        for watcher in &state.command_watchers {
            if let Some(err) = ctx.error() {
                return (request_id, Err(err));
            }
            if watcher.sender.try_send(request.clone()).is_err() {
                return (request_id, Err(ClientError::WatcherBlocked));
            }
        }
        (request_id, Ok(response_receiver))
    }

    pub(crate) fn next_watcher_id(&self) -> u64 {
        self.next_watcher.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn state(&self) -> &Arc<Mutex<MockState>> {
        &self.state
    }
}

/// 构造 mock 命令客户端。
pub fn new_mock_command_client() -> Arc<MockClient> {
    Arc::new(MockClient::new())
}

impl CommandClient for MockClient {
    fn command(
        &self,
        ctx: &ClientContext,
        command_type: &str,
        request: JsonValue,
    ) -> (String, Result<JsonValue, ClientError>) {
        let (request_id, sent) = self.send_command(ctx, command_type, request);
        let receiver = match sent {
            Ok(receiver) => receiver,
            Err(err) => return (request_id, Err(err)),
        };
        let wait_ctx = ctx.with_timeout(Duration::from_secs(TTL_CMD_KEY_LEASE_SECONDS));
        loop {
            let wait = match wait_ctx.wait_slice(Duration::from_millis(20)) {
                Ok(wait) => wait,
                Err(err) => return (request_id, Err(err)),
            };
            match receiver.recv_timeout(wait) {
                Ok(response) if response.error_message.is_empty() => {
                    return (request_id, Ok(response.data));
                }
                Ok(response) => {
                    return (
                        request_id,
                        Err(ClientError::Response(response.error_message)),
                    );
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return (request_id, Err(ClientError::ChannelClosed));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }

    fn watch_command(&self, ctx: ClientContext) -> CommandReceiver {
        let (sender, receiver) = mpsc::sync_channel(16 + self.state.lock().unwrap().requests.len());
        let id = self.next_watcher_id();
        {
            let mut state = self.state.lock().unwrap();
            state.command_watchers.push(MockWatcher {
                id,
                sender: sender.clone(),
            });
            for request in state.requests.values() {
                let _ = sender.try_send(request.clone());
            }
        }
        let state = Arc::clone(&self.state);
        thread::spawn(move || {
            while ctx.error().is_none() {
                thread::sleep(Duration::from_millis(10));
            }
            state
                .lock()
                .unwrap()
                .command_watchers
                .retain(|watcher| watcher.id != id);
        });
        CommandReceiver { receiver }
    }

    fn take_command(&self, _ctx: &ClientContext, request_id: &str) -> Result<bool, ClientError> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .requests
            .remove(request_id)
            .is_some())
    }

    fn response_command(
        &self,
        _ctx: &ClientContext,
        request_id: &str,
        result: CommandResult,
    ) -> Result<(), ClientError> {
        let value = self.state.lock().unwrap().store.remove(request_id);
        let Some(MockValue::CommandResponse(sender)) = value else {
            return Err(ClientError::ResponseKeyNotFound(request_id.to_owned()));
        };
        let (data, error_message) = match result {
            CommandResult::Data(data) => (data, String::new()),
            CommandResult::Error(error) => (JsonValue::Null, error),
        };
        // The Go mock asserts on a blocked channel but still returns nil.
        let _ = sender.0.try_send(CmdResponse {
            request_id: request_id.to_owned(),
            error_message,
            data,
        });
        Ok(())
    }
}

/// 生成唯一 request_id（时间戳 + 计数）。
fn new_request_id() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed) as u128;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let value = nanos ^ (sequence << 64) ^ u128::from(std::process::id());
    format!(
        "{:08x}-{:04x}-4{:03x}-a{:03x}-{:012x}",
        (value >> 96) as u32,
        (value >> 80) as u16,
        (value >> 68) as u16 & 0x0fff,
        (value >> 56) as u16 & 0x0fff,
        value & 0x0000_ffff_ffff_ffff_ffff
    )
}
