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
// Copyright 2026 AsterSQL.

// TiKV RPC 客户端追踪注入（InjectTraceClient）单元测试。
//
// 验证在同步/异步发送前，会把会话 TraceInfo（连接 ID、session_alias）写入请求的
// SourceStmt，便于链路追踪；并确认底层客户端错误原样向上传递。

use std::sync::Mutex;
use std::time::Duration;

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
struct SeenRequest {
    context: TraceContext,
    addr: String,
    request: Request,
    timeout: Duration,
}

/// 可记录收到请求、并可按开关返回错误的 Mock TiKV 客户端。
#[derive(Debug)]
struct MockTiKvClient {
    /// 为 true 时 `send_request` 返回 Backend 错误。
    expected_error: bool,
    /// 已收到的请求副本，用于断言注入是否发生在转发之前。
    seen: Mutex<Vec<SeenRequest>>,
}

impl TikvClient for MockTiKvClient {
    fn send_request(
        &self,
        context: &TraceContext,
        addr: &str,
        request: &mut Request,
        timeout: Duration,
    ) -> Result<Response, DriverError> {
        // 记录完整调用快照，验证注入发生在转发前且其余参数保持不变。
        self.seen.lock().unwrap().push(SeenRequest {
            context: context.clone(),
            addr: addr.to_owned(),
            request: request.clone(),
            timeout,
        });
        if self.expected_error {
            Err(DriverError::Backend("mockErr".into()))
        } else {
            Ok(Response {
                payload: b"response".to_vec(),
            })
        }
    }

    fn send_request_async(
        &self,
        context: &TraceContext,
        addr: &str,
        request: &mut Request,
        callback: Box<dyn FnOnce(Result<Response, DriverError>) + Send>,
    ) {
        // 异步路径复用同步发送，立即回调结果。
        callback(self.send_request(context, addr, request, Duration::ZERO));
    }
}

/// 覆盖多种 TraceInfo / 已有 SourceStmt 组合，并验证错误透传。
#[test]
fn TestInjectTracingClient() {
    // (trace_info, 请求上已有的 source_stmt)：覆盖无追踪、有连接与别名、仅连接 ID、有连接且已有 SourceStmt。
    let cases = [
        (None, None),
        (
            Some(TraceInfo {
                connection_id: 123,
                session_alias: "alias123".into(),
            }),
            None,
        ),
        (
            Some(TraceInfo {
                connection_id: 456,
                session_alias: String::new(),
            }),
            None,
        ),
        (
            Some(TraceInfo {
                connection_id: 0,
                session_alias: "alias456".into(),
            }),
            Some(SourceStmt::default()),
        ),
    ];

    for (trace_info, existing_source) in cases {
        let context = TraceContext {
            trace_info: trace_info.clone(),
        };
        let mut request = Request {
            context: RequestContext {
                source_stmt: existing_source,
            },
        };
        let client = InjectTraceClient {
            client: MockTiKvClient {
                expected_error: false,
                seen: Mutex::new(Vec::new()),
            },
        };
        let response = client
            .SendRequest(&context, "addr1", &mut request, Duration::from_secs(1))
            .unwrap();
        assert_eq!(response.payload, b"response");
        // 无 TraceInfo 时不写 SourceStmt；有则覆盖为追踪中的连接 ID 与别名。
        match trace_info {
            None => assert!(request.context.source_stmt.is_none()),
            Some(trace) => {
                let source = request.context.source_stmt.as_ref().unwrap();
                assert_eq!(source.connection_id, trace.connection_id);
                assert_eq!(source.session_alias, trace.session_alias);
            }
        }

        let seen = client.client.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].context, context);
        assert_eq!(seen[0].addr, "addr1");
        assert_eq!(seen[0].request, request);
        assert_eq!(seen[0].timeout, Duration::from_secs(1));
        drop(seen);

        // Go 用例在每种 TraceInfo / SourceStmt 组合下都验证错误原样透传。
        let failing = InjectTraceClient {
            client: MockTiKvClient {
                expected_error: true,
                seen: Mutex::new(Vec::new()),
            },
        };
        assert_eq!(
            failing
                .SendRequest(&context, "addr2", &mut request, Duration::from_secs(60),)
                .unwrap_err(),
            DriverError::Backend("mockErr".into())
        );
        let seen = failing.client.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].context, context);
        assert_eq!(seen[0].addr, "addr2");
        assert_eq!(seen[0].request, request);
        assert_eq!(seen[0].timeout, Duration::from_secs(60));
    }
}

/// 异步 SendRequestAsync 同样在转发前注入 TraceInfo。
#[test]
fn async_request_injects_trace_before_forwarding() {
    let context = TraceContext {
        trace_info: Some(TraceInfo {
            connection_id: 99,
            session_alias: "async".into(),
        }),
    };
    let mut request = Request::default();
    let client = InjectTraceClient {
        client: MockTiKvClient {
            expected_error: false,
            seen: Mutex::new(Vec::new()),
        },
    };
    client.SendRequestAsync(
        &context,
        "addr",
        &mut request,
        Box::new(|result| {
            assert!(result.is_ok());
        }),
    );
    assert_eq!(
        request.context.source_stmt,
        Some(SourceStmt {
            connection_id: 99,
            session_alias: "async".into()
        })
    );
}
