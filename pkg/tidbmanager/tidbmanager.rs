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

// TiDB Manager HTTP 客户端：通知 Manager 当前 Pod 可被回收复用（`free`）。
//
// 通过可注入的 `HttpTransport` 对齐 Go 侧 `http.RoundTripper` 测试边界。

use std::error::Error as StdError;
use std::io::{self, Read};
use std::sync::Arc;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::blocking::{Client as HttpClient, ClientBuilder};
use thiserror::Error;
use url::Url;

/// `free` API 的路径。
pub const FREE_REQ_PATH: &str = "/api/tidb/free";
/// 默认 HTTP 超时（秒）。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

type BoxError = Box<dyn StdError + Send + Sync>;
/// Manager 客户端统一结果类型。
pub type Result<T> = std::result::Result<T, ManagerError>;

/// 与 Manager 通信过程中的错误分类。
#[derive(Debug, Error)]
pub enum ManagerError {
    #[error("create request failed: {0}")]
    CreateRequest(#[source] url::ParseError),

    #[error("create HTTP client failed: {0}")]
    CreateClient(#[source] reqwest::Error),

    #[error("free request failed: {source}")]
    FreeRequest {
        #[source]
        source: BoxError,
    },

    #[error("free response failed: {status}, err: {body}")]
    FreeResponse { status: String, body: String },

    #[error(
        "free response failed: {status}, read body failed: {source}, partial body: {partial_body}"
    )]
    ReadResponse {
        status: String,
        #[source]
        source: io::Error,
        partial_body: String,
    },
}

/// A response abstraction keeps the production HTTP client testable at the
/// same response-body boundary as Go's `http.RoundTripper`.
/// HTTP 响应抽象：状态码 + 可读 body，便于测试注入。
pub struct HttpResponse {
    status: StatusCode,
    body: Box<dyn Read + Send>,
}

impl HttpResponse {
    /// 构造响应包装。
    pub fn new(status: StatusCode, body: Box<dyn Read + Send>) -> Self {
        Self { status, body }
    }
}

/// HTTP 传输层抽象（对齐 Go `RoundTripper`），当前仅需 PUT。
pub trait HttpTransport: Send + Sync {
    fn put(&self, url: Url) -> Result<HttpResponse>;
}

/// 基于 reqwest blocking 客户端的默认传输实现。
struct ReqwestTransport {
    client: HttpClient,
}

impl HttpTransport for ReqwestTransport {
    fn put(&self, url: Url) -> Result<HttpResponse> {
        let response = self
            .client
            .put(url)
            .send()
            .map_err(|source| ManagerError::FreeRequest {
                source: Box::new(source),
            })?;
        Ok(HttpResponse::new(response.status(), Box::new(response)))
    }
}

/// TiDB Manager 客户端能力面。
pub trait Client {
    /// Notifies the manager that the pod is ready to be reused.
    /// 通知 Manager：当前 Pod 已可回收复用；`exit_reason` 写入重启日志字段。
    fn free(&self, exit_reason: &str) -> Result<()>;
}

/// 持有 Manager 地址、Pod 身份与传输层的具体客户端。
pub struct ManagerClient {
    transport: Arc<dyn HttpTransport>,
    manager_addr: String,
    pod_name: String,
    pod_ip: String,
    namespace: String,
}

impl Client for ManagerClient {
    fn free(&self, exit_reason: &str) -> Result<()> {
        // 组装 `/api/tidb/free` 查询参数，与 Go 侧字段名保持一致。
        let mut url = Url::parse(&format!("{}{}", self.manager_addr, FREE_REQ_PATH))
            .map_err(ManagerError::CreateRequest)?;
        url.query_pairs_mut()
            .append_pair("pod_name", &self.pod_name)
            .append_pair("pod_ip", &self.pod_ip)
            .append_pair("ns", &self.namespace)
            .append_pair("normal_restart_log", exit_reason);

        let mut response = self.transport.put(url)?;
        if response.status == StatusCode::OK {
            return Ok(());
        }

        // 非 OK：尽量读完 body；读失败则保留部分内容与 IO 错误。
        let status = status_text(response.status);
        let mut body = Vec::new();
        if let Err(source) = response.body.read_to_end(&mut body) {
            return Err(ManagerError::ReadResponse {
                status,
                source,
                partial_body: String::from_utf8_lossy(&body).into_owned(),
            });
        }
        Err(ManagerError::FreeResponse {
            status,
            body: String::from_utf8_lossy(&body).into_owned(),
        })
    }
}

/// 将状态码格式化为 `"{code} {reason}"` 文本（无 reason 时仅数字）。
fn status_text(status: StatusCode) -> String {
    match status.canonical_reason() {
        Some(reason) => format!("{} {reason}", status.as_u16()),
        None => status.as_u16().to_string(),
    }
}

/// Creates a manager client. Passing a builder selects HTTPS and allows the
/// caller to provide the same TLS customization represented by Go's
/// `*tls.Config`; `None` selects HTTP.
/// 创建 Manager 客户端：`Some(builder)` 走 HTTPS（可定制 TLS），`None` 走 HTTP。
pub fn new_client(
    addr: &str,
    tls_config: Option<ClientBuilder>,
    pod_name: &str,
    pod_ip: &str,
    namespace: &str,
) -> Result<ManagerClient> {
    let uses_tls = tls_config.is_some();
    let client = tls_config
        .unwrap_or_else(HttpClient::builder)
        .timeout(DEFAULT_TIMEOUT)
        .build()
        .map_err(ManagerError::CreateClient)?;
    let scheme = if uses_tls { "https://" } else { "http://" };
    new_client_with_transport(
        &format!("{scheme}{}", addr.trim_end_matches('/')),
        pod_name,
        pod_ip,
        namespace,
        Arc::new(ReqwestTransport { client }),
    )
}

/// 使用自定义传输层构造客户端（测试注入用）。
pub fn new_client_with_transport(
    manager_addr: &str,
    pod_name: &str,
    pod_ip: &str,
    namespace: &str,
    transport: Arc<dyn HttpTransport>,
) -> Result<ManagerClient> {
    Ok(ManagerClient {
        transport,
        manager_addr: manager_addr.trim_end_matches('/').to_owned(),
        pod_name: pod_name.to_owned(),
        pod_ip: pod_ip.to_owned(),
        namespace: namespace.to_owned(),
    })
}
