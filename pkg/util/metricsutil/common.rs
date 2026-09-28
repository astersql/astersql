// Copyright 2026 AsterSQL.
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

// 指标注册公共逻辑：常量标签、Keyspace 元数据与各子系统 InitMetricsVars。
//
// 对应 Go `pkg/util/metricsutil`。Keyspace 是多租户命名空间；PD（Placement Driver）
// 提供集群元数据。本模块在注册 Prometheus 指标前合并 keyspace 可观测性标签，
// 并按序初始化 domain/executor/session 等子包指标变量。

use astersql_config as config;
use astersql_config_kerneltype as kerneltype;
use astersql_domain_metrics::domain_metrics;
use astersql_executor_metrics::executor_metrics;
use astersql_infoschema_metrics as infoschema_metrics;
use astersql_metrics_common as metricscommon;
use astersql_planner_core_metrics::planner_core_metrics as plannercore;
use astersql_server_metrics as server_metrics;
use astersql_session_metrics::session_metrics;
use astersql_session_txninfo::txn_info;
use astersql_sessiontxn_isolation_metrics::isolation_metrics;
use astersql_statistics_handle_cache_metrics::cache_metrics as statscache_metrics;
use astersql_statistics_handle_metrics as statshandler_metrics;
use astersql_store::{
    NetworkPdKeyspaceClient, NetworkSecurity, PdKeyspaceError, PdKeyspaceErrorKind,
};
use astersql_store_copr_metrics::copr_metrics;
use astersql_store_mockstore_unistore_metrics as unimetrics;
use astersql_ttl_metrics::ttl_metrics;
use astersql_util_topsql_reporter_metrics::reporter_metrics as topsqlreporter_metrics;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, Once, OnceLock, RwLock};
use std::time::Duration;

/// 组件名，创建 PD 客户端时标识调用方。
const componentName: &str = "tidb-metrics-util";
/// 常量标签键：keyspace 数值 ID。
const keyspaceIDLabel: &str = "keyspace_id";
/// 连接 PD 的超时时间。
const pdTimeout: Duration = Duration::from_secs(10);
/// 查询 Keyspace 元数据的最大重试次数。
const defaultMaxRetries: usize = 30;
/// 重试基础间隔（实际间隔随 attempt 递增）。
const retryInterval: Duration = Duration::from_millis(500);

/// TLS 证书路径配置（CA/证书/私钥）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TlsConfig {
    pub ca_path: String,
    pub cert_path: String,
    pub key_path: String,
}

impl TlsConfig {
    /// 与 BR TLSConfig 一致：CA 路径非空才视为启用 TLS。
    pub fn IsEnabled(&self) -> bool {
        !self.ca_path.is_empty()
    }

    /// 转为 PD 客户端安全选项。
    pub fn ToPDSecurityOption(&self) -> SecurityOption {
        SecurityOption {
            ca_path: self.ca_path.clone(),
            cert_path: self.cert_path.clone(),
            key_path: self.key_path.clone(),
        }
    }
}

/// PD 客户端安全选项。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SecurityOption {
    pub ca_path: String,
    pub cert_path: String,
    pub key_path: String,
}

/// Keyspace 元数据（当前仅需数值 id）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyspaceMeta {
    pub id: u32,
}

/// PD 相关错误分类，用于决定是否可重试。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PdErrorKind {
    /// 集群尚未 bootstrap。
    NotBootstrapped,
    /// 指定 Keyspace 不存在。
    KeyspaceNotExist,
    /// 其他不可重试错误。
    Unexpected,
}

/// metricsutil 操作错误，携带 PD 错误种类与消息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetricsUtilError {
    pub kind: PdErrorKind,
    pub message: String,
}

impl MetricsUtilError {
    /// 构造 Unexpected 类错误。
    pub fn unexpected(message: impl Into<String>) -> Self {
        Self {
            kind: PdErrorKind::Unexpected,
            message: message.into(),
        }
    }

    /// NotBootstrapped / KeyspaceNotExist 可重试。
    fn retryable(&self) -> bool {
        matches!(
            self.kind,
            PdErrorKind::NotBootstrapped | PdErrorKind::KeyspaceNotExist
        )
    }
}

impl fmt::Display for MetricsUtilError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for MetricsUtilError {}

/// PD 客户端抽象：按名加载 Keyspace 并关闭连接。
pub trait PdClient: Send + Sync {
    fn LoadKeyspace(&self, keyspace_name: &str) -> Result<KeyspaceMeta, MetricsUtilError>;
    fn Close(&self);
}

/// 可注入的 PD 客户端工厂（测试可替换）。
pub trait PdClientFactory: Send + Sync {
    fn NewClient(
        &self,
        component: &str,
        addresses: &[String],
        security: &SecurityOption,
        timeout: Duration,
        init_metrics: bool,
    ) -> Result<Box<dyn PdClient>, MetricsUtilError>;
}

/// 全局 PD 客户端工厂槽位。
static PD_CLIENT_FACTORY: OnceLock<RwLock<Option<Arc<dyn PdClientFactory>>>> = OnceLock::new();
static REGISTER_UNISTORE_METRICS: Once = Once::new();

struct ProductionPdClientFactory;

struct ProductionPdClient {
    client: Mutex<Option<NetworkPdKeyspaceClient>>,
}

impl PdClient for ProductionPdClient {
    fn LoadKeyspace(&self, keyspace_name: &str) -> Result<KeyspaceMeta, MetricsUtilError> {
        let guard = self
            .client
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let client = guard
            .as_ref()
            .ok_or_else(|| MetricsUtilError::unexpected("PD client is closed"))?;
        client
            .load_keyspace(keyspace_name)
            .map(|id| KeyspaceMeta { id })
            .map_err(map_pd_keyspace_error)
    }

    fn Close(&self) {
        self.client
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
    }
}

impl PdClientFactory for ProductionPdClientFactory {
    fn NewClient(
        &self,
        component: &str,
        addresses: &[String],
        security: &SecurityOption,
        timeout: Duration,
        _init_metrics: bool,
    ) -> Result<Box<dyn PdClient>, MetricsUtilError> {
        let network_security = (!security.ca_path.is_empty()).then(|| NetworkSecurity {
            ca_path: security.ca_path.clone(),
            cert_path: security.cert_path.clone(),
            key_path: security.key_path.clone(),
        });
        let client = NetworkPdKeyspaceClient::connect(
            addresses,
            network_security.as_ref(),
            timeout,
            component,
        )
        .map_err(map_pd_keyspace_error)?;
        Ok(Box::new(ProductionPdClient {
            client: Mutex::new(Some(client)),
        }))
    }
}

fn map_pd_keyspace_error(error: PdKeyspaceError) -> MetricsUtilError {
    let kind = match error.kind {
        PdKeyspaceErrorKind::NotBootstrapped => PdErrorKind::NotBootstrapped,
        PdKeyspaceErrorKind::KeyspaceNotExist => PdErrorKind::KeyspaceNotExist,
        PdKeyspaceErrorKind::Unexpected => PdErrorKind::Unexpected,
    };
    MetricsUtilError {
        kind,
        message: error.message,
    }
}

/// 设置或清空测试用 PD 客户端工厂；清空后恢复生产 gRPC 工厂。
pub fn SetPdClientFactory(factory: Option<Arc<dyn PdClientFactory>>) {
    *PD_CLIENT_FACTORY
        .get_or_init(|| RwLock::new(None))
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = factory;
}

fn pdClientFactory() -> Arc<dyn PdClientFactory> {
    PD_CLIENT_FACTORY
        .get_or_init(|| RwLock::new(None))
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
        .unwrap_or_else(|| Arc::new(ProductionPdClientFactory))
}

/// 普通路径注册指标：NextGen 时先打上 keyspace_name 常量标签。
pub fn RegisterMetrics() -> Result<(), MetricsUtilError> {
    let cfg = config::get_global_config();
    if kerneltype::IsNextGen() {
        metricscommon::SetConstLabels(&["keyspace_name".to_string(), cfg.keyspace_name.clone()]);
    }
    registerMetrics()
}

/// BR（Backup & Restore）路径：按 Keyspace 名查 PD 元数据后注册指标。
pub fn RegisterMetricsForBR(
    pd_addrs: &[String],
    tls: &TlsConfig,
    keyspace_name: &str,
) -> Result<(), MetricsUtilError> {
    if keyspace_name.is_empty() {
        return registerMetrics();
    }
    if kerneltype::IsNextGen() {
        metricscommon::SetConstLabels(&["keyspace_name".to_string(), keyspace_name.to_string()]);
    }
    let security = if tls.IsEnabled() {
        tls.ToPDSecurityOption()
    } else {
        SecurityOption::default()
    };
    let factory = pdClientFactory();
    let client = factory.NewClient(componentName, pd_addrs, &security, pdTimeout, false)?;
    // 查到 Keyspace id 后写入常量标签再注册。
    let result = getKeyspaceMeta(client.as_ref(), keyspace_name).and_then(|meta| {
        setKeyspaceIDConstLabel(meta.id);
        registerMetrics()
    });
    client.Close();
    result
}

/// 初始化各 metrics 子 crate 的父级 collector（对齐 Go InitMetrics）。
fn initParentMetricsCollectors() {
    // Go calls metrics.InitMetrics() before the per-package InitMetricsVars
    // bindings. The Rust tree splits pkg/metrics into owner crates that each
    // keep a local #[path] copy of the parent collectors, so initialize those
    // local parents first rather than pulling the grpcio-backed metrics crate.
    unsafe {
        astersql_domain_metrics::stats::InitStatsMetrics();
    }
    astersql_store_copr_metrics::metrics::init_dist_sql_metrics();
    astersql_executor_metrics::server::InitServerMetrics();
    astersql_executor_metrics::session::InitSessionMetrics();
    astersql_executor_metrics::metric_executor::InitExecutorMetrics();
    unsafe {
        use astersql_sessiontxn_isolation_metrics::metrics as isolation_parent;
        use std::ptr;
        // Rust 2024 禁止对 `static mut` 创建共享引用；用 addr_of_mut 读 Option 状态。
        if (*ptr::addr_of_mut!(isolation_parent::RCCheckTSWriteConfilictCounter)).is_none() {
            isolation_parent::RCCheckTSWriteConfilictCounter = Some(
                prometheus::CounterVec::new(
                    prometheus::Opts::new(
                        "rc_check_ts_conflict_total",
                        "Counter of WriteConflict caused by RCCheckTS.",
                    ),
                    &["type"],
                )
                .expect("RCCheckTS conflict counter options are valid"),
            );
        }
    }
    astersql_statistics_handle_cache_metrics::metrics::init_parent_metrics();
    astersql_util_topsql_reporter_metrics::metrics::init_parent_metrics();
}

/// 父级 collector 初始化后，按固定顺序调用各包 InitMetricsVars。
fn initMetrics() -> Result<(), MetricsUtilError> {
    // Corresponds to Go metrics.InitMetrics + metrics.RegisterMetrics, then the
    // same InitMetricsVars sequence. Registration of the central collectors is
    // owned by each metrics subcrate; UniStore still registers explicitly.
    initParentMetricsCollectors();
    copr_metrics::InitMetricsVars();
    domain_metrics::InitMetricsVars();
    executor_metrics::InitMetricsVars();
    infoschema_metrics::InitMetricsVars();
    isolation_metrics::init_metrics_vars();
    plannercore::InitMetricsVars();
    server_metrics::InitMetricsVars();
    session_metrics::InitMetricsVars();
    statshandler_metrics::InitMetricsVars();
    statscache_metrics::InitMetricsVars();
    topsqlreporter_metrics::InitMetricsVars();
    ttl_metrics::InitMetricsVars();
    txn_info::InitMetricsVars();
    if config::get_global_config().store == config::StoreTypeUniStore.String() {
        REGISTER_UNISTORE_METRICS.call_once(unimetrics::RegisterMetrics);
    }
    Ok(())
}

/// 合并现有常量标签与 Keyspace 可观测性标签后初始化指标。
pub(crate) fn registerMetrics() -> Result<(), MetricsUtilError> {
    let mut labels = cloneConstLabels();
    labels.extend(
        config::get_global_config()
            .GetKeyspaceObservabilityMetricLabels()
            .clone(),
    );
    if !labels.is_empty() {
        setConstLabels(labels);
    }
    initMetrics()
}

/// 克隆当前全局常量标签映射。
pub fn cloneConstLabels() -> HashMap<String, String> {
    metricscommon::GetConstLabels()
}

/// 写入 `keyspace_id` 常量标签。
pub fn setKeyspaceIDConstLabel(keyspace_id: u32) {
    let mut labels = cloneConstLabels();
    labels.insert(keyspaceIDLabel.to_string(), keyspace_id.to_string());
    setConstLabels(labels);
}

/// 将标签 map 按键排序后扁平化为交替键值切片再 SetConstLabels。
pub fn setConstLabels(labels: HashMap<String, String>) {
    let mut entries = labels.into_iter().collect::<Vec<_>>();
    entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    let mut key_values = Vec::with_capacity(entries.len() * 2);
    for (key, value) in entries {
        key_values.push(key);
        key_values.push(value);
    }
    metricscommon::SetConstLabels(&key_values);
}

/// 带退避重试地从 PD 加载 Keyspace 元数据。
pub fn getKeyspaceMeta(
    pd_client: &dyn PdClient,
    keyspace_name: &str,
) -> Result<KeyspaceMeta, MetricsUtilError> {
    getKeyspaceMetaWithRetry(
        pd_client,
        keyspace_name,
        defaultMaxRetries,
        retryInterval,
        std::thread::sleep,
    )
}

pub(crate) fn getKeyspaceMetaWithRetry<F>(
    pd_client: &dyn PdClient,
    keyspace_name: &str,
    max_retries: usize,
    backoff: Duration,
    mut sleep: F,
) -> Result<KeyspaceMeta, MetricsUtilError>
where
    F: FnMut(Duration),
{
    let mut last_error = None;
    for attempt in 1..=max_retries {
        match pd_client.LoadKeyspace(keyspace_name) {
            Ok(meta) => return Ok(meta),
            Err(error) if error.retryable() => {
                last_error = Some(error);
                sleep(backoff * attempt as u32);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error
        .unwrap_or_else(|| MetricsUtilError::unexpected("PD keyspace lookup did not run")))
}
