// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// `tidbmanager` 迁移回归单测：用本地 TCP 假服务器与可注入 transport 验证 `free` 契约。

use std::error::Error as _;
use std::io::{self, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;

use super::{
    Client, FREE_REQ_PATH, HttpResponse, HttpTransport, Result, new_client,
    new_client_with_transport,
};

/// 启动一次性 HTTP 应答服务，返回监听地址与收取到的原始请求线程句柄。
fn serve_once(status: &str, body: &str) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let status = status.to_owned();
    let body = body.to_owned();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        // 读到完整请求头（`\r\n\r\n`）或对端关闭后停止。
        loop {
            let count = stream.read(&mut buffer).unwrap();
            request.extend_from_slice(&buffer[..count]);
            if count == 0 || request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        String::from_utf8(request).unwrap()
    });
    (addr.to_string(), handle)
}

/// 验证 `free` 发送 PUT 到固定路径，并带上 Go 侧同名查询参数。
#[test]
fn free_sends_put_path_and_go_query_fields() {
    let (addr, request) = serve_once("200 OK", "");
    let client = new_client(&addr, None, "pod-1", "10.0.0.1", "ns-1").unwrap();

    client.free("idle restart").unwrap();

    let request = request.join().unwrap();
    let request_line = request.lines().next().unwrap();
    assert!(request_line.starts_with(&format!("PUT {FREE_REQ_PATH}?")));
    let query = request_line.split_whitespace().nth(1).unwrap();
    let query = url::Url::parse(&format!("http://manager{query}"))
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(query.get("pod_name").map(String::as_str), Some("pod-1"));
    assert_eq!(query.get("pod_ip").map(String::as_str), Some("10.0.0.1"));
    assert_eq!(query.get("ns").map(String::as_str), Some("ns-1"));
    assert_eq!(
        query.get("normal_restart_log").map(String::as_str),
        Some("idle restart")
    );
}

/// 验证非 200 时错误信息同时包含状态行与响应体。
#[test]
fn free_returns_status_and_body_on_non_ok() {
    let (addr, request) = serve_once("503 Service Unavailable", "not ready");
    let client = new_client(&addr, None, "pod-1", "10.0.0.1", "ns-1").unwrap();

    let error = client.free("idle restart").unwrap_err().to_string();

    request.join().unwrap();
    assert!(error.contains("503 Service Unavailable"), "{error}");
    assert!(error.contains("not ready"), "{error}");
}

/// 读 body 时始终失败的假 Reader。
struct ReadFails;

impl Read for ReadFails {
    fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("read failed"))
    }
}

/// 返回 503 且 body 读取失败的假 transport。
struct ErrorTransport;

impl HttpTransport for ErrorTransport {
    fn put(&self, _url: url::Url) -> Result<HttpResponse> {
        Ok(HttpResponse::new(
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            Box::new(ReadFails),
        ))
    }
}

/// 验证读 body 失败时错误链保留 `source` 与上下文字符串。
#[test]
fn free_preserves_body_read_error_context() {
    let client = new_client_with_transport(
        "http://manager.example.com",
        "pod-1",
        "10.0.0.1",
        "ns-1",
        Arc::new(ErrorTransport),
    )
    .unwrap();

    let error = client.free("idle restart").unwrap_err();
    let message = error.to_string();
    assert!(message.contains("503 Service Unavailable"), "{message}");
    assert!(
        message.contains("read body failed: read failed"),
        "{message}"
    );
    assert_eq!(error.source().unwrap().to_string(), "read failed");
}
