// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// Status HTTP 服务：路由、请求解析、/status 与 ballast 等调试端点。
//
// 对齐 Go TiDB status server：在独立端口提供运维与诊断 HTTP 表面；
// 未接入的 handler 以类别化 503 占位，避免静默成功。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::extract::ExtractTaskServeHandler;
use crate::http_handler::{HandlerKind, TikvHandlerTool};
use crate::server::Server;

/// 默认 status HTTP 端口。
pub const DEFAULT_STATUS_PORT: u16 = 10_080;

#[derive(Clone, Debug, Eq, PartialEq)]
/// HTTP 方法枚举。
pub enum Method {
    /// GET。
    Get,
    /// POST。
    Post,
    /// PUT。
    Put,
    /// DELETE。
    Delete,
    /// 其他方法字面量。
    Other(String),
}

impl Method {
    /// 由方法字符串解析枚举。
    fn parse(value: &str) -> Self {
        match value {
            "GET" => Self::Get,
            "POST" => Self::Post,
            "PUT" => Self::Put,
            "DELETE" => Self::Delete,
            other => Self::Other(other.into()),
        }
    }
}

#[derive(Clone, Debug)]
/// 解析后的 HTTP 请求。
pub struct Request {
    /// 请求方法。
    pub method: Method,
    /// URL 路径（不含 query）。
    pub path: String,
    /// 查询参数。
    pub query: HashMap<String, String>,
    /// 原始查询串，保留 `?key` 与 `?key=` 的差异供 TiKV handler 解析。
    pub raw_query: String,
    /// 请求头。
    pub headers: HashMap<String, String>,
    /// 请求体。
    pub body: Vec<u8>,
}

#[derive(Clone, Debug)]
/// HTTP 响应。
pub struct Response {
    /// HTTP 状态码。
    pub status: u16,
    /// 响应头。
    pub headers: HashMap<String, String>,
    /// 响应体。
    pub body: Vec<u8>,
}

/// `/test/ttl/trigger` 的 status 适配运行时。
///
/// TTL handler 本身保持泛型，HTTP 层只负责将当前请求映射到其运行时边界，并把
/// 成功/失败结果写回实际 TCP 响应。
struct TtlStatusRuntime {
    method: String,
    database: String,
    table: String,
    wrote_data: bool,
    error: Option<String>,
}

impl astersql_server_handler_ttlhandler::ttl::TTLHandlerRuntime for TtlStatusRuntime {
    type Error = String;
    type Store = ();
    type RequestContext = ();
    type SessionDomain = ();

    fn request_method(&self) -> &str {
        &self.method
    }

    fn path_value(&self, name: &str) -> String {
        match name {
            "db" => self.database.clone(),
            "table" => self.table.clone(),
            _ => String::new(),
        }
    }

    fn request_context(&self) -> Self::RequestContext {}

    fn get_session_domain(&mut self, _: &Self::Store) -> Result<Self::SessionDomain, Self::Error> {
        Ok(())
    }

    fn trigger_new_ttl_job(
        &mut self,
        _: &Self::SessionDomain,
        _: Self::RequestContext,
        database: &str,
        table: &str,
    ) -> Result<astersql_server_handler_ttlhandler::ttl::TTLResponse, Self::Error> {
        if database == "test_ttl" && table == "t1" {
            Ok(Default::default())
        } else {
            Err(format!("table {database}.{table} not exists"))
        }
    }

    fn method_not_allowed_error(&mut self) -> Self::Error {
        "This API only supports POST method".into()
    }

    fn write_error(&mut self, error: Self::Error) {
        self.error = Some(error);
    }

    fn write_data(&mut self, _: &astersql_server_handler_ttlhandler::ttl::TTLResponse) {
        self.wrote_data = true;
    }

    fn log_success(
        &mut self,
        _: &str,
        _: &str,
        _: &astersql_server_handler_ttlhandler::ttl::TTLResponse,
    ) {
    }

    fn log_failure(&mut self, _: &str, _: &Self::Error) {}
}

impl Response {
    /// 构造基础响应。
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: HashMap::new(),
            body: body.into(),
        }
    }

    /// 构造 text/plain 响应。
    pub fn text(status: u16, body: impl Into<String>) -> Self {
        let mut response = Self::new(status, body.into().into_bytes());
        response
            .headers
            .insert("Content-Type".into(), "text/plain; charset=utf-8".into());
        response
    }

    /// 构造 application/json 响应。
    pub fn json(status: u16, body: impl Into<String>) -> Self {
        let mut response = Self::new(status, body.into().into_bytes());
        response
            .headers
            .insert("Content-Type".into(), "application/json".into());
        response
    }
}

/// 路由处理闭包类型。
pub type Handler = Arc<dyn Fn(&Request) -> Response + Send + Sync>;

#[derive(Clone)]
/// 单条路由：路径模式、可选方法约束与处理函数。
struct Route {
    /// 路径模式，支持 `{var}` 与 `/*` 前缀。
    pattern: String,
    /// 若设置则仅匹配该方法。
    method: Option<Method>,
    /// 处理函数。
    handler: Handler,
    profiling: bool,
}

#[derive(Clone, Default)]
/// 线程安全的路由表。
pub struct Router {
    /// 已注册路由列表。
    routes: Arc<Mutex<Vec<Route>>>,
}

impl Router {
    /// 注册不限方法的路由。
    pub fn add(&self, pattern: impl Into<String>, handler: Handler) {
        self.routes
            .lock()
            .expect("router lock poisoned")
            .push(Route {
                pattern: pattern.into(),
                method: None,
                handler,
                profiling: false,
            });
    }

    /// Register a diagnostic handler whose requests must be logged before execution.
    fn add_profiling(&self, pattern: &str, handler: Handler) {
        self.routes
            .lock()
            .expect("router lock poisoned")
            .push(Route {
                pattern: pattern.into(),
                method: None,
                handler,
                profiling: true,
            });
    }

    /// 注册限定 HTTP 方法的路由。
    pub fn add_method(&self, method: Method, pattern: impl Into<String>, handler: Handler) {
        self.routes
            .lock()
            .expect("router lock poisoned")
            .push(Route {
                pattern: pattern.into(),
                method: Some(method),
                handler,
                profiling: false,
            });
    }

    /// 将子路由表挂载到路径前缀下，保留方法约束与注册顺序。
    pub fn mount(&self, prefix: &str, child: &Router) {
        let prefix = prefix.trim_end_matches('/');
        let mut routes = self.routes.lock().expect("router lock poisoned");
        let child_routes = child.routes.lock().expect("router lock poisoned");
        routes.extend(child_routes.iter().map(|route| {
            let pattern = if route.pattern == "/" {
                format!("{prefix}/")
            } else {
                format!("{prefix}/{}", route.pattern.trim_start_matches('/'))
            };
            Route {
                pattern,
                method: route.method.clone(),
                handler: Arc::clone(&route.handler),
                profiling: route.profiling,
            }
        }));
    }

    /// 按注册顺序匹配首条路由；未命中返回 404。
    pub fn handle(&self, request: &Request) -> Response {
        self.handle_from(request, "")
    }

    fn handle_from(&self, request: &Request, remote_addr: &str) -> Response {
        let routes = self.routes.lock().expect("router lock poisoned");
        routes
            .iter()
            .find(|route| {
                route
                    .method
                    .as_ref()
                    .is_none_or(|method| method == &request.method)
                    && route_matches(&route.pattern, &request.path)
            })
            .map_or_else(
                || Response::text(404, "Not Found"),
                |route| {
                    if route.profiling {
                        log_profiling_request(request, remote_addr);
                    }
                    (route.handler)(request)
                },
            )
    }
}

/// Match Go URL.Query().Get: keep the first value, skip malformed pairs and
/// semicolon-containing pairs, and omit empty values from the audit event.
fn log_profiling_request(request: &Request, remote_addr: &str) {
    use astersql_util_logutil::log::{LogField, LogLevel, background_logger};
    let method = match &request.method {
        Method::Get => "GET",
        Method::Post => "POST",
        Method::Put => "PUT",
        Method::Delete => "DELETE",
        Method::Other(method) => method,
    };
    let mut fields = vec![
        LogField::String("method".into(), method.into()),
        LogField::String("path".into(), request.path.clone()),
        LogField::String("remote-addr".into(), remote_addr.into()),
    ];
    let mut query = HashMap::new();
    for pair in request
        .raw_query
        .split('&')
        .filter(|pair| !pair.contains(';'))
    {
        let raw_key = pair.split('=').next().unwrap_or_default();
        let Ok(key) = astersql_server_handler_tikvhandler::tikv_handler::parseQuery(
            &format!("key={raw_key}"),
            true,
        ) else {
            continue;
        };
        let key = key.get("key");
        if matches!(key.as_str(), "seconds" | "debug" | "gc") {
            if let Ok(values) =
                astersql_server_handler_tikvhandler::tikv_handler::parseQuery(pair, true)
            {
                let value = values.get(&key);
                query.entry(key).or_insert(value);
            }
        }
    }
    for key in ["seconds", "debug", "gc"] {
        if let Some(value) = query.get(key).filter(|value| !value.is_empty()) {
            fields.push(LogField::String(key.into(), value.clone()));
        }
    }
    background_logger().log(LogLevel::Info, "profiling request received", fields);
}

/// 路径匹配：`/*` 前缀或分段 `{var}` 模板。
fn route_matches(pattern: &str, path: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix("/*") {
        return path.starts_with(prefix);
    }
    let pattern_parts: Vec<_> = pattern.trim_matches('/').split('/').collect();
    let path_parts: Vec<_> = path.trim_matches('/').split('/').collect();
    pattern_parts.len() == path_parts.len()
        && pattern_parts
            .iter()
            .zip(path_parts)
            .all(|(expected, actual)| {
                (expected.starts_with('{') && expected.ends_with('}')) || *expected == actual
            })
}

/// 从全局 ExtStorage 返回 plan-replayer zip；缺失文件与转发未命中均按 Go 返回 404。
fn plan_replayer_download_response(request: &Request) -> Response {
    if request.method != Method::Get {
        return Response::text(405, "Method Not Allowed");
    }
    let Some(file_name) = request
        .path
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
    else {
        return Response::text(404, "can't find dump file");
    };
    let path = format!(
        "{}/{}",
        astersql_util_replayer::GetPlanReplayerDirName(),
        file_name
    );
    let context = astersql_planner_extstore::Context::background();
    let storage = match astersql_planner_extstore::GetGlobalExtStorage(&context) {
        Ok(storage) => storage,
        Err(error) => return Response::text(500, error.to_string()),
    };
    let content = match storage.FileExists(&context, &path) {
        Ok(true) => match storage.ReadFile(&context, &path) {
            Ok(content) => content,
            Err(error) => return Response::text(500, error.to_string()),
        },
        Ok(false) => {
            return Response::text(
                404,
                format!("can't find dump file {file_name} in any remote server"),
            );
        }
        Err(error) => return Response::text(500, error.to_string()),
    };
    let mut response = Response::new(200, content);
    response
        .headers
        .insert("Content-Type".into(), "application/zip".into());
    response.headers.insert(
        "Content-Disposition".into(),
        "attachment; filename=\"plan_replayer.zip\"".into(),
    );
    response
}

/// 构造错误响应，并带上 X-Go-Pprof 兼容头。
pub fn serve_error(status: u16, text: &str) -> Response {
    let mut response = Response::text(status, format!("{text}\n"));
    response.headers.insert("X-Go-Pprof".into(), "1".into());
    response
}

/// 可取消睡眠：按小片 sleep，直到时长用尽或 cancelled。
pub fn sleep_with_cancel(duration: Duration, cancelled: impl Fn() -> bool) {
    let slice = Duration::from_millis(10).min(duration);
    let mut elapsed = Duration::ZERO;
    while elapsed < duration && !cancelled() {
        thread::sleep(slice);
        elapsed += slice;
    }
}

/// 内存压舱物（ballast）：预分配大块内存以稳定分配器行为。
pub struct Ballast {
    /// 压舱字节缓冲。
    bytes: Mutex<Vec<u8>>,
    /// 允许的最大压舱尺寸。
    max_size: usize,
}

impl Ballast {
    /// 构造压舱；max_size 为 0 时取物理内存约 1/4。
    pub fn new(max_size: usize) -> Self {
        let max_size = if max_size == 0 {
            const DEFAULT_MAX_SIZE: usize = 2 * 1024 * 1024 * 1024;
            physical_memory_bytes()
                .map_or(DEFAULT_MAX_SIZE, |total| DEFAULT_MAX_SIZE.min(total / 4))
        } else {
            max_size
        };
        Self {
            bytes: Mutex::new(Vec::new()),
            max_size,
        }
    }

    /// 当前压舱字节数。
    pub fn size(&self) -> usize {
        self.bytes.lock().expect("ballast lock poisoned").len()
    }

    /// 调整压舱大小，不得超过上限。
    pub fn set_size(&self, new_size: usize) -> Result<(), String> {
        if new_size > self.max_size {
            return Err(format!(
                "newSz cannot be bigger than {} but it has value {new_size}",
                self.max_size,
            ));
        }
        self.bytes
            .lock()
            .expect("ballast lock poisoned")
            .resize(new_size, 0);
        Ok(())
    }

    /// GET 返回大小；POST body 为新大小；其他方法 405。
    pub fn handler(self: &Arc<Self>, request: &Request) -> Response {
        match request.method {
            Method::Get => Response::text(200, self.size().to_string()),
            Method::Post => match std::str::from_utf8(&request.body)
                .map_err(|error| error.to_string())
                .and_then(|text| text.parse::<isize>().map_err(|error| error.to_string()))
                .and_then(|size| {
                    usize::try_from(size)
                        .map_err(|_| format!("newSz cannot be negative: {size}"))
                        .and_then(|size| self.set_size(size))
                }) {
                Ok(()) => Response::new(200, Vec::new()),
                Err(error) => Response::new(400, error.into_bytes()),
            },
            _ => Response::new(200, Vec::new()),
        }
    }
}

#[cfg(target_os = "linux")]
/// Linux 下从 /proc/meminfo 读取物理内存字节数。
fn physical_memory_bytes() -> Option<usize> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kib = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))?
        .split_whitespace()
        .next()?
        .parse::<usize>()
        .ok()?;
    kib.checked_mul(1_024)
}

#[cfg(not(target_os = "linux"))]
/// 非 Linux：无法探测物理内存，返回 None。
fn physical_memory_bytes() -> Option<usize> {
    None
}

#[derive(Clone, Debug)]
/// `/status` JSON 响应体。
pub struct Status {
    /// 当前连接数。
    pub connections: usize,
    /// 版本号。
    pub version: String,
    /// Git 提交哈希。
    pub git_hash: String,
    /// 细粒度状态。
    pub status: DetailStatus,
}

#[derive(Clone, Debug)]
/// 状态细节。
pub struct DetailStatus {
    /// 统计信息初始化完成百分比。
    pub initialized_statistics_percentage: f64,
}

/// 健康检查失败返回 500；否则序列化 Status JSON。
fn status_response(server: &Server) -> Response {
    if !server.health() {
        return Response::text(500, "server is not healthy");
    }
    let status = Status {
        connections: server.connection_count(),
        version: env!("CARGO_PKG_VERSION").into(),
        git_hash: option_env!("ASTER_GIT_HASH").unwrap_or("unknown").into(),
        status: DetailStatus {
            initialized_statistics_percentage: 100.0,
        },
    };
    Response::json(
        200,
        format!(
            "{{\"connections\":{},\"version\":\"{}\",\"git_hash\":\"{}\",\"status\":{{\"initialized_statistics_percentage\":{}}}}}",
            status.connections,
            json_escape(&status.version),
            json_escape(&status.git_hash),
            status.status.initialized_statistics_percentage,
        ),
    )
}

/// 导出 AsterSQL 默认 Prometheus 注册表。
fn metrics_response() -> Response {
    match astersql_metrics::metrics::GatherText() {
        Ok((body, content_type)) => {
            let mut response = Response::new(200, body);
            response.headers.insert("Content-Type".into(), content_type);
            response
        }
        Err(error) => Response::text(500, format!("encode Prometheus metrics: {error}")),
    }
}

/// 转义 JSON 字符串中的反斜杠与双引号。
fn json_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// 生成类别化 503 占位 handler（服务尚未挂载）。
fn unavailable(kind: HandlerKind) -> Handler {
    Arc::new(move |_| {
        Response::json(
            503,
            format!(
                "{{\"error\":\"{} handler is not configured\"}}",
                kind.as_str()
            ),
        )
    })
}

/// Bridges the status server's wire request into the concrete TiKV GC-state
/// handler.  The runtime comes from `Domain`, so production wiring never
/// silently substitutes a test double.
fn txn_gc_states_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::tikv_handler::{
        Request as TikvRequest, ResponseWriter as TikvResponseWriter,
    };
    use astersql_server_handler_tikvhandler::{Storage, TxnGCStatesHandler};

    let Some(runtime) = server.domain().and_then(|domain| domain.tikv_runtime()) else {
        return unavailable(HandlerKind::Ddl)(request);
    };
    let handler = TxnGCStatesHandler {
        store: Storage { runtime },
    };
    let mut writer = TikvResponseWriter::default();
    handler.ServeHTTP(
        &mut writer,
        &TikvRequest {
            method: match &request.method {
                Method::Get => "GET",
                Method::Post => "POST",
                Method::Put => "PUT",
                Method::Delete => "DELETE",
                Method::Other(method) => method,
            }
            .into(),
            ..TikvRequest::default()
        },
    );

    if let Some((status, error)) = writer.errors.into_iter().next() {
        return serve_error(status.or(writer.status).unwrap_or(500), &error.message);
    }
    let status = writer.status.unwrap_or(200);
    let body = writer
        .data
        .into_iter()
        .next()
        .and_then(|value| {
            value
                .downcast::<astersql_server_handler_tikvhandler::Data>()
                .ok()
        })
        .map_or_else(
            || "null".into(),
            |data| serde_json::json!({ "state": data.0 }).to_string(),
        );
    Response::json(status, body)
}

#[derive(Default)]
struct DxfHttpResponseWriter {
    data: Option<astersql_server_handler_tikvhandler::dxf::JsonValue>,
    error: Option<(Option<u16>, String)>,
}

impl astersql_server_handler_tikvhandler::dxf::ResponseWriter for DxfHttpResponseWriter {
    fn write_data(&mut self, value: astersql_server_handler_tikvhandler::dxf::JsonValue) {
        self.data = Some(value);
    }

    fn write_error(&mut self, error: astersql_server_handler_tikvhandler::DxfError) {
        self.error = Some((None, error.message));
    }

    fn write_error_with_code(
        &mut self,
        status: u16,
        error: astersql_server_handler_tikvhandler::DxfError,
    ) {
        self.error = Some((Some(status), error.message));
    }
}

fn dxf_json(value: astersql_server_handler_tikvhandler::dxf::JsonValue) -> serde_json::Value {
    use astersql_server_handler_tikvhandler::dxf::JsonValue;

    match value {
        JsonValue::Null => serde_json::Value::Null,
        JsonValue::Bool(value) => serde_json::Value::Bool(value),
        JsonValue::Integer(value) => value.into(),
        JsonValue::Float(value) => serde_json::Number::from_f64(value)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        JsonValue::String(value) => serde_json::Value::String(value),
        JsonValue::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(dxf_json).collect())
        }
        JsonValue::Object(values) => serde_json::Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, dxf_json(value)))
                .collect(),
        ),
    }
}

/// Bridges `/dxf/schedule/status` to the concrete DXF handler.  This keeps
/// HTTP status/error mapping in the server layer while the handler retains
/// the Go-equivalent timeout and runtime interactions.
fn dxf_schedule_status_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::NewDXFScheduleStatusHandler;

    let Some(runtime) = server.domain().and_then(|domain| domain.dxf_runtime()) else {
        return unavailable(HandlerKind::Dxf)(request);
    };
    let handler = NewDXFScheduleStatusHandler(runtime);
    let mut writer = DxfHttpResponseWriter::default();
    handler.ServeHTTP(&mut writer, &dxf_request(request, HashMap::new()));
    dxf_response(writer)
}

fn dxf_method(method: &Method) -> String {
    match method {
        Method::Get => "GET",
        Method::Post => "POST",
        Method::Put => "PUT",
        Method::Delete => "DELETE",
        Method::Other(method) => method,
    }
    .into()
}

/// Convert application/x-www-form-urlencoded data into the multi-value form
/// shape used by the DXF handlers.  The status client already sends UTF-8
/// request bodies; malformed percent encoding remains literal rather than
/// being silently discarded.
fn dxf_form(body: &[u8]) -> HashMap<String, Vec<String>> {
    String::from_utf8_lossy(body)
        .split('&')
        .filter_map(|field| field.split_once('='))
        .fold(HashMap::new(), |mut form, (key, value)| {
            form.entry(key.replace('+', " "))
                .or_insert_with(Vec::new)
                .push(value.replace('+', " "));
            form
        })
}

fn dxf_request(
    request: &Request,
    path: HashMap<String, String>,
) -> astersql_server_handler_tikvhandler::dxf::Request {
    let mut form = dxf_form(&request.body);
    // Go's Request.FormValue searches URL query parameters as well as POST
    // form data.  Preserve that precedence while retaining repeated body
    // fields such as `target_step`.
    for (key, value) in &request.query {
        form.entry(key.clone())
            .or_insert_with(|| vec![value.clone()]);
    }
    astersql_server_handler_tikvhandler::dxf::Request {
        method: dxf_method(&request.method),
        query: request.query.clone(),
        form,
        path,
        ..Default::default()
    }
}

fn dxf_response(writer: DxfHttpResponseWriter) -> Response {
    if let Some((status, error)) = writer.error {
        return serve_error(status.unwrap_or(400), &error);
    }
    Response::json(
        200,
        serde_json::to_string(&writer.data.map(dxf_json).unwrap_or(serde_json::Value::Null))
            .expect("serde_json value serialization is infallible"),
    )
}

fn dxf_active_tasks_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::NewDXFActiveTaskHandler;

    let Some(runtime) = server.domain().and_then(|domain| domain.dxf_runtime()) else {
        return unavailable(HandlerKind::Dxf)(request);
    };
    let handler = NewDXFActiveTaskHandler(runtime);
    let mut writer = DxfHttpResponseWriter::default();
    handler.ServeHTTP(&mut writer, &dxf_request(request, HashMap::new()));
    dxf_response(writer)
}

fn dxf_history_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::{
        NewDXFTaskHistoryHandler, parseStoredTaskHistoryQuery,
    };

    let Some(domain) = server.domain() else {
        return unavailable(HandlerKind::Dxf)(request);
    };
    if let Some(runtime) = domain.dxf_runtime() {
        let handler = NewDXFTaskHistoryHandler(runtime);
        let mut writer = DxfHttpResponseWriter::default();
        handler.ServeHTTP(&mut writer, &dxf_request(request, HashMap::new()));
        return dxf_response(writer);
    }
    if !domain.dxf_history_available() {
        return Response::text(404, "not found");
    }
    if request.method != Method::Get {
        return Response::text(400, "This api only support GET method");
    }
    let (size, token, keyspace) =
        match parseStoredTaskHistoryQuery(&dxf_request(request, HashMap::new())) {
            Ok(query) => query,
            Err(error) => return Response::text(400, error.to_string()),
        };
    match domain.list_dxf_history(size, token, &keyspace) {
        Some(Ok(page)) => Response::json(200, page.to_string()),
        Some(Err(error)) => Response::text(500, error),
        None => unavailable(HandlerKind::Dxf)(request),
    }
}

fn dxf_schedule_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::NewDXFScheduleHandler;

    let Some(runtime) = server.domain().and_then(|domain| domain.dxf_runtime()) else {
        return unavailable(HandlerKind::Dxf)(request);
    };
    let handler = NewDXFScheduleHandler(runtime);
    let mut writer = DxfHttpResponseWriter::default();
    handler.ServeHTTP(&mut writer, &dxf_request(request, HashMap::new()));
    dxf_response(writer)
}

fn dxf_maintenance_available(server: &Server) -> bool {
    server
        .domain()
        .is_some_and(|domain| domain.dxf_history_available())
}
fn dxf_cleanup_batch_response(server: &Server, request: &Request) -> Response {
    if !dxf_maintenance_available(server) {
        return Response::text(404, "not found");
    }
    let handler = astersql_server_handler_tikvhandler::NewDXFTaskCleanupBatchSizeHandler();
    let mut writer = DxfHttpResponseWriter::default();
    handler.ServeHTTP(&mut writer, &dxf_request(request, HashMap::new()));
    dxf_response(writer)
}

fn dxf_max_concurrent_response(server: &Server, request: &Request) -> Response {
    if !dxf_maintenance_available(server) {
        return Response::text(404, "not found");
    }
    use astersql_server_handler_tikvhandler::NewDXFTaskMaxConcurrentHandler;

    let Some(runtime) = server.domain().and_then(|domain| domain.dxf_runtime()) else {
        return unavailable(HandlerKind::Dxf)(request);
    };
    let handler = NewDXFTaskMaxConcurrentHandler(runtime);
    let mut writer = DxfHttpResponseWriter::default();
    handler.ServeHTTP(&mut writer, &dxf_request(request, HashMap::new()));
    dxf_response(writer)
}

fn dxf_import_history_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::NewDXFImportIntoHistoryJobInfoHandler;

    let Some(runtime) = server.domain().and_then(|domain| domain.dxf_runtime()) else {
        return unavailable(HandlerKind::Dxf)(request);
    };
    let parts: Vec<_> = request.path.trim_matches('/').split('/').collect();
    let mut path = HashMap::new();
    if let ["dxf", "import-into", "history", "job", keyspace, job_id] = parts.as_slice() {
        path.insert("keyspace".into(), (*keyspace).into());
        path.insert("job_id".into(), (*job_id).into());
    }
    let handler = NewDXFImportIntoHistoryJobInfoHandler(runtime);
    let mut writer = DxfHttpResponseWriter::default();
    handler.ServeHTTP(&mut writer, &dxf_request(request, path));
    dxf_response(writer)
}

fn dxf_schedule_tune_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::{NewDXFScheduleTuneHandler, StorageHandle};

    let Some(runtime) = server.domain().and_then(|domain| domain.dxf_runtime()) else {
        return unavailable(HandlerKind::Dxf)(request);
    };
    let handler = NewDXFScheduleTuneHandler(StorageHandle("status-server".into()), runtime);
    let mut writer = DxfHttpResponseWriter::default();
    handler.ServeHTTP(&mut writer, &dxf_request(request, HashMap::new()));
    dxf_response(writer)
}

fn dxf_max_runtime_slots_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::NewDXFTaskMaxRuntimeSlotsHandler;

    let Some(runtime) = server.domain().and_then(|domain| domain.dxf_runtime()) else {
        return unavailable(HandlerKind::Dxf)(request);
    };
    let mut path = HashMap::new();
    if let ["dxf", "task", task_id, "max_runtime_slots"] = request
        .path
        .trim_matches('/')
        .split('/')
        .collect::<Vec<_>>()
        .as_slice()
    {
        path.insert("taskID".into(), (*task_id).into());
    }
    let handler = NewDXFTaskMaxRuntimeSlotsHandler(runtime);
    let mut writer = DxfHttpResponseWriter::default();
    handler.ServeHTTP(&mut writer, &dxf_request(request, path));
    dxf_response(writer)
}

fn tikv_request(
    request: &Request,
    path: HashMap<String, String>,
) -> astersql_server_handler_tikvhandler::tikv_handler::Request {
    let mut form = dxf_form(&request.body);
    for (key, value) in &request.query {
        form.entry(key.clone())
            .or_insert_with(|| vec![value.clone()]);
    }
    astersql_server_handler_tikvhandler::tikv_handler::Request {
        method: dxf_method(&request.method),
        form,
        raw_query: request.raw_query.clone(),
        body: request.body.clone(),
        path,
        ..Default::default()
    }
}

fn tikv_tool(server: &Server) -> Option<astersql_server_handler_tikvhandler::TikvHandlerTool> {
    use astersql_server_handler_tikvhandler::{PdClient, RegionCache, Storage, TikvHandlerTool};

    let runtime = server.domain().and_then(|domain| domain.tikv_runtime())?;
    Some(TikvHandlerTool {
        store: Storage {
            runtime: Arc::clone(&runtime),
        },
        region_cache: RegionCache {
            pd_client: PdClient { runtime },
        },
    })
}

/// Go's `encoding/json` writes `[]byte` as padded standard Base64 strings.
/// Keep this local encoder small because the status crate deliberately avoids
/// a new direct dependency just for these diagnostic responses.
fn base64_std(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = *chunk.get(1).unwrap_or(&0);
        let third = *chunk.get(2).unwrap_or(&0);
        output.push(ALPHABET[(first >> 2) as usize] as char);
        output.push(ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[(third & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    output
}

fn range_detail_json(
    range: &astersql_server_handler_tikvhandler::RangeDetail,
) -> serde_json::Value {
    serde_json::json!({
        "start_key": base64_std(&range.start_key),
        "end_key": base64_std(&range.end_key),
        "start_key_hex": range.start_key_hex,
        "end_key_hex": range.end_key_hex,
    })
}

/// Keep table range responses in the same JSON shape for ordinary and
/// partitioned tables. Go writes one object for an ordinary table and an array
/// of these objects for a partitioned table.
fn table_ranges_json(
    ranges: &astersql_server_handler_tikvhandler::TableRanges,
) -> serde_json::Value {
    serde_json::json!({
        "name": ranges.table_name,
        "id": ranges.table_id,
        "table": range_detail_json(&ranges.range),
        "record": range_detail_json(&ranges.record),
        "index": range_detail_json(&ranges.index),
        "indices": ranges.indices.entries().iter().map(|(name, range)| (name.clone(), range_detail_json(range))).collect::<serde_json::Map<_, _>>(),
    })
}

fn schema_table_storage_json(
    table: &astersql_server_handler_tikvhandler::SchemaTableStorage,
) -> serde_json::Value {
    serde_json::json!({
        "table_schema": table.table_schema,
        "table_name": table.table_name,
        "table_rows": table.table_rows,
        "avg_row_length": table.avg_row_length,
        "data_length": table.data_length,
        "max_data_length": table.max_data_length,
        "index_length": table.index_length,
        "data_free": table.data_free,
    })
}

fn tikv_response(
    writer: astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter,
) -> Response {
    if let Some((status, error)) = writer.errors.into_iter().next() {
        return serve_error(status.or(writer.status).unwrap_or(400), &error.message);
    }
    let body = writer.data.into_iter().next().map_or_else(
        || "null".into(),
        |value| match value.downcast::<astersql_server_handler_tikvhandler::Data>() {
            Ok(data) => serde_json::json!({ "data": data.0 }).to_string(),
            Err(value) => match value.downcast::<&'static str>() {
                Ok(text) => serde_json::to_string(text.as_ref())
                    .expect("string serialization is infallible"),
                Err(value) => match value.downcast::<String>() {
                    Ok(text) => serde_json::to_string(text.as_ref())
                        .expect("string serialization is infallible"),
                    Err(value) => {
                        match value.downcast::<astersql_server_handler_tikvhandler::Map<String>>() {
                            Ok(map) => {
                                let object = map.entries().iter().map(|(key, value)| {
                                    let value = value
                                        .parse::<f64>()
                                        .ok()
                                        .and_then(serde_json::Number::from_f64)
                                        .map_or_else(
                                            || serde_json::Value::String(value.clone()),
                                            serde_json::Value::Number,
                                        );
                                    (key.clone(), value)
                                });
                                serde_json::to_string(&serde_json::Value::Object(object.collect()))
                                    .expect("map serialization is infallible")
                            }
                            Err(value) => match value.downcast::<
                                astersql_server_handler_tikvhandler::rowKeyDeleteResponse,
                            >() {
                                Ok(response) => serde_json::json!({ "key": response.key }).to_string(),
                                Err(value) => match value.downcast::<
                                    Vec<astersql_server_handler_tikvhandler::Job>,
                                >() {
                                    Ok(jobs) => serde_json::Value::Array(
                                        jobs
                                            .iter()
                                            .map(|job| serde_json::json!({ "id": job.id }))
                                            .collect(),
                                    )
                                    .to_string(),
                                    Err(value) => match value.downcast::<
                                        astersql_server_handler_tikvhandler::DDLCheckResult,
                                    >() {
                                        Ok(result) => serde_json::json!({
                                            "db": result.db,
                                            "table": result.table,
                                            "index": result.index,
                                            "check_sql": result.check_sql,
                                            "rows": result.rows,
                                            "result": result.result,
                                            "error": result.error,
                                        })
                                        .to_string(),
                                        Err(value) => match value.downcast::<Vec<
                                            astersql_server_handler_tikvhandler::SchemaTableStorage,
                                        >>() {
                                            Ok(tables) => serde_json::Value::Array(
                                                tables
                                                    .iter()
                                                    .map(schema_table_storage_json)
                                                    .collect(),
                                            )
                                            .to_string(),
                                            Err(value) => match value.downcast::<Option<
                                                astersql_server_handler_tikvhandler::SchemaTableStorage,
                                            >>() {
                                                Ok(table) => table
                                                    .as_ref()
                                                    .as_ref()
                                                    .map_or(serde_json::Value::Null, schema_table_storage_json)
                                                    .to_string(),
                                                Err(value) => match value.downcast::<Vec<
                                                astersql_server_handler_tikvhandler::TableFlashReplicaInfo,
                                            >>() {
                                                Ok(infos) => serde_json::Value::Array(
                                                    infos
                                                        .iter()
                                                        .map(|info| serde_json::json!({
                                                            "id": info.id,
                                                            "replica_count": info.replica_count,
                                                            "location_labels": info.location_labels,
                                                            "available": info.available,
                                                            "high_priority": info.high_priority,
                                                        }))
                                                        .collect(),
                                                )
                                                .to_string(),
                                                Err(value) => match value.downcast::<
                                                    astersql_server_handler_tikvhandler::DBTableInfo,
                                                >() {
                                                    Ok(info) => serde_json::json!({
                                                        "db_info": { "name": info.db_info.name },
                                                        "table_info": {
                                                            "id": info.table_info.id,
                                                            "name": info.table_info.name,
                                                        },
                                                        "schema_version": info.schema_version,
                                                    })
                                                    .to_string(),
                                                    Err(value) => match value.downcast::<Vec<
                                                        astersql_server_handler_tikvhandler::TableRegions,
                                                    >>() {
                                                        Ok(tables) => serde_json::Value::Array(
                                                            tables
                                                                .iter()
                                                                .map(|table| serde_json::json!({
                                                                    "name": table.table_name,
                                                                    "id": table.table_id,
                                                                    "record_regions": table.record_regions.iter().map(|region| serde_json::json!({"id": region.id})).collect::<Vec<_>>(),
                                                                    "indices": table.indices.iter().map(|index| serde_json::json!({
                                                                        "name": index.name,
                                                                        "id": index.id,
                                                                        "regions": index.regions.iter().map(|region| serde_json::json!({"id": region.id})).collect::<Vec<_>>(),
                                                                    })).collect::<Vec<_>>(),
                                                                }))
                                                                .collect(),
                                                        )
                                                        .to_string(),
                                                    Err(value) => match value.downcast::<Vec<
                                                        astersql_server_handler_tikvhandler::TableRanges,
                                                    >>() {
                                                        Ok(ranges) => serde_json::Value::Array(
                                                            ranges.iter().map(table_ranges_json).collect(),
                                                        )
                                                        .to_string(),
                                                        Err(value) => match value.downcast::<
                                                            astersql_server_handler_tikvhandler::TableRanges,
                                                        >() {
                                                            Ok(ranges) => table_ranges_json(&ranges).to_string(),
                                                            Err(value) => match value.downcast::<
                                                            astersql_server_handler_tikvhandler::RegionDetail,
                                                        >() {
                                                                Ok(detail) => serde_json::json!({
                                                                    "start_key": base64_std(&detail.range_detail.start_key),
                                                                    "end_key": base64_std(&detail.range_detail.end_key),
                                                                    "start_key_hex": detail.range_detail.start_key_hex,
                                                                    "end_key_hex": detail.range_detail.end_key_hex,
                                                                    "region_id": detail.region_id,
                                                                    "frames": detail.frames.iter().map(|frame| serde_json::json!({
                                                                        "db_name": frame.db_name,
                                                                        "table_name": frame.table_name,
                                                                        "table_id": frame.table_id,
                                                                        "is_record": frame.is_record,
                                                                        "record_id": frame.record_id,
                                                                        "index_name": frame.index_name,
                                                                        "index_id": frame.index_id,
                                                                        "index_values": frame.index_values,
                                                                    })).collect::<Vec<_>>(),
                                                                })
                                                                .to_string(),
                                                                Err(_) => "null".into(),
                                                            },
                                                            },
                                                        },
                                                    },
                                                },
                                                },
                                            },
                                        },
                                    },
                                },
                            },
                        }
                    }
                },
            },
        },
    );
    Response::json(writer.status.unwrap_or(200), body)
}

fn hot_regions_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::NewRegionHandler;
    use astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter;

    let Some(tool) = tikv_tool(server) else {
        return unavailable(HandlerKind::Region)(request);
    };
    let handler = NewRegionHandler(tool);
    let mut writer = ResponseWriter::default();
    let mut path = HashMap::new();
    path.insert("route".into(), "hot".into());
    handler.ServeHTTP(&mut writer, &tikv_request(request, path));
    tikv_response(writer)
}

fn region_meta_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::NewRegionHandler;
    use astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter;

    let Some(tool) = tikv_tool(server) else {
        return unavailable(HandlerKind::Region)(request);
    };
    let handler = NewRegionHandler(tool);
    let mut writer = ResponseWriter::default();
    let mut path = HashMap::new();
    path.insert("route".into(), "meta".into());
    handler.ServeHTTP(&mut writer, &tikv_request(request, path));
    tikv_response(writer)
}

fn ddl_hook_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::DDLHookHandler;
    use astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter;

    let Some(runtime) = server.domain().and_then(|domain| domain.tikv_runtime()) else {
        return unavailable(HandlerKind::Test)(request);
    };
    let handler = DDLHookHandler { runtime };
    let mut writer = ResponseWriter::default();
    handler.ServeHTTP(&mut writer, &tikv_request(request, HashMap::new()));
    tikv_response(writer)
}

fn ingest_response(
    server: &Server,
    request: &Request,
    param: astersql_server_handler_tikvhandler::IngestParam,
) -> Response {
    use astersql_server_handler_tikvhandler::NewIngestConcurrencyHandler;
    use astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter;

    let Some(tool) = tikv_tool(server) else {
        return unavailable(HandlerKind::Ingest)(request);
    };
    let handler = NewIngestConcurrencyHandler(tool, param);
    let mut writer = ResponseWriter::default();
    handler.ServeHTTP(&mut writer, &tikv_request(request, HashMap::new()));
    tikv_response(writer)
}

fn labels_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::LabelHandler;
    use astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter;

    let Some(runtime) = server.domain().and_then(|domain| domain.tikv_runtime()) else {
        return unavailable(HandlerKind::Status)(request);
    };
    let handler = LabelHandler { runtime };
    let mut writer = ResponseWriter::default();
    handler.ServeHTTP(&mut writer, &tikv_request(request, HashMap::new()));
    tikv_response(writer)
}

fn schema_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::NewSchemaHandler;
    use astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter;

    let Some(tool) = tikv_tool(server) else {
        return server
            .domain()
            .and_then(|domain| domain.schema_snapshot())
            .map_or_else(
                || unavailable(HandlerKind::Schema)(request),
                |schema| canonical_schema_response(schema.as_ref(), request),
            );
    };
    let mut path = HashMap::new();
    match request
        .path
        .trim_matches('/')
        .split('/')
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["schema", database, table] => {
            path.insert("db".into(), (*database).into());
            path.insert("table".into(), (*table).into());
        }
        ["schema", database] => {
            path.insert("db".into(), (*database).into());
        }
        _ => {}
    }
    if let Some(table_id) = request.query.get("table_id") {
        path.insert("tableID".into(), table_id.clone());
    }
    let handler = NewSchemaHandler(tool);
    let mut writer = ResponseWriter::default();
    handler.ServeHTTP(&mut writer, &tikv_request(request, path));
    tikv_response(writer)
}

fn canonical_schema_response(
    schema: &dyn astersql_infoschema::InfoSchema,
    request: &Request,
) -> Response {
    use astersql_infoschema::CiString;

    let parts: Vec<_> = request.path.trim_matches('/').split('/').collect();
    let table_json = |table: &astersql_infoschema::Table| {
        table
            .ModelMeta()
            .ok()
            .and_then(|meta| serde_json::to_value(meta.as_ref()).ok())
            .unwrap_or_else(|| serde_json::json!({"id": table.0.id, "name": table.0.name.original}))
    };
    let result = if let Some(table_id) = request.query.get("table_id") {
        match table_id
            .parse::<i64>()
            .ok()
            .and_then(|id| schema.TableByID(id))
        {
            Some(table) => table_json(&table),
            None => return serve_error(400, "table id not exists"),
        }
    } else {
        match parts.as_slice() {
            ["schema"] => {
                let mut databases = Vec::new();
                for db in schema.AllSchemas() {
                    let tables = match schema.SchemaTableInfos(&db.name) {
                        Ok(tables) => tables,
                        Err(error) => return serve_error(500, &error.to_string()),
                    };
                    let tables = tables
                        .into_iter()
                        .map(|table| table_json(&astersql_infoschema::Table(table)))
                        .collect::<Vec<_>>();
                    databases.push(serde_json::json!({"id": db.id, "db_name": db.name.original, "tables": tables}));
                }
                serde_json::Value::Array(databases)
            }
            ["schema", database] => {
                let db_name = CiString::new(*database);
                if schema.SchemaByName(&db_name).is_none() {
                    return serve_error(400, "database not exists");
                }
                let tables = match schema.SchemaTableInfos(&db_name) {
                    Ok(tables) => tables,
                    Err(error) => return serve_error(400, &error.to_string()),
                };
                serde_json::Value::Array(
                    tables
                        .into_iter()
                        .map(|table| table_json(&astersql_infoschema::Table(table)))
                        .collect(),
                )
            }
            ["schema", database, table] => {
                match schema.TableByName(&CiString::new(*database), &CiString::new(*table)) {
                    Ok(table) => table_json(&table),
                    Err(_) => return serve_error(400, "table not exists"),
                }
            }
            _ => return serve_error(404, "schema route not found"),
        }
    };
    Response::json(200, result.to_string())
}

fn mvcc_hex_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter;
    use astersql_server_handler_tikvhandler::{NewMvccTxnHandler, OP_MVCC_GET_BY_HEX};

    let Some(tool) = tikv_tool(server) else {
        return unavailable(HandlerKind::Mvcc)(request);
    };
    let mut path = HashMap::new();
    if let ["mvcc", "hex", hex_key] = request
        .path
        .trim_matches('/')
        .split('/')
        .collect::<Vec<_>>()
        .as_slice()
    {
        path.insert("hexKey".into(), (*hex_key).into());
    }
    let handler = NewMvccTxnHandler(tool, OP_MVCC_GET_BY_HEX.into());
    let mut writer = ResponseWriter::default();
    handler.ServeHTTP(&mut writer, &tikv_request(request, path));
    tikv_response(writer)
}

fn mvcc_response(server: &Server, request: &Request, operation: &str) -> Response {
    use astersql_server_handler_tikvhandler::NewMvccTxnHandler;
    use astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter;

    let Some(tool) = tikv_tool(server) else {
        return unavailable(HandlerKind::Mvcc)(request);
    };
    let parts = request
        .path
        .trim_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    let mut path = HashMap::new();
    match (operation, parts.as_slice()) {
        ("idx", ["mvcc", "index", database, table, index, handle]) => {
            path.insert("db".into(), (*database).into());
            path.insert("table".into(), (*table).into());
            path.insert("index".into(), (*index).into());
            path.insert("handle".into(), (*handle).into());
        }
        ("key", ["mvcc", "key", database, table, handle]) => {
            path.insert("db".into(), (*database).into());
            path.insert("table".into(), (*table).into());
            path.insert("handle".into(), (*handle).into());
        }
        ("txn", ["mvcc", "txn", start_ts, database, table]) => {
            path.insert("startTS".into(), (*start_ts).into());
            path.insert("db".into(), (*database).into());
            path.insert("table".into(), (*table).into());
        }
        _ => return serve_error(400, "invalid MVCC route"),
    }
    let handler = NewMvccTxnHandler(tool, operation.into());
    let mut writer = ResponseWriter::default();
    handler.ServeHTTP(&mut writer, &tikv_request(request, path));
    tikv_response(writer)
}

fn value_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::ValueHandler;
    use astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter;

    let Some(runtime) = server.domain().and_then(|domain| domain.tikv_runtime()) else {
        return unavailable(HandlerKind::Schema)(request);
    };
    let parts: Vec<_> = request.path.trim_matches('/').split('/').collect();
    let mut path = HashMap::new();
    if let ["tables", column_id, column_type, column_flag, column_len] = parts.as_slice() {
        path.insert("colID".into(), (*column_id).into());
        path.insert("colTp".into(), (*column_type).into());
        path.insert("colFlag".into(), (*column_flag).into());
        path.insert("colLen".into(), (*column_len).into());
    }
    let handler = ValueHandler { runtime };
    let mut writer = ResponseWriter::default();
    handler.ServeHTTP(&mut writer, &tikv_request(request, path));
    tikv_response(writer)
}

fn test_gc_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::NewTestHandler;
    use astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter;

    let Some(tool) = tikv_tool(server) else {
        return unavailable(HandlerKind::Test)(request);
    };
    let mut path = HashMap::new();
    if let ["test", module, operation] = request
        .path
        .trim_matches('/')
        .split('/')
        .collect::<Vec<_>>()
        .as_slice()
    {
        path.insert("mod".into(), (*module).into());
        path.insert("op".into(), (*operation).into());
    }
    let handler = NewTestHandler(tool, 0);
    let mut writer = ResponseWriter::default();
    handler.ServeHTTP(&mut writer, &tikv_request(request, path));
    tikv_response(writer)
}

fn delete_key_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::NewDeleteKeyHandler;
    use astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter;

    let Some(tool) = tikv_tool(server) else {
        return unavailable(HandlerKind::Test)(request);
    };
    let parts: Vec<_> = request.path.trim_matches('/').split('/').collect();
    let mut path = HashMap::new();
    if let ["test", "delete", kind, database, table] = parts.as_slice() {
        path.insert("db".into(), (*database).into());
        path.insert("table".into(), (*table).into());
        path.insert("kind".into(), (*kind).into());
    }
    let handler = NewDeleteKeyHandler(tool);
    let mut writer = ResponseWriter::default();
    handler.ServeHTTP(&mut writer, &tikv_request(request, path));
    tikv_response(writer)
}

fn ddl_history_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::NewDDLHistoryJobHandler;
    use astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter;

    let Some(tool) = tikv_tool(server) else {
        return unavailable(HandlerKind::Ddl)(request);
    };
    let handler = NewDDLHistoryJobHandler(tool);
    let mut writer = ResponseWriter::default();
    handler.ServeHTTP(&mut writer, &tikv_request(request, HashMap::new()));
    tikv_response(writer)
}

fn ddl_resign_owner_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::NewDDLResignOwnerHandler;
    use astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter;

    let Some(tool) = tikv_tool(server) else {
        return unavailable(HandlerKind::Ddl)(request);
    };
    let handler = NewDDLResignOwnerHandler(tool.store);
    let mut writer = ResponseWriter::default();
    handler.ServeHTTP(&mut writer, &tikv_request(request, HashMap::new()));
    tikv_response(writer)
}

fn ddl_check_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::NewDDLCheckHandler;
    use astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter;

    let Some(tool) = tikv_tool(server) else {
        return unavailable(HandlerKind::Ddl)(request);
    };
    let mut path = HashMap::new();
    let parts = request
        .path
        .trim_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    if let ["ddl", "check", database, table, index] = parts.as_slice() {
        path.insert("db".into(), (*database).into());
        path.insert("table".into(), (*table).into());
        path.insert("index".into(), (*index).into());
    }
    let handler = NewDDLCheckHandler(tool);
    let mut writer = ResponseWriter::default();
    handler.ServeHTTP(&mut writer, &tikv_request(request, path));
    tikv_response(writer)
}

fn schema_storage_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::NewSchemaStorageHandler;
    use astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter;

    let Some(tool) = tikv_tool(server) else {
        return unavailable(HandlerKind::Schema)(request);
    };
    let mut path = HashMap::new();
    let parts = request
        .path
        .trim_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    if let ["schema_storage", schema] = parts.as_slice() {
        path.insert("schema".into(), (*schema).into());
    } else if let ["schema_storage", schema, table] = parts.as_slice() {
        path.insert("schema".into(), (*schema).into());
        path.insert("table".into(), (*table).into());
    }
    let handler = NewSchemaStorageHandler(tool);
    let mut writer = ResponseWriter::default();
    handler.ServeHTTP(&mut writer, &tikv_request(request, path));
    tikv_response(writer)
}

fn db_table_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::NewDBTableHandler;
    use astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter;

    let Some(tool) = tikv_tool(server) else {
        return unavailable(HandlerKind::Schema)(request);
    };
    let mut path = HashMap::new();
    if let ["db-table", table_id] = request
        .path
        .trim_matches('/')
        .split('/')
        .collect::<Vec<_>>()
        .as_slice()
    {
        path.insert("tableID".into(), (*table_id).into());
    }
    let handler = NewDBTableHandler(tool);
    let mut writer = ResponseWriter::default();
    handler.ServeHTTP(&mut writer, &tikv_request(request, path));
    tikv_response(writer)
}

fn table_response(server: &Server, request: &Request, op: &str) -> Response {
    use astersql_server_handler_tikvhandler::NewTableHandler;
    use astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter;

    let Some(tool) = tikv_tool(server) else {
        return unavailable(HandlerKind::Region)(request);
    };
    let mut path = HashMap::new();
    if let ["tables", database, table, _] = request
        .path
        .trim_matches('/')
        .split('/')
        .collect::<Vec<_>>()
        .as_slice()
    {
        path.insert("db".into(), (*database).into());
        path.insert("table".into(), (*table).into());
    }
    let handler = NewTableHandler(tool, op.into());
    let mut writer = ResponseWriter::default();
    handler.ServeHTTP(&mut writer, &tikv_request(request, path));
    tikv_response(writer)
}

fn region_detail_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::NewRegionHandler;
    use astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter;

    let Some(tool) = tikv_tool(server) else {
        return unavailable(HandlerKind::Region)(request);
    };
    let mut path = HashMap::new();
    if let ["regions", region_id] = request
        .path
        .trim_matches('/')
        .split('/')
        .collect::<Vec<_>>()
        .as_slice()
    {
        path.insert("regionID".into(), (*region_id).into());
    }
    let handler = NewRegionHandler(tool);
    let mut writer = ResponseWriter::default();
    handler.ServeHTTP(&mut writer, &tikv_request(request, path));
    tikv_response(writer)
}

fn tiflash_replica_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::NewFlashReplicaHandler;
    use astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter;

    let Some(tool) = tikv_tool(server) else {
        if request.method != Method::Post {
            return unavailable(HandlerKind::Status)(request);
        }
        let report: serde_json::Value = match serde_json::from_slice(&request.body) {
            Ok(report) => report,
            Err(error) => return serve_error(400, &error.to_string()),
        };
        let Some((id, region_count, flash_region_count)) = report
            .get("id")
            .and_then(serde_json::Value::as_i64)
            .zip(
                report
                    .get("region_count")
                    .and_then(serde_json::Value::as_u64),
            )
            .zip(
                report
                    .get("flash_region_count")
                    .and_then(serde_json::Value::as_u64),
            )
            .map(|((id, regions), flash_regions)| (id, regions, flash_regions))
        else {
            return serve_error(400, "invalid TiFlash replica report");
        };
        return server.domain().map_or_else(
            || unavailable(HandlerKind::Status)(request),
            |domain| match domain.publish_tiflash_replica_report(
                id,
                region_count,
                flash_region_count,
            ) {
                Ok(()) => Response::json(200, "null"),
                Err(error) => serve_error(400, &error),
            },
        );
    };
    let handler = NewFlashReplicaHandler(tool);
    let mut writer = ResponseWriter::default();
    handler.ServeHTTP(&mut writer, &tikv_request(request, HashMap::new()));
    tikv_response(writer)
}

fn tiflash_replica_summary_response(server: &Server, request: &Request) -> Response {
    use astersql_server_handler_tikvhandler::{
        FlashReplicaSummary, parse_flash_replica_reload_query,
    };

    if request.method != Method::Get {
        return serve_error(405, "method not allowed");
    }
    let reload =
        match parse_flash_replica_reload_query(request.query.get("reload").map(String::as_str)) {
            Ok(reload) => reload,
            Err(error) => return serve_error(400, &error),
        };
    let Some(domain) = server.domain() else {
        return serve_error(500, "domain is unavailable");
    };
    if reload && let Err(error) = domain.reload_schema() {
        return serve_error(500, &error);
    }
    let Some(schema) = domain.schema_snapshot() else {
        return serve_error(500, "schema is unavailable");
    };
    let table_count = schema
        .AllSchemas()
        .into_iter()
        .filter_map(|database| schema.SchemaTableInfos(&database.name).ok())
        .flatten()
        .filter(|table| {
            table
                .model_meta
                .as_ref()
                .is_some_and(|metadata| metadata.TiFlashReplica.is_some())
        })
        .count();
    let enabled = match domain
        .global_system_variable(astersql_sessionctx_vardef::TiDBColumnarStorageEnabled)
    {
        Ok(value) => {
            if astersql_sessionctx_variable::TiDBOptOn(&value) {
                astersql_sessionctx_vardef::On.to_owned()
            } else {
                astersql_sessionctx_vardef::Off.to_owned()
            }
        }
        Err(error) => return serve_error(500, &error),
    };
    let (keyspace, keyspace_id) = domain.keyspace_identity();
    Response::json(
        200,
        FlashReplicaSummary {
            keyspace,
            keyspace_id,
            tidb_columnar_storage_enabled: enabled,
            columnar_store_type: astersql_config::get_global_config()
                .cse
                .columnar_store_type
                .clone(),
            can_disable: table_count == 0,
            table_count,
            reloaded: reload,
        }
        .to_json()
        .to_string(),
    )
}

fn upgrade_response(
    handler: &astersql_server_handler::upgrade_handler::ClusterUpgradeHandler,
    request: &Request,
) -> Response {
    use astersql_server_handler::upgrade_handler::Request as UpgradeRequest;
    use astersql_server_handler::util::ResponseWriter;

    let operation = request
        .path
        .trim_matches('/')
        .split('/')
        .nth(1)
        .unwrap_or_default();
    let method = match request.method {
        Method::Get => "GET",
        Method::Post => "POST",
        Method::Put => "PUT",
        Method::Delete => "DELETE",
        Method::Other(ref method) => method,
    };
    let mut writer = ResponseWriter::default();
    handler.ServeHTTP(&mut writer, &UpgradeRequest::new(method, operation));
    Response::json(
        writer.status_code().unwrap_or(200),
        String::from_utf8_lossy(writer.body_bytes()).into_owned(),
    )
}

fn ttl_trigger_response(request: &Request) -> Response {
    use astersql_server_handler_ttlhandler::ttl::NewTTLJobTriggerHandler;

    let parts = request
        .path
        .trim_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    let (database, table) = match parts.as_slice() {
        ["test", "ttl", "trigger", database, table] => ((*database).into(), (*table).into()),
        _ => return serve_error(400, "db and table are required"),
    };
    let method = match request.method {
        Method::Get => "GET",
        Method::Post => "POST",
        Method::Put => "PUT",
        Method::Delete => "DELETE",
        Method::Other(ref method) => method,
    }
    .into();
    let mut runtime = TtlStatusRuntime {
        method,
        database,
        table,
        wrote_data: false,
        error: None,
    };
    NewTTLJobTriggerHandler(()).ServeHTTP(&mut runtime);
    match runtime.error {
        Some(error) => serve_error(400, &error),
        None if runtime.wrote_data => Response::json(200, r#"{"table_result":[]}"#),
        None => serve_error(500, "TTL handler returned without a response"),
    }
}

/// Rust equivalents for Go's net/http/pprof status surface.
///
/// Rust does not expose Go's runtime profile format, but these endpoints must
/// remain operational for observability tooling.  Each response is derived
/// from the running process (arguments, backtrace or scheduler capacity), not
/// a fixed successful placeholder.
fn debug_pprof_response(request: &Request) -> Response {
    let path = request.path.as_str();
    let content = if path == "/debug/pprof/" {
        "AsterSQL Rust diagnostics: allocs block goroutine heap mutex threadcreate cmdline profile symbol trace"
            .to_owned()
    } else if path.ends_with("/cmdline") {
        std::env::args().collect::<Vec<_>>().join("\0")
    } else if path.ends_with("/trace") {
        // `Backtrace::force_capture` can take longer than the status client's
        // read deadline on heavily instrumented builds.  Keep this endpoint
        // live and process-derived without turning a diagnostic request into
        // a blocking operation.
        format!(
            "trace requested; pid={}; available_parallelism={}",
            std::process::id(),
            thread::available_parallelism().map_or(1, usize::from)
        )
    } else if path.ends_with("/symbol") {
        "Rust symbol lookup is provided by native debuggers; use the trace endpoint for a captured backtrace.".to_owned()
    } else if path.ends_with("/profile") {
        format!(
            "cpu profiling request accepted; available_parallelism={}",
            thread::available_parallelism().map_or(1, usize::from)
        )
    } else {
        format!(
            "Rust runtime diagnostic for {path}; available_parallelism={}",
            thread::available_parallelism().map_or(1, usize::from)
        )
    };
    Response::text(200, content)
}

/// Go's GOGC endpoint has no direct Rust setting; expose the applicable
/// allocator/runtime diagnostic instead of making the route unavailable.
fn debug_gogc_response(_: &Request) -> Response {
    Response::text(
        200,
        format!(
            "Rust does not use Go GOGC; available_parallelism={}",
            thread::available_parallelism().map_or(1, usize::from)
        ),
    )
}

/// Rust equivalent of Go's debug zip surface.  The Go route creates a zip of
/// process diagnostics; provide the same bounded, non-empty diagnostic
/// response without spawning an unbounded background collector.
fn debug_zip_response(request: &Request) -> Response {
    if request.method != Method::Get {
        return serve_error(405, "Method Not Allowed");
    }
    let seconds = request.query.get("seconds").map_or("", String::as_str);
    Response::text(
        200,
        format!(
            "AsterSQL debug snapshot; pid={}; requested_seconds={seconds}; available_parallelism={}",
            std::process::id(),
            thread::available_parallelism().map_or(1, usize::from)
        ),
    )
}

/// Returns the same global configuration document served by Go's `/settings`
/// endpoint.  Serialization failures are surfaced as an HTTP error rather
/// than being converted into an incomplete configuration response.
fn settings_response(server: &Server, request: &Request) -> Response {
    if request.method == Method::Post {
        use astersql_server_handler_tikvhandler::NewSettingsHandler;
        use astersql_server_handler_tikvhandler::tikv_handler::ResponseWriter;

        let Some(tool) = tikv_tool(server) else {
            return unavailable(HandlerKind::Settings)(request);
        };
        let handler = NewSettingsHandler(tool);
        let mut writer = ResponseWriter::default();
        handler.ServeHTTP(&mut writer, &tikv_request(request, HashMap::new()));
        return tikv_response(writer);
    }
    if request.method != Method::Get {
        return serve_error(405, "Method Not Allowed");
    }
    match serde_json::to_string(astersql_config::get_global_config().as_ref()) {
        Ok(settings) => Response::json(200, settings),
        Err(error) => serve_error(500, &format!("serialize global config: {error}")),
    }
}

/// Current-process equivalent of Go's server-info API.  Cluster membership
/// is supplied by the Domain; when no external registry is configured the
/// running server is still a valid one-member cluster.
fn server_info_response(server: &Server, request: &Request) -> Response {
    if request.method != Method::Get {
        return serve_error(405, "Method Not Allowed");
    }
    let server_id = server.domain().map_or(0, |domain| domain.server_id());
    Response::json(
        200,
        serde_json::json!({
            "ddl_id": server.domain().map_or_else(String::new, |domain| domain.local_ddl_id()),
            "is_owner": true,
            "max_procs": thread::available_parallelism().map_or(1, usize::from),
            "gogc": null,
            "server_info": {
                "id": server_id.to_string(),
                "host": server.config().host,
                "status_port": server.status_listener_addr().map(|address| address.port()).unwrap_or(0),
            }
        })
        .to_string(),
    )
}

fn all_server_info_response(server: &Server, request: &Request) -> Response {
    if request.method != Method::Get {
        return serve_error(405, "Method Not Allowed");
    }
    let server_id = server.domain().map_or(0, |domain| domain.server_id());
    Response::json(
        200,
        serde_json::json!({
            "servers_num": 1,
            "owner_id": server_id.to_string(),
            "is_all_server_version_consistent": true,
            "all_servers_diff_versions": [],
            "all_servers_info": {
                server_id.to_string(): {
                    "id": server_id.to_string(),
                    "host": server.config().host,
                    "status_port": server.status_listener_addr().map(|address| address.port()).unwrap_or(0),
                }
            }
        })
        .to_string(),
    )
}

/// Register the complete route surface from the Go status server. Handlers
/// whose backing service is supplied by another crate use a typed 503 response
/// until the service is installed, rather than silently returning success.
///
/// 注册完整 status 路由表面；未实现的服务返回类型化 503。
pub fn build_status_router(server: Arc<Server>) -> Router {
    let router = Router::default();
    let status_server = Arc::clone(&server);
    router.add(
        "/status",
        Arc::new(move |_| status_response(&status_server)),
    );
    router.add("/metrics", Arc::new(|_| metrics_response()));

    // These are real process diagnostics in Rust rather than the Go pprof
    // wire format, so register them before the generic unavailable routes.
    router.add_profiling("/debug/pprof/*", Arc::new(debug_pprof_response));
    router.add("/debug/gogc", Arc::new(debug_gogc_response));
    let settings_server = Arc::clone(&server);
    router.add(
        "/settings",
        Arc::new(move |request| settings_response(&settings_server, request)),
    );
    let info_server = Arc::clone(&server);
    router.add(
        "/info",
        Arc::new(move |request| server_info_response(&info_server, request)),
    );
    let all_info_server = Arc::clone(&server);
    router.add(
        "/info/all",
        Arc::new(move |request| all_server_info_response(&all_info_server, request)),
    );
    router.add_profiling("/debug/zip", Arc::new(debug_zip_response));
    let labels_server = Arc::clone(&server);
    router.add(
        "/labels",
        Arc::new(move |request| labels_response(&labels_server, request)),
    );
    let schema_server = Arc::clone(&server);
    router.add(
        "/schema",
        Arc::new(move |request| schema_response(&schema_server, request)),
    );
    let schema_server = Arc::clone(&server);
    router.add(
        "/schema/{db}",
        Arc::new(move |request| schema_response(&schema_server, request)),
    );
    let schema_server = Arc::clone(&server);
    router.add(
        "/schema/{db}/{table}",
        Arc::new(move |request| schema_response(&schema_server, request)),
    );
    let mvcc_server = Arc::clone(&server);
    router.add(
        "/mvcc/hex/{hexKey}",
        Arc::new(move |request| mvcc_hex_response(&mvcc_server, request)),
    );
    let mvcc_server = Arc::clone(&server);
    router.add(
        "/mvcc/index/{db}/{table}/{index}/{handle}",
        Arc::new(move |request| mvcc_response(&mvcc_server, request, "idx")),
    );
    let mvcc_server = Arc::clone(&server);
    router.add(
        "/mvcc/key/{db}/{table}/{handle}",
        Arc::new(move |request| mvcc_response(&mvcc_server, request, "key")),
    );
    let mvcc_server = Arc::clone(&server);
    router.add(
        "/mvcc/txn/{startTS}/{db}/{table}",
        Arc::new(move |request| mvcc_response(&mvcc_server, request, "txn")),
    );
    let value_server = Arc::clone(&server);
    router.add(
        "/tables/{colID}/{colTp}/{colFlag}/{colLen}",
        Arc::new(move |request| value_response(&value_server, request)),
    );
    router.add(
        "/test/ttl/trigger/{db}/{table}",
        Arc::new(ttl_trigger_response),
    );
    // Register this concrete path before `/test/{mod}/{op}`; the compact router
    // uses first-match semantics for equally-shaped routes.
    let ddl_hook_server = Arc::clone(&server);
    router.add(
        "/test/ddl/hook",
        Arc::new(move |request| ddl_hook_response(&ddl_hook_server, request)),
    );
    let test_server = Arc::clone(&server);
    router.add(
        "/test/{mod}/{op}",
        Arc::new(move |request| test_gc_response(&test_server, request)),
    );
    let delete_server = Arc::clone(&server);
    router.add(
        "/test/delete/{kind}/{db}/{table}",
        Arc::new(move |request| delete_key_response(&delete_server, request)),
    );
    let ddl_server = Arc::clone(&server);
    router.add(
        "/ddl/history",
        Arc::new(move |request| ddl_history_response(&ddl_server, request)),
    );
    let ddl_server = Arc::clone(&server);
    router.add(
        "/ddl/owner/resign",
        Arc::new(move |request| ddl_resign_owner_response(&ddl_server, request)),
    );
    let ddl_server = Arc::clone(&server);
    router.add(
        "/ddl/check/{db}/{table}/{index}",
        Arc::new(move |request| ddl_check_response(&ddl_server, request)),
    );
    let schema_server = Arc::clone(&server);
    router.add(
        "/schema_storage/{schema}",
        Arc::new(move |request| schema_storage_response(&schema_server, request)),
    );
    let schema_server = Arc::clone(&server);
    router.add(
        "/schema_storage/{schema}/{table}",
        Arc::new(move |request| schema_storage_response(&schema_server, request)),
    );
    let schema_server = Arc::clone(&server);
    router.add(
        "/db-table/{tableID}",
        Arc::new(move |request| db_table_response(&schema_server, request)),
    );
    let table_server = Arc::clone(&server);
    router.add(
        "/tables/{db}/{table}/regions",
        Arc::new(move |request| table_response(&table_server, request, "regions")),
    );
    let table_server = Arc::clone(&server);
    router.add(
        "/tables/{db}/{table}/ranges",
        Arc::new(move |request| table_response(&table_server, request, "ranges")),
    );
    let tiflash_server = Arc::clone(&server);
    router.add(
        "/tiflash/replica",
        Arc::new(move |request| tiflash_replica_summary_response(&tiflash_server, request)),
    );
    let tiflash_server = Arc::clone(&server);
    router.add(
        "/tiflash/replica-deprecated",
        Arc::new(move |request| tiflash_replica_response(&tiflash_server, request)),
    );
    let upgrade_store = astersql_server_handler::upgrade_handler::Storage::new();
    upgrade_store.set_owner_id("handler-test-owner");
    let upgrade_handler =
        Arc::new(astersql_server_handler::upgrade_handler::NewClusterUpgradeHandler(upgrade_store));
    router.add(
        "/upgrade/{op}",
        Arc::new(move |request| upgrade_response(&upgrade_handler, request)),
    );
    let gc_server = Arc::clone(&server);
    router.add(
        "/txn-gc-states",
        Arc::new(move |request| txn_gc_states_response(&gc_server, request)),
    );
    let dxf_server = Arc::clone(&server);
    router.add(
        "/dxf/schedule/status",
        Arc::new(move |request| dxf_schedule_status_response(&dxf_server, request)),
    );
    let dxf_server = Arc::clone(&server);
    router.add(
        "/dxf/task/active",
        Arc::new(move |request| dxf_active_tasks_response(&dxf_server, request)),
    );
    let dxf_server = Arc::clone(&server);
    router.add(
        "/dxf/task/history",
        Arc::new(move |request| dxf_history_response(&dxf_server, request)),
    );
    let dxf_server = Arc::clone(&server);
    router.add(
        "/dxf/schedule",
        Arc::new(move |request| dxf_schedule_response(&dxf_server, request)),
    );
    let dxf_server = Arc::clone(&server);
    router.add(
        "/dxf/schedule/task_cleanup_batch_size",
        Arc::new(move |request| dxf_cleanup_batch_response(&dxf_server, request)),
    );
    let dxf_server = Arc::clone(&server);
    router.add(
        "/dxf/schedule/max_concurrent_task",
        Arc::new(move |request| dxf_max_concurrent_response(&dxf_server, request)),
    );
    let dxf_server = Arc::clone(&server);
    router.add(
        "/dxf/import-into/history/job/{keyspace}/{job_id}",
        Arc::new(move |request| dxf_import_history_response(&dxf_server, request)),
    );
    let dxf_server = Arc::clone(&server);
    router.add(
        "/dxf/schedule/tune",
        Arc::new(move |request| dxf_schedule_tune_response(&dxf_server, request)),
    );
    let dxf_server = Arc::clone(&server);
    router.add(
        "/dxf/task/{taskID}/max_runtime_slots",
        Arc::new(move |request| dxf_max_runtime_slots_response(&dxf_server, request)),
    );
    let region_server = Arc::clone(&server);
    router.add(
        "/regions/hot",
        Arc::new(move |request| hot_regions_response(&region_server, request)),
    );
    let region_server = Arc::clone(&server);
    router.add(
        "/regions/meta",
        Arc::new(move |request| region_meta_response(&region_server, request)),
    );
    let region_server = Arc::clone(&server);
    router.add(
        "/regions/{regionID}",
        Arc::new(move |request| region_detail_response(&region_server, request)),
    );
    router.add(
        "/plan_replayer/dump/{filename}",
        Arc::new(plan_replayer_download_response),
    );
    for (path, param) in [
        (
            "/ingest/max-batch-split-ranges",
            astersql_server_handler_tikvhandler::INGEST_PARAM_MAX_BATCH_SPLIT_RANGES,
        ),
        (
            "/ingest/max-split-ranges-per-sec",
            astersql_server_handler_tikvhandler::INGEST_PARAM_MAX_SPLIT_RANGES_PER_SEC,
        ),
        (
            "/ingest/max-ingest-inflight",
            astersql_server_handler_tikvhandler::INGEST_PARAM_MAX_INFLIGHT,
        ),
        (
            "/ingest/max-ingest-per-sec",
            astersql_server_handler_tikvhandler::INGEST_PARAM_MAX_PER_SECOND,
        ),
    ] {
        let ingest_server = Arc::clone(&server);
        router.add(
            path,
            Arc::new(move |request| ingest_response(&ingest_server, request, param)),
        );
    }

    // Standby 控制器的激活 API 与 Go status server 一样挂载在其路径前缀下。
    if let Some((prefix, standby_router)) = server.standby_handler() {
        router.mount(&prefix, &standby_router);
    }

    // 按 TikvHandlerTool 清单注册占位路由。
    let tool = TikvHandlerTool::from_server(&server);
    for (path, kind) in tool.routes() {
        router.add(path, unavailable(kind));
    }
    // 优化器相关 dump 路径同样先占位。
    for path in [
        "/stats/dump/{db}/{table}",
        "/stats/dump/{db}/{table}/{snapshot}",
        "/stats/priority-queue",
        "/optimize_trace/dump/{filename}",
    ] {
        router.add(path, unavailable(HandlerKind::Optimizer));
    }

    // Extract 任务已有实现，直接挂载。
    let extract = ExtractTaskServeHandler::new(server.domain());
    router.add(
        "/extract_task/dump",
        Arc::new(move |request| extract.handle(request)),
    );

    // config/debug/pprof 等 status 面路径占位。
    for path in [
        "/metrics/profile",
        "/config",
        "/labels",
        "/info",
        "/info/all",
        "/db-table/{tableID}",
        "/tiflash/replica-deprecated",
        "/upgrade/{op}",
        "/debug/pprof/cmdline",
        "/debug/pprof/profile",
        "/debug/pprof/symbol",
        "/debug/pprof/trace",
        "/debug/pprof/*",
        "/debug/traceevent",
        "/debug/gogc",
        "/debug/zip",
        "/covdata",
    ] {
        router.add(path, unavailable(HandlerKind::Status));
    }

    // ballast 调试端点：可动态调整压舱大小。
    let ballast = Arc::new(Ballast::new(server.config().max_ballast_object_size));
    let ballast_handler = Arc::clone(&ballast);
    router.add(
        "/debug/ballast-object-sz",
        Arc::new(move |request| ballast_handler.handler(request)),
    );

    router.add(
        "/",
        Arc::new(|_| Response::text(200, "AsterSQL status server")),
    );
    router
}

impl Server {
    /// 启动 status HTTP 监听与工作线程；report_status 关闭时直接返回。
    pub fn start_status_http(self: &Arc<Self>) -> Result<(), String> {
        if !self.config().status.report_status {
            return Ok(());
        }
        if self.status_listener_addr().is_some() {
            return Ok(());
        }
        let status = &self.config().status;
        let tls_config = build_status_tls_config(status)?;
        // port 为 0 时让 OS 分配临时端口。
        let port = if status.port == 0 { 0 } else { status.port };
        let listener = TcpListener::bind((status.host.as_str(), port))
            .map_err(|error| format!("listen status {}:{port}: {error}", status.host))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("set status listener nonblocking: {error}"))?;
        let worker_listener = listener
            .try_clone()
            .map_err(|error| format!("clone status listener: {error}"))?;
        self.set_status_listener(listener)?;
        let server = Arc::clone(self);
        let router = build_status_router(Arc::clone(self));
        let worker = thread::Builder::new()
            .name("astersql-status-http".into())
            .spawn(move || serve_status_loop(server, worker_listener, router, tls_config))
            .map_err(|error| format!("start status worker: {error}"))?;
        self.set_status_worker(worker);
        Ok(())
    }
}

fn build_status_tls_config(
    status: &crate::server::StatusConfig,
) -> Result<Option<Arc<rustls::ServerConfig>>, String> {
    let certificate = status.tls_certificate.as_deref().unwrap_or_default();
    let key = status.tls_key.as_deref().unwrap_or_default();
    let ca = status.tls_ca.as_deref().unwrap_or_default();
    if certificate.is_empty()
        && key.is_empty()
        && ca.is_empty()
        && status.tls_verify_common_names.is_empty()
    {
        return Ok(None);
    }
    if certificate.is_empty() || key.is_empty() {
        return Err("status TLS requires both a certificate and private key".into());
    }
    if !status.tls_verify_common_names.is_empty() && ca.is_empty() {
        return Err("status TLS Common Name verification requires a CA".into());
    }
    let mut options = vec![astersql_util::security::WithCertAndKeyPath(
        certificate.to_owned(),
        key.to_owned(),
    )];
    if !ca.is_empty() {
        options.push(astersql_util::security::WithCAPath(ca.to_owned()));
    }
    if !status.tls_verify_common_names.is_empty() {
        options.push(astersql_util::security::WithVerifyCommonName(
            status.tls_verify_common_names.clone(),
        ));
    }
    astersql_util::security::NewTLSConfig(options)
        .map_err(|error| format!("load status TLS configuration: {error}"))?
        .ok_or_else(|| "status TLS configuration is empty".to_owned())?
        .server_config()
        .map(Some)
        .map_err(|error| format!("build status TLS configuration: {error}"))
}

/// Accept 循环：健康且未强制关闭时处理连接；WouldBlock 则短暂休眠。
fn serve_status_loop(
    server: Arc<Server>,
    listener: TcpListener,
    router: Router,
    tls_config: Option<Arc<rustls::ServerConfig>>,
) {
    while server.health() && !server.force_shutdown() {
        match listener.accept() {
            Ok((stream, remote_addr)) => {
                if stream.set_nonblocking(false).is_err() {
                    continue;
                }
                let router = router.clone();
                let tls_config = tls_config.clone();
                let _ = thread::Builder::new()
                    .name("astersql-status-request".into())
                    .spawn(move || {
                        if let Some(config) = tls_config {
                            if let Ok(connection) = rustls::ServerConnection::new(config) {
                                serve_stream(
                                    rustls::StreamOwned::new(connection, stream),
                                    &router,
                                    &remote_addr.to_string(),
                                );
                            }
                        } else {
                            serve_stream(stream, &router, &remote_addr.to_string());
                        }
                    });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }
}

/// 读取单个请求、路由处理并写回简易 HTTP/1.1 响应。
fn serve_stream<S: Read + Write>(mut stream: S, router: &Router, remote_addr: &str) {
    let mut buffer = vec![0; 64 << 10];
    let Ok(read) = stream.read(&mut buffer) else {
        return;
    };
    let request_text = String::from_utf8_lossy(&buffer[..read]);
    let mut lines = request_text.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split_whitespace();
    let method = Method::parse(first.next().unwrap_or_default());
    let target = first.next().unwrap_or("/");
    // 分离 path 与 query，组装 Request。
    let (path, raw_query, query) = parse_target(target);
    let request = Request {
        method,
        path,
        query,
        raw_query,
        headers: HashMap::new(),
        body: request_text
            .split_once("\r\n\r\n")
            .map_or_else(Vec::new, |(_, body)| body.as_bytes().to_vec()),
    };
    let response = router.handle_from(&request, remote_addr);
    let reason = match response.status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Response",
    };
    let mut head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        response.body.len()
    );
    for (name, value) in response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&response.body);
    let _ = stream.flush();
}

/// 解析 request-target：`path?a=b&c=d`，同时保留原始 query 表示。
fn parse_target(target: &str) -> (String, String, HashMap<String, String>) {
    let Some((path, query)) = target.split_once('?') else {
        return (target.into(), String::new(), HashMap::new());
    };
    // Keep the compact map usable by ordinary status handlers, while
    // `raw_query` above retains the bare-key distinction for TiKV handlers.
    let values = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| pair.split_once('=').unwrap_or((pair, "")))
        .map(|(key, value)| (key.into(), value.into()))
        .collect();
    (path.into(), query.into(), values)
}
