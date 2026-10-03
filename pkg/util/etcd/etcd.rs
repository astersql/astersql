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

// etcd key 操作辅助：命名空间前缀与带超时重试的删除。
//
// 对应 Go `util/etcd`：在客户端边界统一加 namespace，删除 key 时按次超时并累计可重试错误指标。

use async_trait::async_trait;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::time::Duration;

/// 单次 key 操作默认超时。
/// Default timeout for each key operation.
pub const KEY_OP_DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);
/// 单次 key 操作默认重试次数。
/// Default number of attempts for each key operation.
pub const KEY_OP_DEFAULT_RETRY_COUNT: usize = 5;
/// 调用方在 key 操作之间使用的间隔。
/// Interval used by callers between key operations.
pub const KEY_OP_RETRY_INTERVAL: Duration = Duration::from_millis(30);

/// 删除辅助所需的最小客户端接口。
/// The small client surface required by the delete helper.
///
/// `etcd_client::Client` implements this trait directly. Keeping the boundary
/// explicit also lets focused tests verify retry semantics without running an
/// external etcd process.
#[async_trait]
pub trait DeleteClient {
    type Error: Error + Send + Sync + 'static;

    /// 删除指定 key（字节形式）。
    async fn delete(&mut self, key: Vec<u8>) -> Result<(), Self::Error>;
}

#[async_trait]
impl DeleteClient for etcd_client::Client {
    type Error = etcd_client::Error;

    async fn delete(&mut self, key: Vec<u8>) -> Result<(), Self::Error> {
        etcd_client::Client::delete(self, key, None)
            .await
            .map(|_| ())
    }
}

/// 带命名空间前缀的 etcd 客户端视图；操作前自动拼接前缀。
/// An etcd client view whose operations are rooted below a namespace prefix.
///
/// Go replaces the client's KV, Watcher, and Lease fields with namespaced
/// wrappers. The Rust client keeps those fields private, so this wrapper keeps
/// one prefix at the client boundary and applies it before an operation reaches
/// the real client.
pub struct NamespacedClient<C> {
    inner: C,
    namespace_prefix: Vec<u8>,
}

impl<C> NamespacedClient<C> {
    /// 包装底层客户端，初始无命名空间前缀。
    pub fn new(inner: C) -> Self {
        Self {
            inner,
            namespace_prefix: Vec::new(),
        }
    }

    /// 只读访问底层客户端。
    pub fn inner(&self) -> &C {
        &self.inner
    }

    /// 可变访问底层客户端。
    pub fn inner_mut(&mut self) -> &mut C {
        &mut self.inner
    }

    /// 取出底层客户端，丢弃包装。
    pub fn into_inner(self) -> C {
        self.inner
    }

    /// 当前命名空间前缀字节。
    pub fn namespace_prefix(&self) -> &[u8] {
        &self.namespace_prefix
    }

    /// 将用户 key 拼到命名空间前缀之后。
    fn prefixed_key(&self, key: Vec<u8>) -> Vec<u8> {
        let mut namespaced = Vec::with_capacity(self.namespace_prefix.len() + key.len());
        namespaced.extend_from_slice(&self.namespace_prefix);
        namespaced.extend_from_slice(&key);
        namespaced
    }
}

#[async_trait]
impl<C> DeleteClient for NamespacedClient<C>
where
    C: DeleteClient + Send,
{
    type Error = C::Error;

    async fn delete(&mut self, key: Vec<u8>) -> Result<(), Self::Error> {
        let key = self.prefixed_key(key);
        self.inner.delete(key).await
    }
}

/// 设置后续操作使用的命名空间前缀。
/// Adds an etcd namespace prefix before subsequent client operations.
pub fn set_etcd_client_namespace<C>(client: &mut NamespacedClient<C>, namespace_prefix: &str) {
    client.namespace_prefix = namespace_prefix.as_bytes().to_vec();
}

#[derive(Debug)]
/// 删除 key 失败：底层客户端错误或单次尝试超时。
pub enum DeleteKeyError<E> {
    Client(E),
    Timeout(Duration),
}

impl<E: Display> Display for DeleteKeyError<E> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Client(error) => Display::fmt(error, formatter),
            Self::Timeout(timeout) => write!(formatter, "etcd delete timed out after {timeout:?}"),
        }
    }
}

impl<E> Error for DeleteKeyError<E>
where
    E: Error + Send + Sync + 'static,
{
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Client(error) => Some(error),
            Self::Timeout(_) => None,
        }
    }
}

/// 删除 key：每次尝试独立超时；无内部 sleep；失败记指标并打日志；成功立即返回；retry_count=0 为空操作成功。
/// Deletes a key, giving every attempt its own timeout.
///
/// This follows the Go loop exactly: there is no sleep inside this helper, all
/// failures increment the retryable-error metric and are logged, success stops
/// immediately, and a zero retry count is a no-op success.
pub async fn delete_key_from_etcd<C>(
    key: &str,
    client: &mut C,
    retry_count: usize,
    timeout: Duration,
) -> Result<(), DeleteKeyError<C::Error>>
where
    C: DeleteClient + Send,
{
    let mut last_error = None;

    // 与 Go 一致：循环内不 sleep，由调用方用 KEY_OP_RETRY_INTERVAL 控制节奏。
    for retry in 0..retry_count {
        let result = tokio::time::timeout(timeout, client.delete(key.as_bytes().to_vec())).await;
        let error = match result {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(error)) => DeleteKeyError::Client(error),
            Err(_) => DeleteKeyError::Timeout(timeout),
        };

        let error_message = error.to_string();
        metrics::counter!(
            "tidb_retryable_error_total",
            "error" => error_message.clone()
        )
        .increment(1);
        tracing::warn!(key, retry, error = %error_message, "etcd-cli delete key failed");
        last_error = Some(error);
    }

    match last_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[allow(non_snake_case)]
/// Go 风格导出名：设置 etcd 客户端命名空间。
pub fn SetEtcdCliByNamespace<C>(client: &mut NamespacedClient<C>, namespace_prefix: &str) {
    set_etcd_client_namespace(client, namespace_prefix);
}

#[allow(non_snake_case)]
/// Go 风格导出名：带重试删除 etcd key。
pub async fn DeleteKeyFromEtcd<C>(
    key: &str,
    client: &mut C,
    retry_count: usize,
    timeout: Duration,
) -> Result<(), DeleteKeyError<C::Error>>
where
    C: DeleteClient + Send,
{
    delete_key_from_etcd(key, client, retry_count, timeout).await
}

#[allow(non_upper_case_globals)]
/// Go 风格常量别名：默认超时。
pub const KeyOpDefaultTimeout: Duration = KEY_OP_DEFAULT_TIMEOUT;
#[allow(non_upper_case_globals)]
/// Go 风格常量别名：默认重试次数。
pub const KeyOpDefaultRetryCnt: usize = KEY_OP_DEFAULT_RETRY_COUNT;
#[allow(non_upper_case_globals)]
/// Go 风格常量别名：重试间隔。
pub const KeyOpRetryInterval: Duration = KEY_OP_RETRY_INTERVAL;

/// Namespaced KV operations used by the store's existing etcd client.
impl NamespacedClient<etcd_client::Client> {
    pub async fn put(
        &mut self,
        key: Vec<u8>,
        value: Vec<u8>,
        options: Option<etcd_client::PutOptions>,
    ) -> Result<etcd_client::PutResponse, etcd_client::Error> {
        let key = self.prefixed_key(key);
        self.inner.put(key, value, options).await
    }
    pub async fn get(
        &mut self,
        key: Vec<u8>,
        prefix: bool,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, etcd_client::Error> {
        let key = self.prefixed_key(key);
        let response = self
            .inner
            .get(
                key,
                prefix.then(|| etcd_client::GetOptions::new().with_prefix()),
            )
            .await?;
        Ok(response
            .kvs()
            .iter()
            .map(|kv| {
                (
                    kv.key()
                        .strip_prefix(self.namespace_prefix.as_slice())
                        .unwrap_or(kv.key())
                        .to_vec(),
                    kv.value().to_vec(),
                )
            })
            .collect())
    }
}
