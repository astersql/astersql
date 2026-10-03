// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// etcd 客户端封装与命名空间（namespace）接入。
//
// TiDB 使用 etcd（经 PD）存放部分元数据协调信息。本模块从 `Storage` 解析 etcd 地址、
// TLS 与 keyspace 命名空间前缀，构造带 TiDB 连接策略的 `EtcdClient`。

use std::time::Duration;

use etcd_client::{Client, ConnectOptions, TlsOptions};
use etcd_dependency::{NamespacedClient, SetEtcdCliByNamespace};
use keyspace_dependency::MakeKeyspaceEtcdNamespace;

use crate::{Storage, StoreError};

/// 可提供 etcd / PD 地址与 TLS、并启动 GC Worker 的存储后端能力。
pub trait EtcdBackend: Send + Sync {
    fn EtcdAddrs(&self) -> Result<Vec<String>, StoreError>;
    fn GetPDAddrs(&self) -> Result<Vec<String>, StoreError>;
    fn TLSConfig(&self) -> Option<TlsOptions>;
    fn StartGCWorker(&self) -> Result<(), StoreError>;
}

/// Values supplied to Go's clientv3 and gRPC configuration. `etcd-client`
/// delegates endpoint balancing and reconnect backoff to tonic, while these
/// values retain TiDB's explicit policy at the package boundary.
/// 对应 Go clientv3 / gRPC 的连接策略：拨号超时、保活、退避等。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EtcdClientSettings {
    pub auto_sync_interval: Duration,
    pub dial_timeout: Duration,
    pub backoff_max_delay: Duration,
    pub keep_alive_interval: Duration,
    pub keep_alive_timeout: Duration,
}

impl Default for EtcdClientSettings {
    fn default() -> Self {
        Self {
            auto_sync_interval: Duration::from_secs(30),
            dial_timeout: Duration::from_secs(5),
            backoff_max_delay: Duration::from_secs(3),
            keep_alive_interval: Duration::from_secs(10),
            keep_alive_timeout: Duration::from_secs(3),
        }
    }
}

/// 带命名空间前缀的 etcd 客户端包装。
pub struct EtcdClient {
    inner: NamespacedClient<Client>,
    endpoints: Vec<String>,
    settings: EtcdClientSettings,
}

impl EtcdClient {
    /// Keep writes and reads inside the store codec's namespace.
    pub async fn Put(&mut self, key: &str, value: &[u8]) -> Result<(), StoreError> {
        self.inner
            .put(key.as_bytes().to_vec(), value.to_vec(), None)
            .await
            .map(|_| ())
            .map_err(|error| StoreError::other(format!("put etcd key: {error}")))
    }
    pub async fn Get(
        &mut self,
        key: &str,
        prefix: bool,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, StoreError> {
        self.inner
            .get(key.as_bytes().to_vec(), prefix)
            .await
            .map_err(|error| StoreError::other(format!("get etcd key: {error}")))
    }
    /// 只读访问底层 etcd `Client`。
    pub fn inner(&self) -> &Client {
        self.inner.inner()
    }

    /// 可变访问底层 etcd `Client`。
    pub fn inner_mut(&mut self) -> &mut Client {
        self.inner.inner_mut()
    }

    /// keyspace 命名空间前缀字节。
    pub fn namespace_prefix(&self) -> &[u8] {
        self.inner.namespace_prefix()
    }

    /// 连接使用的 etcd 端点列表。
    pub fn endpoints(&self) -> &[String] {
        &self.endpoints
    }

    /// 当前客户端连接设置。
    pub fn settings(&self) -> &EtcdClientSettings {
        &self.settings
    }
}

/// 从可选 Storage 创建 etcd 客户端；无地址时返回 `None`，并按 keyspace 设置命名空间。
pub async fn NewEtcdCli(store: Option<&dyn Storage>) -> Result<Option<EtcdClient>, StoreError> {
    let (backend, addrs) = GetEtcdAddrs(store)?;
    if addrs.is_empty() {
        return Ok(None);
    }

    let backend = backend.expect("non-empty etcd addresses come from an etcd backend");
    let mut client = NewEtcdCliWithAddrs(addrs, backend).await?;
    if let Some(store) = store {
        let namespace = EtcdNamespace(store);
        if !namespace.is_empty() {
            // 多租户 / keyspace 下隔离 etcd 键前缀。
            SetEtcdCliByNamespace(&mut client.inner, &namespace);
        }
    }
    Ok(Some(client))
}

/// 解析 etcd 后端与地址；无 store 或非 EtcdBackend 时返回空列表。
pub fn GetEtcdAddrs(
    store: Option<&dyn Storage>,
) -> Result<(Option<&dyn EtcdBackend>, Vec<String>), StoreError> {
    let Some(store) = store else {
        return Ok((None, Vec::new()));
    };
    let Some(backend) = store.AsEtcdBackend() else {
        return Ok((None, Vec::new()));
    };
    let addrs = backend.EtcdAddrs()?;
    Ok((Some(backend), addrs))
}

/// 由 Storage 的 codec 生成 keyspace etcd 命名空间字符串。
pub fn EtcdNamespace(store: &dyn Storage) -> String {
    MakeKeyspaceEtcdNamespace(store.GetCodec())
}

/// 使用默认 `EtcdClientSettings` 连接指定地址。
pub async fn NewEtcdCliWithAddrs(
    addrs: Vec<String>,
    backend: &dyn EtcdBackend,
) -> Result<EtcdClient, StoreError> {
    NewEtcdCliWithSettings(addrs, backend, EtcdClientSettings::default()).await
}

/// 按显式设置构造连接选项（超时/保活/TLS）并连接 etcd。
pub async fn NewEtcdCliWithSettings(
    addrs: Vec<String>,
    backend: &dyn EtcdBackend,
    settings: EtcdClientSettings,
) -> Result<EtcdClient, StoreError> {
    let mut options = ConnectOptions::new()
        .with_connect_timeout(settings.dial_timeout)
        .with_keep_alive(settings.keep_alive_interval, settings.keep_alive_timeout)
        .with_keep_alive_while_idle(true);
    if let Some(tls) = backend.TLSConfig() {
        options = options.with_tls(tls);
    }

    let client = Client::connect(addrs.clone(), Some(options))
        .await
        .map_err(|error| StoreError::other(format!("create etcd client: {error}")))?;
    Ok(EtcdClient {
        inner: NamespacedClient::new(client),
        endpoints: addrs,
        settings,
    })
}
