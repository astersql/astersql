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

// httputil 迁移回归测试：与 Go 用例对齐的成功/错误路径。
//
// 覆盖 `NewClient`（含自定义 TLS builder）、`GetJSON`（连接失败、200 解码、
// 204/503 状态与 body 文案）以及 `GetText` 完整响应读取。

use super::{GetJSON, GetText, NewClient};
use serde::Deserialize;
use std::thread::{self, JoinHandle};
use tiny_http::{Header, Method, Response, Server, StatusCode};

/// 与 Go 测试一致的 JSON 载荷结构。
#[derive(Debug, Deserialize, PartialEq)]
struct TestPayload {
    username: String,
    password: String,
}

/// 启动一次性 GET 服务：返回指定状态码与 body，并给出访问 URL。
fn serve_once(status: u16, body: &'static str) -> (String, JoinHandle<()>) {
    let server = Server::http("127.0.0.1:0").expect("bind test server");
    let url = format!("http://{}", server.server_addr());
    let handle = thread::spawn(move || {
        let request = server.recv().expect("receive request");
        assert_eq!(request.method(), &Method::Get);
        let response = Response::from_string(body)
            .with_status_code(StatusCode(status))
            .with_header(
                Header::from_bytes("content-type", "application/json")
                    .expect("valid content-type header"),
            );
        request.respond(response).expect("send response");
    });
    (url, handle)
}

/// GetJSON：连接失败、200 成功、204（空 body 文案）与 503（含 body）错误消息对齐 Go。
#[test]
fn migration_get_json_matches_go_success_and_error_paths() {
    let client = NewClient(None).expect("build HTTP client");

    let connection_error = GetJSON::<TestPayload>(&client, "http://127.0.0.1:1");
    assert!(connection_error.is_err());

    let (url, handle) = serve_once(
        200,
        r#"{"username":"lightning","password":"lightning-ctl"}"#,
    );
    let response = GetJSON::<TestPayload>(&client, &url).expect("decode JSON response");
    handle.join().expect("server thread");
    assert_eq!(
        response,
        TestPayload {
            username: "lightning".to_owned(),
            password: "lightning-ctl".to_owned()
        }
    );

    let (url, handle) = serve_once(204, "not available");
    let error = GetJSON::<TestPayload>(&client, &url).expect_err("reject non-200 status");
    handle.join().expect("server thread");
    assert_eq!(
        error.to_string(),
        format!("get {url} http status code != 200, message ")
    );

    let (url, handle) = serve_once(503, "not available");
    let error = GetJSON::<TestPayload>(&client, &url).expect_err("include non-200 body");
    handle.join().expect("server thread");
    assert_eq!(
        error.to_string(),
        format!("get {url} http status code != 200, message not available")
    );
}

/// GetText 应返回完整响应体（路径后缀不影响 tiny_http 根绑定）。
#[test]
fn migration_get_text_reads_the_complete_response() {
    let (url, handle) = serve_once(200, "test-content");
    let client = NewClient(None).expect("build HTTP client");
    let text = GetText(&client, &(url + "/test")).expect("read text response");
    handle.join().expect("server thread");
    assert_eq!(text, "test-content");
}

/// NewClient 可消费调用方提供的 TLS/ClientBuilder 配置。
#[test]
fn migration_new_client_accepts_custom_tls_builder() {
    let builder = reqwest::blocking::Client::builder().danger_accept_invalid_certs(true);
    NewClient(Some(builder)).expect("build client from caller TLS configuration");
}
