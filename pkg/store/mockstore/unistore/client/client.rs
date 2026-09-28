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

// UniStore 客户端抽象：发送 RPC 请求的最小接口边界。
//
// 刻意不绑定具体协议类型，对应 Go 侧避免循环依赖的接口角色；由实现方
// 自行选定 Context / Request / Response / Error。

use std::error::Error as StdError;
use std::time::Duration;

/// A client that sends RPC requests.
///
/// This boundary intentionally owns no concrete RPC protocol types, matching
/// the Go interface's role of avoiding a circular dependency. Implementations
/// select their context, request, response, and error types.
///
/// 发送 RPC 请求的客户端 trait；关联类型由实现方决定。
pub trait Client {
    /// 请求上下文（如超时取消、追踪 ID）。
    type Context: ?Sized;
    /// 请求体类型。
    type Request: ?Sized;
    /// 响应体类型。
    type Response;
    /// 错误类型，需实现 StdError 且可跨线程传递。
    type Error: StdError + Send + Sync + 'static;

    /// Releases all data and connections held by the client.
    /// 释放客户端持有的数据与连接。
    fn close(&mut self) -> Result<(), Self::Error>;

    /// Sends one request to `address`, bounded by `timeout` and `context`.
    /// 向 `address` 发送一次请求，受 `timeout` 与 `context` 约束。
    fn send_request(
        &self,
        context: &Self::Context,
        address: &str,
        request: &Self::Request,
        timeout: Duration,
    ) -> Result<Self::Response, Self::Error>;
}
