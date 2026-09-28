// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! pprof/status HTTP server ported from `br/pkg/utils/pprof.go`.
//! 进程内状态/pprof 监听器：单例绑定、可选 TLS、可注入路由处理器。
//! 默认 handler 导出真实 Prometheus 指标、进程命令行与 CPU pprof protobuf。
//! STARTED_PPROF 是进程级单例，跨调用共享绑定地址。
//! listen 失败会 Trace 底层 IO 错误，便于排查端口占用。
//! serve 线程崩溃后清空 started，允许热重试绑定。
//! wrap_listener 复用 util/security 的 TLS 包装能力。
//! handle_connection 读取 64KiB 上限内的请求头与 symbol 请求体。
//! 响应始终 Connection: close，简化半双工处理。
//! 默认路由与 Go pprof 路径名对齐，便于运维习惯。
//! metrics 使用默认 registry，profile 使用 pprof-rs 的进程级采样器。
//! 不支持的 Rust runtime trace 会显式返回 501，不伪造空成功响应。
//! failed_to_connect 保留 ErrFailedToConnect 包装能力。
//! StartStatusListener 是最常用的默认启动入口。
//! 重复启动告警文案与 Go 保持一致，避免运维误判。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use std::sync::Arc;

use astersql_br_pkg_errors::{ErrFailedToConnect, ErrUnknown};
use astersql_br_pkg_logutil::{Field, log};
use astersql_errors::{Annotate, SharedError, Trace};
use astersql_util::security::{Connection, Listener, TLS};
use prometheus::{Encoder, TextEncoder};
use prost::Message;

/// 全局已绑定地址；非空表示 pprof 已启动，防止重复 listen。
static STARTED_PPROF: OnceLock<Mutex<String>> = OnceLock::new();

fn started_pprof() -> &'static Mutex<String> {
    STARTED_PPROF.get_or_init(|| Mutex::new(String::new()))
}

/// 绑定 status 地址；若已启动则返回 ErrUnknown（对齐 Go 只启一次）。
fn listen(status_addr: &str) -> Result<TcpListener, SharedError> {
    let lock = started_pprof();
    let mut started = lock.lock().expect("started_pprof lock poisoned");
    if !started.is_empty() {
        // 重复启动直接拒绝，避免多端口/多线程抢同一全局状态。
        log::Warn(
            "Try to start pprof when it has been started, nothing will happen",
            [Field::string("address", started.as_str())],
        );
        return Err(Annotate(
            Some(SharedError::new((*ErrUnknown).clone())),
            format!("try to start pprof when it has been started at {started}"),
        )
        .expect("annotate duplicate pprof"));
    }
    inject_failpoint_determined_port(status_addr);
    let listener = TcpListener::bind(status_addr).map_err(|err| {
        log::Warn(
            "failed to start pprof",
            [
                Field::string("addr", status_addr),
                Field::string("error", &err.to_string()),
            ],
        );
        Trace(Some(SharedError::new(err))).expect("trace")
    })?;
    // 记录实际 bound 地址（含 :0 分配端口），供日志与重复启动检测。
    let bound = listener.local_addr().map_err(|err| SharedError::new(err))?;
    *started = bound.to_string();
    log::L().Info(
        "bound pprof to addr",
        [Field::string("addr", started.as_str())],
    );
    let _ = writeln!(std::io::stderr(), "bound pprof to addr {started}");
    Ok(listener)
}

/// failpoint 钩子：测试可改写绑定端口；生产为空实现。
fn inject_failpoint_determined_port(_status_addr: &str) {}

/// HTTP 请求中默认 status handler 需要的字段。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatusRequest {
    pub method: String,
    pub target: String,
    pub body: Vec<u8>,
}

/// HTTP handler 响应；未匹配路由由服务端生成 Go `ServeMux` 风格 404。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatusResponse {
    pub status: &'static str,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl StatusResponse {
    fn ok(content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            status: "200 OK",
            content_type,
            body,
        }
    }

    fn error(status: &'static str, message: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            body: message.into(),
        }
    }
}

pub type StatusHandler = Arc<dyn Fn(&StatusRequest) -> Option<StatusResponse> + Send + Sync>;

fn query_seconds(target: &str, default: u64) -> u64 {
    target
        .split_once('?')
        .map(|(_, query)| query)
        .and_then(|query| {
            query.split('&').find_map(|field| {
                let (key, value) = field.split_once('=')?;
                (key == "seconds")
                    .then(|| value.parse::<u64>().ok())
                    .flatten()
            })
        })
        .unwrap_or(default)
        .clamp(1, 300)
}

fn cpu_profile(seconds: u64) -> Result<Vec<u8>, String> {
    let guard = pprof::ProfilerGuardBuilder::default()
        .frequency(100)
        .build()
        .map_err(|error| error.to_string())?;
    thread::sleep(Duration::from_secs(seconds));
    let report = guard.report().build().map_err(|error| error.to_string())?;
    let profile = report.pprof().map_err(|error| error.to_string())?;
    let mut body = Vec::new();
    profile
        .encode(&mut body)
        .map_err(|error| error.to_string())?;
    Ok(body)
}

/// 注册与 Go 默认 mux 等价的 metrics / pprof handler。
pub fn RegisterDefaultStatusHandlers() -> StatusHandler {
    Arc::new(|request| {
        let path = request.target.split('?').next().unwrap_or("/");
        let get_or_head = request.method == "GET" || request.method == "HEAD";
        match path {
            "/metrics" if get_or_head => {
                let encoder = TextEncoder::new();
                let mut body = Vec::new();
                match encoder.encode(&prometheus::gather(), &mut body) {
                    Ok(()) => Some(StatusResponse::ok(
                        "text/plain; version=0.0.4; charset=utf-8",
                        body,
                    )),
                    Err(error) => Some(StatusResponse::error(
                        "500 Internal Server Error",
                        format!("failed to gather metrics: {error}\n"),
                    )),
                }
            }
            "/metrics" => Some(StatusResponse::error(
                "405 Method Not Allowed",
                "Method Not Allowed\n",
            )),
            "/debug/pprof/" if request.method == "GET" => Some(StatusResponse::ok(
                "text/html; charset=utf-8",
                b"<html><head><title>/debug/pprof/</title></head><body><a href=\"cmdline\">cmdline</a><br><a href=\"profile\">profile</a><br><a href=\"symbol\">symbol</a><br><a href=\"trace\">trace</a><br></body></html>\n".to_vec(),
            )),
            "/debug/pprof/cmdline" if request.method == "GET" => Some(StatusResponse::ok(
                "text/plain; charset=utf-8",
                std::env::args_os()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("\0")
                    .into_bytes(),
            )),
            "/debug/pprof/profile" if request.method == "GET" => {
                let seconds = query_seconds(&request.target, 30);
                Some(match cpu_profile(seconds) {
                    Ok(body) => StatusResponse::ok("application/octet-stream", body),
                    Err(error) => StatusResponse::error(
                        "500 Internal Server Error",
                        format!("Could not enable CPU profiling: {error}\n"),
                    ),
                })
            }
            "/debug/pprof/symbol" if request.method == "GET" => Some(StatusResponse::ok(
                "text/plain; charset=utf-8",
                b"num_symbols: 0\n".to_vec(),
            )),
            "/debug/pprof/symbol" if request.method == "POST" => Some(StatusResponse::ok(
                "text/plain; charset=utf-8",
                request
                    .body
                    .split(|byte| *byte == b'+')
                    .map(|address| {
                        let mut line = address.to_vec();
                        line.extend_from_slice(b" ?\n");
                        line
                    })
                    .flatten()
                    .collect(),
            )),
            // Rust has no Go execution tracer; return an explicit unsupported response
            // rather than the previous misleading empty successful trace.
            "/debug/pprof/trace" if request.method == "GET" => Some(StatusResponse::error(
                "501 Not Implemented",
                "runtime execution trace is unavailable on Rust\n",
            )),
            path if path.starts_with("/debug/pprof/") => Some(StatusResponse::error(
                "405 Method Not Allowed",
                "Method Not Allowed\n",
            )),
            _ => None,
        }
    })
}

/// 有 TLS 则包装为加密 Listener，否则保持明文 TCP。
fn wrap_listener(wrapper: Option<&TLS>, listener: TcpListener) -> Listener {
    match wrapper {
        Some(tls) => tls.WrapListener(listener),
        None => Listener::Plain(listener),
    }
}

/// 绑定并后台 serve；自定义 handler 覆盖默认桩路由。
pub fn StartStatusListenerWithHandler(
    status_addr: &str,
    wrapper: Option<&TLS>,
    handler: StatusHandler,
) -> Result<(), SharedError> {
    let listener = listen(status_addr)?;
    let wrapped = wrap_listener(wrapper, listener);
    let started = started_pprof()
        .lock()
        .expect("started_pprof lock poisoned")
        .clone();
    // serve 失败时清空 STARTED，允许后续重新绑定。
    thread::spawn(move || {
        if let Err(err) = serve_listener(wrapped, handler) {
            log::Warn(
                "failed to serve pprof",
                [
                    Field::string("addr", &started),
                    Field::string("error", &err.to_string()),
                ],
            );
            *started_pprof().lock().expect("started_pprof lock poisoned") = String::new();
        }
    });
    Ok(())
}

/// 使用默认桩 handler 启动 status 监听。
pub fn StartStatusListener(status_addr: &str, wrapper: Option<&TLS>) -> Result<(), SharedError> {
    StartStatusListenerWithHandler(status_addr, wrapper, RegisterDefaultStatusHandlers())
}

/// 接受循环：每连接一线程，避免慢客户端阻塞 accept。
fn serve_listener(listener: Listener, handler: StatusHandler) -> Result<(), SharedError> {
    loop {
        let (conn, _) = listener
            .accept()
            .map_err(|err| SharedError::new(std::io::Error::other(err.to_string())))?;
        let handler = Arc::clone(&handler);
        thread::spawn(move || {
            let _ = handle_connection(conn, handler);
        });
    }
}

/// 解析单个 HTTP/1.x 请求并按 handler 结果写回状态、类型与 body。
fn handle_connection(mut conn: Connection, handler: StatusHandler) -> Result<(), SharedError> {
    let mut buffer = [0u8; 65536];
    let read = conn
        .read(&mut buffer)
        .map_err(|err| SharedError::new(err))?;
    if read == 0 {
        return Ok(());
    }
    let request = String::from_utf8_lossy(&buffer[..read]);
    let mut lines = request.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_owned();
    let target = parts.next().unwrap_or("/").to_owned();
    let body_start = request
        .find("\r\n\r\n")
        .map(|index| index + 4)
        .unwrap_or(read);
    let request = StatusRequest {
        method,
        target,
        body: buffer[body_start..read].to_vec(),
    };
    let status_response = handler(&request)
        .unwrap_or_else(|| StatusResponse::error("404 Not Found", "404 page not found\n"));
    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status_response.status,
        status_response.content_type,
        status_response.body.len()
    );
    conn.write_all(response.as_bytes())
        .map_err(|err| SharedError::new(err))?;
    if request.method != "HEAD" {
        conn.write_all(&status_response.body)
            .map_err(|err| SharedError::new(err))?;
    }
    Ok(())
}

// Keep failed-to-connect error available for callers extending listener startup.
// 预留连接失败包装；当前启动路径未直接调用。
#[allow(dead_code)]
fn failed_to_connect(err: SharedError) -> SharedError {
    Annotate(
        Some(SharedError::new((*ErrFailedToConnect).clone())),
        &err.to_string(),
    )
    .unwrap_or(err)
}
