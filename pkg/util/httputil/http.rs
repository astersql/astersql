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

// HTTP 客户端辅助：固定超时的 GET，以及 JSON/文本响应解码。
//
// 对应 Go `pkg/util/httputil`。提供与 Go 一致的 30 秒超时客户端、`GetJSON`
//（流式解码首个 JSON 值）与 `GetText`；非 200 状态会带上 URL 与响应体报错。

#![allow(non_snake_case)]

use reqwest::blocking::{Client, ClientBuilder, Response};
use serde::de::DeserializeOwned;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::time::Duration;

/// 默认请求/连接池空闲超时：与 Go 侧 30 秒一致。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Errors returned by the HTTP helpers.
/// HTTP 辅助函数返回的错误：传输失败、JSON 解码失败或非 200 状态。
#[derive(Debug)]
pub enum HttpUtilError {
    /// reqwest 传输/构建错误。
    Request(reqwest::Error),
    /// 响应体 JSON 反序列化失败。
    Json(serde_json::Error),
    /// HTTP 状态码非 200；携带请求 URL 与响应体文本。
    Status { url: String, body: String },
}

impl Display for HttpUtilError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(error) => Display::fmt(error, formatter),
            Self::Json(error) => Display::fmt(error, formatter),
            Self::Status { url, body } => {
                write!(
                    formatter,
                    "get {url} http status code != 200, message {body}"
                )
            }
        }
    }
}

impl Error for HttpUtilError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Request(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Status { .. } => None,
        }
    }
}

impl From<reqwest::Error> for HttpUtilError {
    fn from(error: reqwest::Error) -> Self {
        Self::Request(error)
    }
}

impl From<serde_json::Error> for HttpUtilError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// The caller may supply a preconfigured builder for TLS certificates,
/// identities, or protocol policy. It is consumed just like Go's cloned TLS
/// transport configuration.
/// 调用方可传入预配置的 `ClientBuilder`（TLS 证书、身份、协议策略等），
/// 语义对应 Go 侧克隆后的 TLS transport 配置。
pub type TlsConfig = ClientBuilder;

/// Returns an HTTP(S) client with the same 30-second request timeout as Go.
/// 创建带 30 秒超时（及相同空闲池超时）的 HTTP(S) 客户端。
pub fn NewClient(tls_conf: Option<TlsConfig>) -> Result<Client, HttpUtilError> {
    let builder = tls_conf.unwrap_or_else(Client::builder);
    Ok(builder
        .timeout(DEFAULT_TIMEOUT)
        .pool_idle_timeout(DEFAULT_TIMEOUT)
        .build()?)
}

/// Fetches a page and decodes the first JSON value from its response body.
///
/// Deserializing directly from the response mirrors Go's `Decoder.Decode`: a
/// valid first JSON value is sufficient and the response is closed on return.
/// GET 指定 URL 并从响应体解码第一个 JSON 值（对齐 Go `json.Decoder.Decode`）。
pub fn GetJSON<T>(client: &Client, url: &str) -> Result<T, HttpUtilError>
where
    T: DeserializeOwned,
{
    let response = doGet(client, url)?;
    // 流式反序列化：只要首个 JSON 值合法即可，函数返回时关闭响应。
    let mut decoder = serde_json::Deserializer::from_reader(response);
    Ok(T::deserialize(&mut decoder)?)
}

/// Fetches a page and returns the complete response body as text.
/// GET 指定 URL，将完整响应体按有损 UTF-8 转为字符串返回。
pub fn GetText(client: &Client, url: &str) -> Result<String, HttpUtilError> {
    let response = doGet(client, url)?;
    let data = response.bytes()?;
    Ok(String::from_utf8_lossy(&data).into_owned())
}

/// 发起 GET；仅当状态码为 200 时返回响应，否则包装为 `Status` 错误。
fn doGet(client: &Client, url: &str) -> Result<Response, HttpUtilError> {
    let response = client.get(url).send()?;
    // 非 200：读取 body 以便错误消息与 Go 文案一致。
    if response.status() != reqwest::StatusCode::OK {
        let body = String::from_utf8_lossy(&response.bytes()?).into_owned();
        return Err(HttpUtilError::Status {
            url: url.to_owned(),
            body,
        });
    }
    Ok(response)
}
