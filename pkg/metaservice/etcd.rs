// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// etcd / PD 元数据服务客户端：解析 PD 成员 URL 并带回退重试。
//
// PD（Placement Driver）是集群调度与成员发现中心；本模块持有调用方传入的 etcd 客户端
// 与 PD 客户端，提供 GetPDAddrs / GetPDServiceURLs，并在拉取成员失败时指数退避重试。

use super::{MetaServiceError, ServiceClient};
use std::any::Any;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// 拉取 PD 全部成员时的最长累计退避预算（毫秒）；耗尽后返回 region unavailable。
pub const GET_ALL_MEMBERS_BACKOFF_MS: u64 = 5_000;
const REGION_MISS_BACKOFF_BASE_MS: u64 = 2;
const REGION_MISS_BACKOFF_CAP_MS: u64 = 500;

/// 可取消的轻量上下文，供 PD 成员查询循环检查取消标志。
#[derive(Clone, Default)]
pub struct Context {
    /// 取消标志；`cancel` 写入后查询循环应尽快退出。
    cancelled: Arc<(Mutex<bool>, Condvar)>,
    cancellation_checker: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

impl Context {
    /// Retain cancellation from an owning component without a polling thread.
    pub fn with_cancellation_checker(checker: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self {
            cancellation_checker: Some(Arc::new(checker)),
            ..Default::default()
        }
    }
    /// 标记上下文已取消。
    pub fn cancel(&self) {
        let (cancelled, wake) = &*self.cancelled;
        let mut cancelled = cancelled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *cancelled = true;
        wake.notify_all();
    }

    /// 是否已被取消。
    pub fn is_cancelled(&self) -> bool {
        if self
            .cancellation_checker
            .as_ref()
            .is_some_and(|checker| checker())
        {
            return true;
        }
        let (cancelled, _) = &*self.cancelled;
        *cancelled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 等待一次退避；取消时立即唤醒并返回 false。
    fn wait_backoff(&self, duration: Duration) -> bool {
        if self.cancellation_checker.is_some() {
            let deadline = std::time::Instant::now() + duration;
            while !self.is_cancelled() {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    return true;
                }
                std::thread::sleep(remaining.min(Duration::from_millis(10)));
            }
            return false;
        }
        let (cancelled, wake) = &*self.cancelled;
        let cancelled = cancelled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *cancelled {
            return false;
        }
        let (cancelled, _) = wake
            .wait_timeout_while(cancelled, duration, |cancelled| !*cancelled)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !*cancelled
    }
}

/// PD 成员信息；仅保留 client_urls 供解析可用地址。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PdMember {
    /// PD 客户端可连接的 URL 列表（通常含 http/https scheme）。
    pub client_urls: Vec<String>,
}

/// PD 客户端抽象：查询当前全部成员。
pub trait PdClient: Send + Sync {
    fn get_all_members(&self, ctx: &Context) -> Result<Vec<PdMember>, MetaServiceError>;
}

/// The Go implementation only stores and returns the caller-owned etcd client.
/// This marker keeps that ownership boundary without constructing or using a
/// second etcd implementation inside metaservice.
///
/// Go 侧只保存并返回调用方持有的 etcd 客户端；本 trait 作为所有权边界标记，
/// 不在 metaservice 内再构造第二套 etcd 实现。
pub trait EtcdClient: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;
}

impl<T: Any + Send + Sync> EtcdClient for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// MetaService 客户端：可选持有 PD 与 keyspace 级 etcd 客户端。
pub struct Client {
    pd_client: Option<Arc<dyn PdClient>>,
    keyspace_etcd_client: Option<Arc<dyn EtcdClient>>,
}

/// 构造 etcd MetaService 客户端；两个客户端皆空时返回 None。
pub fn new_etcd_meta_service_client(
    etcd_client: Option<Arc<dyn EtcdClient>>,
    pd_client: Option<Arc<dyn PdClient>>,
) -> Option<Client> {
    new_client(etcd_client, pd_client)
}

/// 若 etcd 与 PD 客户端均未提供则返回 None，否则包装为 Client。
pub fn new_client(
    etcd_client: Option<Arc<dyn EtcdClient>>,
    pd_client: Option<Arc<dyn PdClient>>,
) -> Option<Client> {
    if etcd_client.is_none() && pd_client.is_none() {
        return None;
    }
    Some(Client {
        pd_client,
        keyspace_etcd_client: etcd_client,
    })
}

impl Client {
    /// 返回 keyspace 绑定的 etcd 客户端引用（若有）。
    pub fn keyspace_etcd_client(&self) -> Option<&dyn EtcdClient> {
        self.keyspace_etcd_client.as_deref()
    }

    /// Go 兼容名：同 `keyspace_etcd_client`。
    #[allow(non_snake_case)]
    pub fn GetKeyspaceEtcdCli(&self) -> Option<&dyn EtcdClient> {
        self.keyspace_etcd_client()
    }

    /// Go 兼容名：返回不含 scheme 的 PD 地址列表。
    #[allow(non_snake_case)]
    pub fn GetPDAddrs(&self, ctx: &Context) -> Result<Vec<String>, MetaServiceError> {
        self.get_pd_addrs(ctx)
    }

    /// Return service URLs, including HTTP and Unix-family schemes.
    pub fn GetPDServiceURLs(&self, ctx: &Context) -> Result<Vec<String>, MetaServiceError> {
        self.get_pd_service_urls(ctx)
    }

    /// Go 兼容名：返回带 http/https scheme 的 PD 地址列表。
    #[allow(non_snake_case)]
    pub fn GetPDHttpAddrs(&self, ctx: &Context) -> Result<Vec<String>, MetaServiceError> {
        self.get_pd_http_addrs(ctx)
    }
}

impl ServiceClient for Client {
    fn get_pd_addrs(&self, ctx: &Context) -> Result<Vec<String>, MetaServiceError> {
        let pd_client = self
            .pd_client
            .as_deref()
            .ok_or(MetaServiceError::PdClientNotFound)?;
        get_pd_addrs(ctx, pd_client, false)
    }

    fn get_pd_http_addrs(&self, ctx: &Context) -> Result<Vec<String>, MetaServiceError> {
        let pd_client = self
            .pd_client
            .as_deref()
            .ok_or(MetaServiceError::PdClientNotFound)?;
        get_pd_addrs(ctx, pd_client, true)
    }
}

/// 从 PD 成员列表解析地址；失败时在退避窗口内指数退避重试。
///
/// `with_scheme` 为 true 时保留 `http://` / `https://` 前缀，否则只返回 host:port。
pub fn get_pd_addrs(
    ctx: &Context,
    pd_client: &dyn PdClient,
    with_scheme: bool,
) -> Result<Vec<String>, MetaServiceError> {
    let mut total_sleep_ms = 0;
    let mut delay_ms = REGION_MISS_BACKOFF_BASE_MS;

    loop {
        if ctx.is_cancelled() {
            return Err(MetaServiceError::Cancelled);
        }

        match pd_client.get_all_members(ctx) {
            Ok(members) => {
                let mut addresses = Vec::new();
                for member in members {
                    for raw_url in member.client_urls {
                        if let Ok(endpoint) = astersql_util::service_url::ParseServiceURL(&raw_url)
                        {
                            addresses.push(endpoint.Endpoint(with_scheme));
                        }
                    }
                }
                if addresses.is_empty() {
                    return Err(MetaServiceError::NoUsablePdUrl);
                }
                return Ok(addresses);
            }
            Err(_) => {
                if ctx.is_cancelled() {
                    return Err(MetaServiceError::Cancelled);
                }
                // tikv.BoRegionMiss 使用 2ms 起始、500ms 上限、无抖动；预算只累计
                // 休眠时间，不把 PD 请求耗时算入 5000ms 上限。
                if total_sleep_ms >= GET_ALL_MEMBERS_BACKOFF_MS {
                    return Err(MetaServiceError::Pd("region unavailable".to_owned()));
                }
                if !ctx.wait_backoff(Duration::from_millis(delay_ms)) {
                    return Err(MetaServiceError::Cancelled);
                }
                total_sleep_ms += delay_ms;
                delay_ms = (delay_ms * 2).min(REGION_MISS_BACKOFF_CAP_MS);
            }
        }
    }
}

/// Parse a dialable service URL, retaining Unix socket addresses verbatim.
pub fn parse_url(raw_url: &str) -> Result<(String, String), MetaServiceError> {
    let endpoint = astersql_util::service_url::ParseServiceURL(raw_url)
        .map_err(|error| MetaServiceError::ServiceUrl(error.to_string()))?;
    Ok((endpoint.SchemePrefix(), endpoint.Address().to_owned()))
}

/// Go 兼容构造函数名。
#[allow(non_snake_case)]
pub fn NewEtcdMetaServiceClient(
    etcd_client: Option<Arc<dyn EtcdClient>>,
    pd_client: Option<Arc<dyn PdClient>>,
) -> Option<Client> {
    new_etcd_meta_service_client(etcd_client, pd_client)
}

/// Go 兼容 `newClient` 别名。
#[allow(non_snake_case)]
pub fn newClient(
    etcd_client: Option<Arc<dyn EtcdClient>>,
    pd_client: Option<Arc<dyn PdClient>>,
) -> Option<Client> {
    new_client(etcd_client, pd_client)
}

/// Go 兼容自由函数：直接对 PdClient 取地址。
#[allow(non_snake_case)]
pub fn GetPDAddrs(
    ctx: &Context,
    pd_client: &dyn PdClient,
    with_schema: bool,
) -> Result<Vec<String>, MetaServiceError> {
    get_pd_addrs(ctx, pd_client, with_schema)
}

/// Go 兼容自由函数：解析 URL。
#[allow(non_snake_case)]
pub fn ParseURL(raw_url: &str) -> Result<(String, String), MetaServiceError> {
    parse_url(raw_url)
}

#[cfg(test)]
#[path = "etcd_test.rs"]
mod etcd_test;
