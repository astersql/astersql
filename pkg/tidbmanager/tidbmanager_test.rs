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

// `tidbmanager` 包内单测：用 `tiny_http` 假服务器与假 RoundTripper 验证 `free`。

use std::io::{self, Read};
use std::sync::Arc;
use std::thread;

use super::{
    Client, FREE_REQ_PATH, HttpResponse, HttpTransport, ManagerError, Result, new_client,
    new_client_with_transport,
};
use tiny_http::{Method, Response, Server};

/// 启动一次性 HTTP 服务，返回地址与收到的 (方法, URL) 句柄。
fn serve_once(status: u16, body: &'static str) -> (String, thread::JoinHandle<(Method, String)>) {
    let server = Server::http("127.0.0.1:0").expect("bind test server");
    let address = server.server_addr().to_string();
    let handle = thread::spawn(move || {
        let request = server.recv().expect("receive request");
        let received = (request.method().clone(), request.url().to_owned());
        request
            .respond(Response::from_string(body).with_status_code(status))
            .expect("send response");
        received
    });
    (address, handle)
}

/// 验证成功路径：PUT、路径与查询参数与 Go 一致。
#[test]
fn test_free() {
    let (address, server) = serve_once(200, "");
    let client = new_client(&address, None, "pod-1", "10.0.0.1", "ns-1").unwrap();

    client.free("idle restart").unwrap();

    let (method, request_url) = server.join().unwrap();
    assert_eq!(method, Method::Put);
    let url = url::Url::parse(&format!("http://test{request_url}")).unwrap();
    assert_eq!(url.path(), FREE_REQ_PATH);
    let query = url
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(query.get("pod_name").unwrap(), "pod-1");
    assert_eq!(query.get("pod_ip").unwrap(), "10.0.0.1");
    assert_eq!(query.get("ns").unwrap(), "ns-1");
    assert_eq!(query.get("normal_restart_log").unwrap(), "idle restart");
}

/// 验证非 OK 响应时错误字符串包含状态与 body。
#[test]
fn test_free_returns_error_on_non_ok() {
    let (address, server) = serve_once(503, "not ready");
    let client = new_client(&address, None, "pod-1", "10.0.0.1", "ns-1").unwrap();

    let error = client.free("idle restart").unwrap_err();

    server.join().unwrap();
    let message = error.to_string();
    assert!(message.contains("503 Service Unavailable"), "{message}");
    assert!(message.contains("not ready"), "{message}");
}

/// 返回 503 且 body 读取失败的假 transport。
struct ErrorRoundTripper;

impl HttpTransport for ErrorRoundTripper {
    fn put(&self, _url: url::Url) -> Result<HttpResponse> {
        Ok(HttpResponse::new(
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            Box::new(ErrReader),
        ))
    }
}

/// 读操作始终失败的假 Reader。
struct ErrReader;

impl Read for ErrReader {
    fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("read failed"))
    }
}

/// 验证读 body 失败时得到 `ManagerError::ReadResponse` 且 source 正确。
#[test]
fn test_free_returns_body_read_error() {
    let client = new_client_with_transport(
        "http://manager.example.com",
        "pod-1",
        "10.0.0.1",
        "ns-1",
        Arc::new(ErrorRoundTripper),
    )
    .unwrap();

    let error = client.free("idle restart").unwrap_err();

    let message = error.to_string();
    assert!(message.contains("503 Service Unavailable"), "{message}");
    assert!(
        message.contains("read body failed: read failed"),
        "{message}"
    );
    match error {
        ManagerError::ReadResponse { source, .. } => {
            assert_eq!(source.kind(), io::ErrorKind::Other);
            assert_eq!(source.to_string(), "read failed");
        }
        other => panic!("expected response body read error, got {other:?}"),
    }
}
