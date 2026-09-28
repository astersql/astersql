// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// Mock KV Client（键值存储客户端）实现。
//
// 对应 Go `client.go`：测试中用固定/共享响应替代真实 DistSQL/KV 请求发送。

use std::any::Any;
use std::sync::{Arc, Mutex};

use crate::kv;

/// 内部共享响应句柄类型别名。
type ResponseInner = Arc<Mutex<Box<dyn kv::Response + Send>>>;

/// 共享响应：保留 Go 接口值语义，每次 Send 返回同一配置响应对象的另一句柄。
/// SharedResponse preserves Go interface-value semantics: every Send call
/// returns another handle to the same configured response object.
#[derive(Clone)]
pub struct SharedResponse {
    inner: ResponseInner,
}

impl SharedResponse {
    /// 用已有响应构造共享包装。
    pub fn new(response: Box<dyn kv::Response + Send>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(response)),
        }
    }

    /// 判断两个句柄是否指向同一底层 `Arc`。
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl kv::Response for SharedResponse {
    fn Next(
        &mut self,
        ctx: &kv::context::Context,
    ) -> Result<Option<Box<dyn kv::ResultSubset>>, kv::errors::SharedError> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .Next(ctx)
    }

    fn Close(&mut self) -> Result<(), kv::errors::SharedError> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .Close()
    }
}

/// 测试用 KV 客户端，对应 Go `client.go` 中的 Client。
/// Client is the test KV client from client.go.
pub struct Client {
    pub RequestTypeSupportedChecker: kv::RequestTypeSupportedChecker,
    pub MockResponse: Option<SharedResponse>,
}

impl Default for Client {
    fn default() -> Self {
        Self {
            RequestTypeSupportedChecker: kv::RequestTypeSupportedChecker,
            MockResponse: None,
        }
    }
}

impl Client {
    /// 构造并挂载共享 MockResponse 的客户端。
    pub fn new(response: Box<dyn kv::Response + Send>) -> Self {
        Self {
            RequestTypeSupportedChecker: kv::RequestTypeSupportedChecker,
            MockResponse: Some(SharedResponse::new(response)),
        }
    }

    /// 返回已配置共享响应的 trait 对象句柄。
    /// Returns a new trait-object handle to the configured shared response.
    pub fn SendMockResponse(&self) -> Option<Box<dyn kv::Response>> {
        self.MockResponse
            .as_ref()
            .cloned()
            .map(|response| Box::new(response) as Box<dyn kv::Response>)
    }
}

impl kv::Client for Client {
    fn Send(
        &self,
        _ctx: &kv::context::Context,
        _req: &kv::Request,
        _vars: &dyn Any,
        _option: &kv::ClientSendOption,
    ) -> Option<Box<dyn kv::Response>> {
        self.SendMockResponse()
    }

    fn IsRequestTypeSupported(&self, req_type: i64, sub_type: i64) -> bool {
        self.RequestTypeSupportedChecker
            .IsRequestTypeSupported(req_type, sub_type)
    }
}
