// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// RPC 请求重定向器：按目标把请求发往 mock 客户端或真实网络客户端。
//
// TiKV / TiFlash 请求留在 mock 侧；发往 TiDB 的 RPC 延迟构造网络客户端。
// TiFlash 是列存分析引擎；TiKV 是行存 KV 引擎。

use crate::embedded_unistore::rpc::{RPCClient, Request, Response, RpcError};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// 请求最终投递的存储/服务目标。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreTarget {
    /// 行存 KV（TiKV）。
    TiKv,
    /// 列存分析引擎（TiFlash）。
    TiFlash,
    /// SQL 层服务（TiDB），走网络 RPC 客户端。
    TiDb,
}

/// 带路由目标的包装请求。
pub struct RoutedRequest {
    /// 投递目标。
    pub target: StoreTarget,
    /// 原始 RPC 请求体。
    pub request: Request,
}

/// KV RPC 客户端抽象：关闭连接、按地址发请求、可选事件监听。
pub trait KvClient: Send + Sync {
    /// 关闭客户端持有的全部连接。
    fn close(&self) -> Result<(), RpcError>;
    /// 关闭指定地址的连接。
    fn close_addr(&self, address: &str) -> Result<(), RpcError>;
    /// 向 `address` 发送请求，受 `timeout` 约束。
    fn send_request(
        &self,
        address: &str,
        request: Request,
        timeout: Duration,
    ) -> Result<Response, RpcError>;
    /// 安装事件监听器；默认空实现。
    fn set_event_listener(&self, _listener: Arc<dyn Fn(&str) + Send + Sync>) {}
}

/// 将嵌入式 `RPCClient` 适配为 `KvClient`。
impl KvClient for RPCClient {
    fn close(&self) -> Result<(), RpcError> {
        RPCClient::close(self)
    }

    fn close_addr(&self, address: &str) -> Result<(), RpcError> {
        RPCClient::close_addr(self, address)
    }

    fn send_request(
        &self,
        address: &str,
        request: Request,
        timeout: Duration,
    ) -> Result<Response, RpcError> {
        RPCClient::send_request(self, address, request, timeout)
    }
}

/// TiDB RPCs are redirected to a lazily constructed network-capable client;
/// TiKV/TiFlash requests stay on the mock client. OnceLock provides Go's
/// sync.Once guarantee even when sync and async requests race.
///
/// 将发往 TiDB 的 RPC 重定向到惰性创建的网络客户端；TiKV/TiFlash 仍走 mock。
/// `OnceLock` 对应 Go 的 `sync.Once`，避免同步/异步竞态下重复构造。
pub struct ClientRedirector {
    /// 进程内 mock KV 客户端。
    mock_client: Arc<dyn KvClient>,
    /// 惰性初始化的网络 RPC 客户端（仅 TiDB 目标使用）。
    rpc_client: OnceLock<Arc<dyn KvClient>>,
    /// 首次需要网络客户端时调用的工厂。
    rpc_factory: Arc<dyn Fn() -> Arc<dyn KvClient> + Send + Sync>,
}

impl ClientRedirector {
    /// 用 mock 客户端与 RPC 工厂构造重定向器。
    pub fn new(
        mock_client: Arc<dyn KvClient>,
        rpc_factory: Arc<dyn Fn() -> Arc<dyn KvClient> + Send + Sync>,
    ) -> Self {
        Self {
            mock_client,
            rpc_client: OnceLock::new(),
            rpc_factory,
        }
    }

    /// 惰性获取（或创建）网络 RPC 客户端。
    fn rpc_client(&self) -> Arc<dyn KvClient> {
        Arc::clone(self.rpc_client.get_or_init(|| (self.rpc_factory)()))
    }

    /// 关闭 mock 与（若已创建的）网络客户端。
    pub fn close(&self) -> Result<(), RpcError> {
        self.mock_client.close()?;
        if let Some(client) = self.rpc_client.get() {
            client.close()?;
        }
        Ok(())
    }

    /// 在两侧客户端上关闭指定地址连接。
    pub fn close_addr(&self, address: &str) -> Result<(), RpcError> {
        self.mock_client.close_addr(address)?;
        if let Some(client) = self.rpc_client.get() {
            client.close_addr(address)?;
        }
        Ok(())
    }

    /// 按 `routed.target` 选择 mock 或网络客户端发送请求。
    pub fn send_request(
        &self,
        address: &str,
        routed: RoutedRequest,
        timeout: Duration,
    ) -> Result<Response, RpcError> {
        // TiDB 走网络客户端；其余目标留在 mock。
        if routed.target == StoreTarget::TiDb {
            self.rpc_client()
                .send_request(address, routed.request, timeout)
        } else {
            self.mock_client
                .send_request(address, routed.request, timeout)
        }
    }

    /// 在独立线程中异步发送请求，完成后调用 `callback`。
    pub fn send_request_async(
        self: &Arc<Self>,
        address: String,
        routed: RoutedRequest,
        timeout: Duration,
        callback: impl FnOnce(Result<Response, RpcError>) + Send + 'static,
    ) {
        let client = Arc::clone(self);
        std::thread::spawn(move || {
            callback(client.send_request(&address, routed, timeout));
        });
    }

    /// Go installs listeners only on the mock client; a later TiDB client is
    /// intentionally unaffected.
    /// 仅在 mock 客户端上安装监听器；后续惰性创建的 TiDB 客户端不受影响。
    pub fn set_event_listener(&self, listener: Arc<dyn Fn(&str) + Send + Sync>) {
        self.mock_client.set_event_listener(listener);
    }
}

/// 构造并返回共享的 `ClientRedirector`（对应 Go `newClientRedirector`）。
pub fn newClientRedirector(
    mock_client: Arc<dyn KvClient>,
    rpc_factory: Arc<dyn Fn() -> Arc<dyn KvClient> + Send + Sync>,
) -> Arc<ClientRedirector> {
    Arc::new(ClientRedirector::new(mock_client, rpc_factory))
}
