// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// `httputil` 单元测试：覆盖 `GetJSON` / `GetText` 成功与失败路径。
//
// 使用本地 `tiny_http` 临时端口模拟服务端，验证连接失败、200 解码成功、
// 以及非 200 状态拒绝等与 Go 测试对齐的行为。

use super::{GetJSON, GetText};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::thread;
use std::time::Duration;
use tiny_http::{Response, Server, StatusCode};

/// 测试用 JSON 载荷，字段名与 Go 用例一致。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct TestPayload {
    username: String,
    password: String,
}

// Mirrors TestGetJSON: connection failure, successful decoding, and a non-200
// response from the same test server are all observable behaviors.
/// 验证 GetJSON：连不上的端口、200 解码、以及同服务器后续 204 被拒绝。
#[test]
fn test_get_json() {
    let request = TestPayload {
        username: "lightning".to_owned(),
        password: "lightning-ctl".to_owned(),
    };
    // 绑定临时端口；服务端依次返回 200 JSON 与 204 空响应。
    let server = Server::http("127.0.0.1:0").expect("start test server");
    let url = format!("http://{}", server.server_addr());
    let response_body = serde_json::to_string(&request).expect("encode test payload");
    let server_thread = thread::spawn(move || {
        let first = server.recv().expect("receive JSON request");
        first
            .respond(Response::from_string(response_body).with_status_code(StatusCode(200)))
            .expect("send JSON response");

        let second = server.recv().expect("receive no-content request");
        second
            .respond(Response::empty(StatusCode(204)))
            .expect("send no-content response");
    });

    let client = Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .expect("build HTTP client");

    // 未监听端口：应得到非空连接错误。
    let connection_error = GetJSON::<TestPayload>(&client, "http://localhost:1")
        .expect_err("closed local port must fail");
    assert!(!connection_error.to_string().is_empty());

    let response = GetJSON::<TestPayload>(&client, &url).expect("decode successful response");
    assert_eq!(request, response);

    // 204 No Content：doGet 要求 200，应拒绝并带状态码文案。
    let status_error =
        GetJSON::<TestPayload>(&client, &url).expect_err("204 response must be rejected");
    assert!(
        status_error.to_string().contains("http status code != 200"),
        "unexpected error: {status_error}"
    );
    server_thread.join().expect("test server thread");
}

// Mirrors TestGetText, including the request path and complete response body.
/// 验证 GetText 读取完整响应体，并保留请求路径 `/test`。
#[test]
fn test_get_text() {
    let server = Server::http("127.0.0.1:0").expect("start test server");
    let url = format!("http://{}/test", server.server_addr());
    let server_thread = thread::spawn(move || {
        let request = server.recv().expect("receive text request");
        assert_eq!("/test", request.url());
        request
            .respond(Response::from_string("test-content").with_status_code(StatusCode(200)))
            .expect("send text response");
    });

    let client = Client::new();
    let text = GetText(&client, &url).expect("read successful response");
    assert_eq!("test-content", text);
    server_thread.join().expect("test server thread");
}
