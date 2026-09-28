// Copyright 2026 AsterSQL.

//! 官方 TiKV 事务客户端的同步生命周期边界。
//!
//! `ClientRuntime` 在构造时建立唯一的 Tokio runtime 和 `TransactionClient`，
//! 后续同步调用复用这两个对象；关闭后会释放客户端、停止 runtime，并拒绝新请求。

use std::io;
use std::time::Duration;

use thiserror::Error;
use tikv_client::{
    Backoff, Config as TiKvConfig, ProtoLockInfo, Timestamp, TimestampExt, TransactionClient,
};
use tokio::runtime::{Builder, Runtime};

use crate::tikv_driver::TlsConfig;

/// Match client-go's effectively unlimited gRPC receive size. The upstream
/// Rust client defaults to 4 MiB, which is smaller than valid TiKV scan
/// responses and values.
const GRPC_MAX_DECODING_MESSAGE_SIZE: usize = usize::MAX;

/// TiKV API/keyspace 模式。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum KeyspaceConfig {
    /// API V1，不添加 keyspace 前缀。
    #[default]
    ApiV1,
    /// API V2，由 PD 按名称加载 keyspace 并由客户端编码前缀。
    ApiV2(String),
    /// API V2 服务端嵌入模式，不由客户端添加或删除前缀。
    ApiV2NoPrefix,
}

/// 建立 `ClientRuntime` 所需的生产配置。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientConfig {
    pd_endpoints: Vec<String>,
    timeout: Duration,
    tls: Option<TlsConfig>,
    keyspace: KeyspaceConfig,
}

impl ClientConfig {
    /// 使用 PD 地址构造配置。生产调用方不得传入 TiKV 节点地址。
    pub fn new<I, S>(pd_endpoints: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            pd_endpoints: pd_endpoints.into_iter().map(Into::into).collect(),
            timeout: TiKvConfig::default().timeout,
            tls: None,
            keyspace: KeyspaceConfig::ApiV1,
        }
    }

    /// 设置 PD/TiKV RPC 超时。
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// 设置 TLS 文件。
    #[must_use]
    pub fn with_tls(mut self, tls: TlsConfig) -> Self {
        self.tls = Some(tls);
        self
    }

    /// 设置 API/keyspace 模式。
    #[must_use]
    pub fn with_keyspace(mut self, keyspace: KeyspaceConfig) -> Self {
        self.keyspace = keyspace;
        self
    }

    fn validate(&self) -> Result<(), ClientRuntimeError> {
        if self.pd_endpoints.is_empty() {
            return Err(ClientRuntimeError::Configuration(
                "at least one PD endpoint is required".to_owned(),
            ));
        }
        if self
            .pd_endpoints
            .iter()
            .any(|endpoint| endpoint.trim().is_empty())
        {
            return Err(ClientRuntimeError::Configuration(
                "PD endpoints must not be empty".to_owned(),
            ));
        }
        if matches!(&self.keyspace, KeyspaceConfig::ApiV2(name) if name.is_empty()) {
            return Err(ClientRuntimeError::Configuration(
                "API V2 keyspace name must not be empty".to_owned(),
            ));
        }
        Ok(())
    }

    pub(crate) fn tikv_config(&self) -> TiKvConfig {
        let mut config = TiKvConfig::default()
            .with_timeout(self.timeout)
            .with_grpc_max_decoding_message_size(GRPC_MAX_DECODING_MESSAGE_SIZE);
        if let Some(tls) = &self.tls {
            config = config.with_security(
                tls.ca_path.clone(),
                tls.cert_path.clone(),
                tls.key_path.clone(),
            );
        }
        if let KeyspaceConfig::ApiV2(name) = &self.keyspace {
            config = config.with_keyspace(name);
        }
        config
    }
}

/// Client runtime 构造与命令执行错误。
#[derive(Debug, Error)]
pub enum ClientRuntimeError {
    #[error("invalid TiKV client configuration: {0}")]
    Configuration(String),
    #[error("failed to create Tokio runtime: {0}")]
    Runtime(#[from] io::Error),
    #[error("TiKV client error: {0}")]
    Client(#[from] tikv_client::Error),
    #[error("{0} TiKV transaction lock(s) remain live after coprocessor lock backoff")]
    LocksStillLive(usize),
    #[error("TiKV client runtime is closed")]
    Closed,
}

pub(crate) trait TimestampSource: Send + Sync {
    fn current_timestamp(&self, runtime: &Runtime) -> Result<u64, ClientRuntimeError>;
}

struct OfficialTimestampSource {
    client: TransactionClient,
}

impl TimestampSource for OfficialTimestampSource {
    fn current_timestamp(&self, runtime: &Runtime) -> Result<u64, ClientRuntimeError> {
        runtime
            .block_on(self.client.current_timestamp())
            .map(|timestamp| timestamp.version())
            .map_err(ClientRuntimeError::from)
    }
}

/// 持有单一 Tokio runtime 和官方 `TransactionClient` 的同步调用边界。
pub struct ClientRuntime {
    runtime: Option<Runtime>,
    client: Option<TransactionClient>,
    timestamp_source: Option<Box<dyn TimestampSource>>,
}

impl ClientRuntime {
    /// 连接 PD 并建立官方事务客户端。
    pub fn connect(config: ClientConfig) -> Result<Self, ClientRuntimeError> {
        config.validate()?;
        let runtime = Builder::new_multi_thread().enable_all().build()?;
        let tikv_config = config.tikv_config();
        let endpoints = config.pd_endpoints;
        let client = match config.keyspace {
            KeyspaceConfig::ApiV2NoPrefix => runtime.block_on(
                TransactionClient::new_with_config_api_v2_no_prefix(endpoints, tikv_config),
            )?,
            KeyspaceConfig::ApiV1 | KeyspaceConfig::ApiV2(_) => {
                runtime.block_on(TransactionClient::new_with_config(endpoints, tikv_config))?
            }
        };

        Ok(Self {
            runtime: Some(runtime),
            client: Some(client.clone()),
            timestamp_source: Some(Box::new(OfficialTimestampSource { client })),
        })
    }

    pub fn lock_waits(&self) -> Result<Vec<crate::WaitForEntry>, ClientRuntimeError> {
        let runtime = self.runtime.as_ref().ok_or(ClientRuntimeError::Closed)?;
        let client = self.client.as_ref().ok_or(ClientRuntimeError::Closed)?;
        Ok(runtime
            .block_on(client.get_lock_waits())?
            .into_iter()
            .map(|entry| crate::WaitForEntry {
                txn: entry.txn,
                waiting_for_txn: entry.wait_for_txn,
                key: entry.key,
                key_hash: entry.key_hash,
                resource_group_tag: entry.resource_group_tag,
                wait_time: entry.wait_time,
            })
            .collect())
    }

    /// 向 PD 请求当前时间戳，并转换为 TiDB 使用的 u64 TSO。
    pub fn current_timestamp(&self) -> Result<u64, ClientRuntimeError> {
        let runtime = self.runtime.as_ref().ok_or(ClientRuntimeError::Closed)?;
        let source = self
            .timestamp_source
            .as_ref()
            .ok_or(ClientRuntimeError::Closed)?;
        source.current_timestamp(runtime)
    }

    /// 对齐 Go `ResolveLocksWithOpts`：检查每个锁的事务状态，立即清理已提交/
    /// 已回滚锁，并在活锁上使用 CopNext 的约 20 秒快速退避预算。
    pub fn resolve_locks(
        &self,
        locks: &[astersql_store_copr::TransactionLock],
        caller_start_ts: u64,
    ) -> Result<(), ClientRuntimeError> {
        let runtime = self.runtime.as_ref().ok_or(ClientRuntimeError::Closed)?;
        let client = self.client.clone().ok_or(ClientRuntimeError::Closed)?;
        let locks = locks
            .iter()
            .map(|lock| ProtoLockInfo {
                primary_lock: lock.primary_lock.clone(),
                lock_version: lock.lock_version,
                key: lock.key.clone(),
                lock_ttl: lock.lock_ttl,
                txn_size: lock.txn_size,
                lock_type: lock.lock_type,
                lock_for_update_ts: lock.lock_for_update_ts,
                use_async_commit: lock.use_async_commit,
                min_commit_ts: lock.min_commit_ts,
                secondaries: lock.secondaries.clone(),
                duration_to_last_update_ms: lock.duration_to_last_update_ms,
                is_txn_file: lock.is_txn_file,
                shared_lock_infos: Vec::new(),
            })
            .collect();
        // 2+4+...+2048+3000*6 ~= 22 秒，和 Go CopNextMaxBackoff(20s)
        // 同量级；client-rust 每轮仍完整执行 CheckTxnStatus/async-commit/
        // ResolveLock，而不是仅等待 TTL。
        let live_locks = runtime.block_on(client.resolve_locks(
            locks,
            Timestamp::from_version(caller_start_ts),
            Backoff::no_jitter_backoff(2, 3_000, 17),
        ))?;
        if live_locks.is_empty() {
            Ok(())
        } else {
            Err(ClientRuntimeError::LocksStillLive(live_locks.len()))
        }
    }

    /// 释放客户端并停止 Tokio runtime。重复关闭是幂等的。
    pub fn close(&mut self) -> Result<(), ClientRuntimeError> {
        self.timestamp_source.take();
        self.client.take();
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::from_secs(1));
        }
        Ok(())
    }

    /// 返回共享 runtime；事务/快照适配器只借用，不创建请求级 runtime。
    pub(crate) fn runtime(&self) -> Result<&Runtime, ClientRuntimeError> {
        self.runtime.as_ref().ok_or(ClientRuntimeError::Closed)
    }

    /// 返回官方事务客户端克隆；克隆共享同一 PD/Region 客户端状态。
    pub(crate) fn transaction_client(&self) -> Result<TransactionClient, ClientRuntimeError> {
        self.client.clone().ok_or(ClientRuntimeError::Closed)
    }

    #[cfg(test)]
    pub(crate) fn from_timestamp_source_for_test(
        timestamp_source: Box<dyn TimestampSource>,
    ) -> Result<Self, ClientRuntimeError> {
        let runtime = Builder::new_multi_thread().enable_all().build()?;
        Ok(Self {
            runtime: Some(runtime),
            client: None,
            timestamp_source: Some(timestamp_source),
        })
    }
}

impl Drop for ClientRuntime {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
