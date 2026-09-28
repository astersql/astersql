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

// Client trait 迁移单元测试：用 MockClient 校验请求透传与错误/关闭语义。

use std::cell::{Cell, RefCell};
use std::fmt;
use std::time::Duration;

use super::Client;

/// 测试用错误类型。
#[derive(Debug, Eq, PartialEq)]
struct TestError(&'static str);

impl fmt::Display for TestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for TestError {}

/// 测试用请求上下文，携带 request_id。
#[derive(Debug)]
struct TestContext {
    /// 请求追踪 ID。
    request_id: u64,
}

/// 测试用请求体（原始字节）。
#[derive(Debug, Eq, PartialEq)]
struct TestRequest(Vec<u8>);

/// 测试用响应体（原始字节）。
#[derive(Debug, Eq, PartialEq)]
struct TestResponse(Vec<u8>);

/// 记录一次已发送请求的快照，便于断言。
#[derive(Debug, Eq, PartialEq)]
struct SentRequest {
    /// 来自上下文的 request_id。
    request_id: u64,
    /// 目标地址。
    address: String,
    /// 请求载荷副本。
    payload: Vec<u8>,
    /// 超时设置。
    timeout: Duration,
}

/// 实现 Client 的简易 mock：可模拟发送失败并记录调用。
#[derive(Default)]
struct MockClient {
    /// 是否已 close。
    closed: bool,
    /// 为 true 时 send_request 返回错误。
    fail_send: Cell<bool>,
    /// 已发送请求历史。
    sent: RefCell<Vec<SentRequest>>,
}

impl Client for MockClient {
    type Context = TestContext;
    type Request = TestRequest;
    type Response = TestResponse;
    type Error = TestError;

    fn close(&mut self) -> Result<(), Self::Error> {
        self.closed = true;
        Ok(())
    }

    fn send_request(
        &self,
        context: &Self::Context,
        address: &str,
        request: &Self::Request,
        timeout: Duration,
    ) -> Result<Self::Response, Self::Error> {
        // 注入发送失败路径。
        if self.fail_send.get() {
            return Err(TestError("send failed"));
        }

        // 记录上下文、地址、载荷与超时，响应回显请求体。
        self.sent.borrow_mut().push(SentRequest {
            request_id: context.request_id,
            address: address.to_owned(),
            payload: request.0.clone(),
            timeout,
        });
        Ok(TestResponse(request.0.clone()))
    }
}

/// 校验 send_request 完整保留上下文、地址、请求体与超时。
#[test]
fn send_request_preserves_context_address_request_and_timeout() {
    let client = MockClient::default();
    let context = TestContext { request_id: 42 };
    let request = TestRequest(vec![1, 2, 3]);
    let timeout = Duration::from_millis(750);

    let response = client
        .send_request(&context, "127.0.0.1:20160", &request, timeout)
        .expect("request should succeed");

    assert_eq!(response, TestResponse(vec![1, 2, 3]));
    assert_eq!(
        client.sent.into_inner(),
        vec![SentRequest {
            request_id: 42,
            address: "127.0.0.1:20160".to_owned(),
            payload: vec![1, 2, 3],
            timeout,
        }]
    );
}

/// 校验发送错误原样返回，且 close 后标记已释放。
#[test]
fn close_releases_client_and_send_errors_are_preserved() {
    let mut client = MockClient::default();
    client.fail_send.set(true);

    let error = client
        .send_request(
            &TestContext { request_id: 7 },
            "store-1",
            &TestRequest(Vec::new()),
            Duration::from_secs(1),
        )
        .expect_err("mock transport error should be returned unchanged");
    assert_eq!(error, TestError("send failed"));

    client.close().expect("close should succeed");
    assert!(client.closed);
}
