// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// TiKV worker HTTP 客户端：流式写入 SST 并按 Region ingest。
//
// 实现 `HttpTransport`、chunked PUT `/write_sst`、POST `/ingest_s3`，以及精简 JSON 解析。
// SST：Sorted String Table；Region：TiKV 键范围分片；ingest：跳过常规写路径直接导入。

use astersql_ingestor_errdef as errdef;
use astersql_ingestor_ingestmetric as ingestmetric;
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Instant;

/// 客户端与传输层错误（取消、I/O、HTTP/JSON/protobuf、ingest API 等）。
#[derive(Debug)]
pub enum Error {
    Canceled,
    Io(io::Error),
    InvalidUrl(String),
    UnsupportedScheme(String),
    InvalidHttpResponse(String),
    Json(String),
    Protobuf(String),
    HttpStatus(errdef::HTTPStatusError),
    ClosedPipe,
    WorkerPanicked,
    MissingWriteResponse,
    MissingSstMeta,
    MissingLeader,
    IngestApi(crate::IngestAPIError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Canceled => formatter.write_str("request context canceled"),
            Self::Io(error) => write!(formatter, "HTTP I/O failed: {error}"),
            Self::InvalidUrl(url) => write!(formatter, "invalid HTTP URL: {url}"),
            Self::UnsupportedScheme(scheme) => {
                write!(formatter, "transport does not support URL scheme {scheme}")
            }
            Self::InvalidHttpResponse(message) => {
                write!(formatter, "invalid HTTP response: {message}")
            }
            Self::Json(message) => write!(formatter, "JSON response failed: {message}"),
            Self::Protobuf(message) => write!(formatter, "protobuf response failed: {message}"),
            Self::HttpStatus(error) => write!(formatter, "{error}"),
            Self::ClosedPipe => formatter.write_str("write stream is closed"),
            Self::WorkerPanicked => formatter.write_str("write request thread panicked"),
            Self::MissingWriteResponse => formatter.write_str("write response is missing"),
            Self::MissingSstMeta => {
                formatter.write_str("write response does not contain SST metadata")
            }
            Self::MissingLeader => formatter.write_str("region has no leader"),
            Self::IngestApi(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::HttpStatus(error) => Some(error),
            Self::IngestApi(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<errdef::HTTPStatusError> for Error {
    fn from(error: errdef::HTTPStatusError) -> Self {
        Self::HttpStatus(error)
    }
}

impl From<crate::IngestAPIError> for Error {
    fn from(error: crate::IngestAPIError) -> Self {
        Self::IngestApi(error)
    }
}

/// JsonByteSlice serializes bytes as a JSON integer array rather than base64,
/// matching next-generation TiKV's `Vector<u8>` representation.
/// 将字节序列化为 JSON 整数数组而非 base64，对齐新一代 TiKV 的 `Vector<u8>`。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct JsonByteSlice(pub Option<Vec<u8>>);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 新一代写流返回的 SST 元数据：id、键范围、meta_offset、commit_ts。
pub struct NextGenSstMeta {
    pub id: i64,
    pub smallest: JsonByteSlice,
    pub biggest: JsonByteSlice,
    pub meta_offset: usize,
    pub commit_ts: usize,
}

impl std::fmt::Display for NextGenSstMeta {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{{ID: {}, Smallest: {}, Biggest: {}, CommitTs: {}}}",
            self.id,
            redacted_key(&self.smallest),
            redacted_key(&self.biggest),
            self.commit_ts
        )
    }
}

/// Display 时脱敏键：只显示字节长度，避免日志泄漏。
fn redacted_key(key: &JsonByteSlice) -> String {
    match &key.0 {
        Some(bytes) => format!("?{} bytes", bytes.len()),
        None => "<nil>".to_owned(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 最小 HTTP 响应：状态码与 body。
pub struct HttpResponse {
    pub status_code: u16,
    pub body: Vec<u8>,
}

/// HttpTransport keeps the Go client's injected HTTP transport semantics. The
/// standard implementation handles plain HTTP; deployments can inject their
/// canonical TLS transport without coupling this crate to a new dependency.
/// 可注入的 HTTP 传输；默认实现仅支持明文 HTTP，部署可注入 TLS 传输。
pub trait HttpTransport: Send + Sync {
    fn put_stream(&self, url: &str, chunks: mpsc::Receiver<Vec<u8>>)
    -> Result<HttpResponse, Error>;

    fn post(&self, url: &str, body: &[u8]) -> Result<HttpResponse, Error>;
}

/// 共享的 HttpTransport 句柄。
pub type SharedHttpTransport = Arc<dyn HttpTransport>;

#[derive(Clone, Copy, Debug, Default)]
/// 基于 TcpStream 的标准明文 HTTP 传输。
pub struct StdHttpTransport;

impl HttpTransport for StdHttpTransport {
    fn put_stream(
        &self,
        url: &str,
        chunks: mpsc::Receiver<Vec<u8>>,
    ) -> Result<HttpResponse, Error> {
        // chunked PUT：发送分块体后读完整 HTTP 响应。
        let url = ParsedUrl::parse(url)?;
        let mut stream = url.connect()?;
        write!(
            stream,
            "PUT {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            url.path, url.authority
        )?;
        for chunk in chunks {
            write!(stream, "{:x}\r\n", chunk.len())?;
            stream.write_all(&chunk)?;
            stream.write_all(b"\r\n")?;
        }
        stream.write_all(b"0\r\n\r\n")?;
        stream.flush()?;
        read_http_response(stream)
    }

    fn post(&self, url: &str, body: &[u8]) -> Result<HttpResponse, Error> {
        let url = ParsedUrl::parse(url)?;
        let mut stream = url.connect()?;
        write!(
            stream,
            "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            url.path,
            url.authority,
            body.len()
        )?;
        stream.write_all(body)?;
        stream.flush()?;
        read_http_response(stream)
    }
}

/// 仅支持 http:// 的简易 URL 解析结果。
struct ParsedUrl {
    authority: String,
    host: String,
    port: u16,
    path: String,
}

impl ParsedUrl {
    /// 解析 http URL 的 authority/host/port/path；IPv6 支持括号形式。
    fn parse(url: &str) -> Result<Self, Error> {
        let (scheme, remainder) = url
            .split_once("://")
            .ok_or_else(|| Error::InvalidUrl(url.to_owned()))?;
        if scheme != "http" {
            return Err(Error::UnsupportedScheme(scheme.to_owned()));
        }
        let (authority, path) = remainder
            .split_once('/')
            .map_or((remainder, "/"), |(authority, _path)| {
                (authority, &remainder[authority.len()..])
            });
        if authority.is_empty() {
            return Err(Error::InvalidUrl(url.to_owned()));
        }
        let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
            let end = bracketed
                .find(']')
                .ok_or_else(|| Error::InvalidUrl(url.to_owned()))?;
            let host = bracketed[..end].to_owned();
            let suffix = &bracketed[end + 1..];
            let port = if suffix.is_empty() {
                80
            } else {
                suffix
                    .strip_prefix(':')
                    .ok_or_else(|| Error::InvalidUrl(url.to_owned()))?
                    .parse()
                    .map_err(|_| Error::InvalidUrl(url.to_owned()))?
            };
            (host, port)
        } else if let Some((host, port)) = authority.rsplit_once(':') {
            let port = port
                .parse()
                .map_err(|_| Error::InvalidUrl(url.to_owned()))?;
            (host.to_owned(), port)
        } else {
            (authority.to_owned(), 80)
        };
        Ok(Self {
            authority: authority.to_owned(),
            host,
            port,
            path: path.to_owned(),
        })
    }

    /// 建立到 host:port 的 TCP 连接。
    fn connect(&self) -> Result<TcpStream, Error> {
        Ok(TcpStream::connect((self.host.as_str(), self.port))?)
    }
}

/// 读取至 EOF，解析状态行与 chunked/普通 body。
fn read_http_response(mut stream: TcpStream) -> Result<HttpResponse, Error> {
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes)?;
    let header_end = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| Error::InvalidHttpResponse("missing header terminator".to_owned()))?;
    let headers = std::str::from_utf8(&bytes[..header_end])
        .map_err(|error| Error::InvalidHttpResponse(error.to_string()))?;
    let mut lines = headers.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| Error::InvalidHttpResponse("missing status line".to_owned()))?;
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| Error::InvalidHttpResponse(status_line.to_owned()))?
        .parse()
        .map_err(|_| Error::InvalidHttpResponse(status_line.to_owned()))?;
    let chunked = lines.any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("transfer-encoding")
                && value
                    .split(',')
                    .any(|encoding| encoding.trim().eq_ignore_ascii_case("chunked"))
        })
    });
    let body = &bytes[header_end + 4..];
    Ok(HttpResponse {
        status_code,
        body: if chunked {
            decode_chunked_body(body)?
        } else {
            body.to_vec()
        },
    })
}

/// 解码 Transfer-Encoding: chunked 响应体。
fn decode_chunked_body(mut input: &[u8]) -> Result<Vec<u8>, Error> {
    let mut output = Vec::new();
    loop {
        let line_end = input
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| Error::InvalidHttpResponse("invalid chunk header".to_owned()))?;
        let length_text = std::str::from_utf8(&input[..line_end])
            .map_err(|error| Error::InvalidHttpResponse(error.to_string()))?;
        let length =
            usize::from_str_radix(length_text.split(';').next().unwrap_or_default().trim(), 16)
                .map_err(|_| Error::InvalidHttpResponse("invalid chunk length".to_owned()))?;
        input = &input[line_end + 2..];
        if length == 0 {
            return Ok(output);
        }
        if input.len() < length + 2 || &input[length..length + 2] != b"\r\n" {
            return Err(Error::InvalidHttpResponse(
                "truncated chunked body".to_owned(),
            ));
        }
        output.extend_from_slice(&input[..length]);
        input = &input[length + 2..];
    }
}

/// 写流后台线程与前台共享的完成结果。
type SharedOutcome = Arc<Mutex<Option<Result<NextGenSstMeta, Error>>>>;

/// 写流客户端：同步 channel 推 chunk，后台线程 PUT，finish 取 SST meta。
pub struct WriteClientImpl {
    sender: Option<mpsc::SyncSender<Vec<u8>>>,
    outcome: SharedOutcome,
    worker: Option<JoinHandle<()>>,
}

impl WriteClientImpl {
    /// 启动后台写线程，URL 含 cluster_id 与 commit_ts。
    fn new(
        tikv_worker_url: &str,
        cluster_id: u64,
        http_transport: SharedHttpTransport,
        commit_ts: u64,
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel(8);
        let outcome = Arc::new(Mutex::new(None));
        let worker_outcome = outcome.clone();
        let url =
            format!("{tikv_worker_url}/write_sst?cluster_id={cluster_id}&commit_ts={commit_ts}");
        let worker = std::thread::spawn(move || {
            let started = Instant::now();
            let result = send_write_request(http_transport.as_ref(), &url, receiver);
            observe_write_duration(started);
            *worker_outcome.lock().expect("write outcome lock poisoned") = Some(result);
        });
        Self {
            sender: Some(sender),
            outcome,
            worker: Some(worker),
        }
    }

    /// 关闭发送端、join worker，取出写结果。
    fn finish(&mut self) -> Result<NextGenSstMeta, Error> {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| Error::WorkerPanicked)?;
        }
        self.outcome
            .lock()
            .expect("write outcome lock poisoned")
            .take()
            .ok_or(Error::MissingWriteResponse)?
    }

    /// 管道已断时尽量带回 worker 错误，否则 ClosedPipe。
    fn cause_closed_pipe(&mut self) -> Error {
        match self.finish() {
            Err(error) => error,
            Ok(_) => Error::ClosedPipe,
        }
    }
}

/// 发起 chunked PUT，校验 200 并解码 sst_meta JSON。
fn send_write_request(
    transport: &dyn HttpTransport,
    url: &str,
    chunks: mpsc::Receiver<Vec<u8>>,
) -> Result<NextGenSstMeta, Error> {
    let response = transport.put_stream(url, chunks)?;
    if response.status_code != 200 {
        return Err(errdef::HTTPStatusError {
            StatusCode: i32::from(response.status_code),
            Message: format!(
                "failed to send chunked request: {}",
                String::from_utf8_lossy(&response.body)
            ),
        }
        .into());
    }
    decode_next_gen_response(&response.body)
}

impl crate::WriteClient for WriteClientImpl {
    fn write(&mut self, request: crate::WriteRequest) -> Result<(), Error> {
        // 打包：每对 KV 为 u16 LE key_len + key + u32 LE value_len + value。
        let mut buffer = Vec::new();
        for pair in request.pairs {
            buffer.extend_from_slice(&(pair.key.len() as u16).to_le_bytes());
            buffer.extend_from_slice(&pair.key);
            buffer.extend_from_slice(&(pair.value.len() as u32).to_le_bytes());
            buffer.extend_from_slice(&pair.value);
        }
        let Some(sender) = self.sender.as_ref() else {
            return Err(Error::ClosedPipe);
        };
        if sender.send(buffer).is_err() {
            return Err(self.cause_closed_pipe());
        }
        Ok(())
    }

    fn recv(&mut self) -> Result<crate::WriteResponse, Error> {
        let sst_meta = self.finish()?;
        Ok(crate::WriteResponse {
            next_gen_sst_meta: Some(sst_meta),
        })
    }

    fn close(&mut self) {
        let _ = self.finish();
    }
}

impl Drop for WriteClientImpl {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

/// ingestcli Client 实现：持有 worker URL、cluster_id、传输与 SplitClient。
pub struct ClientImpl {
    pub url_schema: String,
    pub tikv_worker_url: String,
    cluster_id: u64,
    http_transport: SharedHttpTransport,
    split_client: crate::SharedSplitClient,
}

impl ClientImpl {
    /// 构造客户端；若 URL 无 schema 则按 is_https 补全。
    pub fn new(
        tikv_worker_url: impl Into<String>,
        cluster_id: u64,
        is_https: bool,
        http_transport: SharedHttpTransport,
        split_client: crate::SharedSplitClient,
    ) -> Self {
        let mut tikv_worker_url = tikv_worker_url.into();
        let url_schema = if is_https { "https://" } else { "http://" };
        if !tikv_worker_url.starts_with("http://") && !tikv_worker_url.starts_with("https://") {
            tikv_worker_url = format!("{url_schema}{tikv_worker_url}");
        }
        Self {
            url_schema: url_schema.to_owned(),
            tikv_worker_url,
            cluster_id,
            http_transport,
            split_client,
        }
    }

    /// 使用 StdHttpTransport 与 HTTP schema 的便捷构造。
    pub fn new_std(
        tikv_worker_url: impl Into<String>,
        cluster_id: u64,
        split_client: crate::SharedSplitClient,
    ) -> Self {
        Self::new(
            tikv_worker_url,
            cluster_id,
            false,
            Arc::new(StdHttpTransport),
            split_client,
        )
    }
}

/// 工厂：返回 `Arc<dyn Client>`。
pub fn NewClient(
    tikv_worker_url: impl Into<String>,
    cluster_id: u64,
    is_https: bool,
    http_transport: SharedHttpTransport,
    split_client: crate::SharedSplitClient,
) -> Arc<dyn crate::Client> {
    Arc::new(ClientImpl::new(
        tikv_worker_url,
        cluster_id,
        is_https,
        http_transport,
        split_client,
    ))
}

impl crate::Client for ClientImpl {
    fn write_client(
        &self,
        context: &dyn crate::RequestContext,
        commit_ts: u64,
    ) -> Result<Box<dyn crate::WriteClient>, Error> {
        if context.is_cancelled() {
            return Err(Error::Canceled);
        }
        Ok(Box::new(WriteClientImpl::new(
            &self.tikv_worker_url,
            self.cluster_id,
            self.http_transport.clone(),
            commit_ts,
        )))
    }

    fn ingest(
        &self,
        context: &dyn crate::RequestContext,
        request: crate::IngestRequest,
    ) -> Result<(), Error> {
        if context.is_cancelled() {
            return Err(Error::Canceled);
        }
        // 经 SplitClient 取 leader store 的 status_address，POST /ingest_s3。
        let leader = request.region.leader.as_ref().ok_or(Error::MissingLeader)?;
        let store = self.split_client.get_store(context, leader.store_id)?;
        let version = request
            .region
            .region
            .region_epoch
            .as_ref()
            .map_or(0, |epoch| epoch.version);
        let url = format!(
            "{}{}{}?cluster_id={}&region_id={}&epoch_version={}",
            self.url_schema,
            store.status_address,
            "/ingest_s3",
            self.cluster_id,
            request.region.region.id,
            version
        );
        let sst_meta = request
            .write_response
            .next_gen_sst_meta
            .as_ref()
            .ok_or(Error::MissingSstMeta)?;
        let body = encode_sst_meta(sst_meta);
        let _duration = IngestDurationGuard(Instant::now());
        let response = self.http_transport.post(&url, body.as_bytes())?;
        if response.status_code == 200 {
            return Ok(());
        }
        let mut protobuf_error =
            crate::ingest_err::decode_error_pb(&response.body).map_err(|error| {
                Error::HttpStatus(errdef::HTTPStatusError {
                    StatusCode: i32::from(response.status_code),
                    Message: format!("failed to unmarshal error response: {error}"),
                })
            })?;
        protobuf_error.message =
            format!("{}(ingest SST ID {})", protobuf_error.message, sst_meta.id);
        Err(crate::NewIngestAPIError(&protobuf_error, None).into())
    }
}

/// 将 NextGenSstMeta 编码为 ingest 请求 JSON body。
fn encode_sst_meta(meta: &NextGenSstMeta) -> String {
    format!(
        "{{\"id\":{},\"smallest\":{},\"biggest\":{},\"meta-offset\":{},\"commit-ts\":{}}}",
        meta.id,
        encode_json_bytes(&meta.smallest),
        encode_json_bytes(&meta.biggest),
        meta.meta_offset,
        meta.commit_ts
    )
}

/// Exposed at `pub(crate)` (Go's `jsonByteSlice.MarshalJSON` is package-private
/// too) so `client_test.rs` can exercise the exact wire-array encoding used by
/// `encode_sst_meta`.
/// 将 JsonByteSlice 编码为 JSON 整数数组或 null（包内可见，供测试）。
pub(crate) fn encode_json_bytes(bytes: &JsonByteSlice) -> String {
    let Some(bytes) = &bytes.0 else {
        return "null".to_owned();
    };
    let mut output = String::from("[");
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str(&byte.to_string());
    }
    output.push(']');
    output
}

/// 从写流响应 JSON 解析 NextGenSstMeta。
pub(crate) fn decode_next_gen_response(data: &[u8]) -> Result<NextGenSstMeta, Error> {
    let root = JsonParser::new(data).parse()?;
    let response = root.object()?;
    let Some(meta_value) = response.get("sst_meta") else {
        return Ok(NextGenSstMeta::default());
    };
    if matches!(meta_value, JsonValue::Null) {
        return Ok(NextGenSstMeta::default());
    }
    let meta = meta_value.object()?;
    Ok(NextGenSstMeta {
        id: json_number_or(meta, "id", 0)?
            .try_into()
            .map_err(|_| Error::Json("id is out of range".to_owned()))?,
        smallest: json_bytes(meta, "smallest")?,
        biggest: json_bytes(meta, "biggest")?,
        meta_offset: json_number_or(meta, "meta-offset", 0)?
            .try_into()
            .map_err(|_| Error::Json("meta-offset is out of range".to_owned()))?,
        commit_ts: json_number_or(meta, "commit-ts", 0)?
            .try_into()
            .map_err(|_| Error::Json("commit-ts is out of range".to_owned()))?,
    })
}

/// 从对象取整数字段；缺失或 null 时使用 Go `encoding/json` 的字段零值。
fn json_number_or(
    object: &BTreeMap<String, JsonValue>,
    name: &str,
    default: i128,
) -> Result<i128, Error> {
    match object.get(name) {
        None | Some(JsonValue::Null) => Ok(default),
        Some(value) => value.number(),
    }
}

/// 从对象取字节数组字段（null 或整数数组）。
fn json_bytes(object: &BTreeMap<String, JsonValue>, name: &str) -> Result<JsonByteSlice, Error> {
    let Some(value) = object.get(name) else {
        return Ok(JsonByteSlice(None));
    };
    if matches!(value, JsonValue::Null) {
        return Ok(JsonByteSlice(None));
    }
    let bytes = value
        .array()?
        .iter()
        .map(|value| {
            value
                .number()?
                .try_into()
                .map_err(|_| Error::Json(format!("{name} contains a non-byte value")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(JsonByteSlice(Some(bytes)))
}

#[derive(Debug)]
/// 精简 JSON 值：仅支持 null/整数/字符串(丢内容)/数组/对象。
enum JsonValue {
    Null,
    Bool,
    Number(i128),
    Float,
    String,
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

impl JsonValue {
    fn object(&self) -> Result<&BTreeMap<String, JsonValue>, Error> {
        match self {
            Self::Object(value) => Ok(value),
            _ => Err(Error::Json("expected object".to_owned())),
        }
    }

    fn array(&self) -> Result<&[JsonValue], Error> {
        match self {
            Self::Array(value) => Ok(value),
            _ => Err(Error::Json("expected array".to_owned())),
        }
    }

    fn number(&self) -> Result<i128, Error> {
        match self {
            Self::Number(value) => Ok(*value),
            _ => Err(Error::Json("expected integer".to_owned())),
        }
    }
}

/// 手写 JSON 解析器，覆盖写流响应所需子集。
struct JsonParser<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self { input, offset: 0 }
    }

    fn parse(mut self) -> Result<JsonValue, Error> {
        let value = self.value()?;
        self.whitespace();
        if self.offset != self.input.len() {
            return Err(Error::Json("trailing JSON data".to_owned()));
        }
        Ok(value)
    }

    fn value(&mut self) -> Result<JsonValue, Error> {
        self.whitespace();
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => self.string().map(|_| JsonValue::String),
            Some(b'n') if self.take_literal(b"null") => Ok(JsonValue::Null),
            Some(b't') if self.take_literal(b"true") => Ok(JsonValue::Bool),
            Some(b'f') if self.take_literal(b"false") => Ok(JsonValue::Bool),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(Error::Json("unexpected JSON token".to_owned())),
        }
    }

    fn object(&mut self) -> Result<JsonValue, Error> {
        self.expect(b'{')?;
        let mut output = BTreeMap::new();
        self.whitespace();
        if self.consume(b'}') {
            return Ok(JsonValue::Object(output));
        }
        loop {
            let key = self.string()?;
            self.whitespace();
            self.expect(b':')?;
            output.insert(key, self.value()?);
            self.whitespace();
            if self.consume(b'}') {
                return Ok(JsonValue::Object(output));
            }
            self.expect(b',')?;
            self.whitespace();
        }
    }

    fn array(&mut self) -> Result<JsonValue, Error> {
        self.expect(b'[')?;
        let mut output = Vec::new();
        self.whitespace();
        if self.consume(b']') {
            return Ok(JsonValue::Array(output));
        }
        loop {
            output.push(self.value()?);
            self.whitespace();
            if self.consume(b']') {
                return Ok(JsonValue::Array(output));
            }
            self.expect(b',')?;
        }
    }

    fn string(&mut self) -> Result<String, Error> {
        self.expect(b'"')?;
        let mut output = String::new();
        loop {
            let byte = self
                .next()
                .ok_or_else(|| Error::Json("unterminated string".to_owned()))?;
            match byte {
                b'"' => return Ok(output),
                b'\\' => {
                    let escaped = self
                        .next()
                        .ok_or_else(|| Error::Json("unterminated escape".to_owned()))?;
                    match escaped {
                        b'"' => output.push('"'),
                        b'\\' => output.push('\\'),
                        b'/' => output.push('/'),
                        b'b' => output.push('\u{0008}'),
                        b'f' => output.push('\u{000c}'),
                        b'n' => output.push('\n'),
                        b'r' => output.push('\r'),
                        b't' => output.push('\t'),
                        b'u' => output.push(self.unicode_escape()?),
                        _ => return Err(Error::Json("invalid string escape".to_owned())),
                    }
                }
                0..=31 => return Err(Error::Json("control byte in string".to_owned())),
                32..=127 => output.push(char::from(byte)),
                _ => {
                    let start = self.offset - 1;
                    let remaining = std::str::from_utf8(&self.input[start..])
                        .map_err(|error| Error::Json(error.to_string()))?;
                    let character = remaining
                        .chars()
                        .next()
                        .ok_or_else(|| Error::Json("invalid UTF-8".to_owned()))?;
                    self.offset = start + character.len_utf8();
                    output.push(character);
                }
            }
        }
    }

    fn unicode_escape(&mut self) -> Result<char, Error> {
        if self.offset + 4 > self.input.len() {
            return Err(Error::Json("truncated unicode escape".to_owned()));
        }
        let text = std::str::from_utf8(&self.input[self.offset..self.offset + 4])
            .map_err(|error| Error::Json(error.to_string()))?;
        self.offset += 4;
        let code = u32::from_str_radix(text, 16)
            .map_err(|_| Error::Json("invalid unicode escape".to_owned()))?;
        char::from_u32(code).ok_or_else(|| Error::Json("invalid unicode scalar".to_owned()))
    }

    fn number(&mut self) -> Result<JsonValue, Error> {
        let start = self.offset;
        self.consume(b'-');
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.offset += 1;
        }
        let mut is_integer = true;
        if self.consume(b'.') {
            is_integer = false;
            while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                self.offset += 1;
            }
        }
        if self.peek().is_some_and(|byte| matches!(byte, b'e' | b'E')) {
            is_integer = false;
            self.offset += 1;
            if self.peek().is_some_and(|byte| matches!(byte, b'+' | b'-')) {
                self.offset += 1;
            }
            while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                self.offset += 1;
            }
        }
        let text = std::str::from_utf8(&self.input[start..self.offset])
            .map_err(|error| Error::Json(error.to_string()))?;
        if is_integer {
            let value = text
                .parse()
                .map_err(|_| Error::Json("invalid integer".to_owned()))?;
            Ok(JsonValue::Number(value))
        } else {
            text.parse::<f64>()
                .map_err(|_| Error::Json("invalid number".to_owned()))?;
            Ok(JsonValue::Float)
        }
    }

    fn whitespace(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.offset += 1;
        }
    }

    fn take_literal(&mut self, literal: &[u8]) -> bool {
        if self.input[self.offset..].starts_with(literal) {
            self.offset += literal.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), Error> {
        self.whitespace();
        if self.consume(byte) {
            Ok(())
        } else {
            Err(Error::Json(format!("expected byte {byte}")))
        }
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
        self.input.get(self.offset).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let value = self.peek()?;
        self.offset += 1;
        Some(value)
    }
}

/// RAII：drop 时记录 IngestAPIDuration。
struct IngestDurationGuard(Instant);

impl Drop for IngestDurationGuard {
    fn drop(&mut self) {
        if let Ok(metric) = ingestmetric::IngestAPIDuration.read()
            && let Some(metric) = metric.as_ref()
        {
            metric.observe(self.0.elapsed().as_secs_f64());
        }
    }
}

/// 记录 WriteAPIDuration 直方图样本。
fn observe_write_duration(started: Instant) {
    if let Ok(metric) = ingestmetric::WriteAPIDuration.read()
        && let Some(metric) = metric.as_ref()
    {
        metric.observe(started.elapsed().as_secs_f64());
    }
}
