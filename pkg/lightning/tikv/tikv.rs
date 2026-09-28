// Copyright 2019 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// Lightning 与 PD/TiKV 集群交互：连接、导入模式、压缩、指标与版本检查。
//
// PD（Placement Driver）负责调度与元数据；TiKV 为分布式 KV 存储。
// 导入模式（Import）会放宽 RocksDB 压缩限制以加速 Ingest SST。

use regex::Regex;
use semver::Version;
use std::sync::LazyLock;
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// TikV/PD 交互过程中的统一错误。
#[derive(Debug, Error)]
pub enum TikvError {
    /// 本地或远端 I/O 失败。
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// 调用参数不合法。
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// 数据/指标格式不符合预期。
    #[error("invalid data: {0}")]
    InvalidData(String),
    /// 远端未实现该 RPC，可被忽略。
    #[error("unimplemented operation: {0}")]
    Unimplemented(String),
    /// 远端返回的业务错误。
    #[error("remote error: {0}")]
    Remote(String),
    /// 组件版本不在允许区间。
    #[error("version error: {0}")]
    Version(String),
}

/// Store（TiKV 节点）在 PD 中的生命周期状态。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum StoreState {
    /// 在线可服务。
    Up = 0,
    /// 下线但仍可能被遍历。
    Offline = 1,
    /// 已墓碑，通常跳过。
    Tombstone = 2,
}

/// PD 返回的单个 Store 描述。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Store {
    /// Store ID。
    pub id: u64,
    /// 访问地址。
    pub address: String,
    /// TiKV 版本字符串（可带 `v` 前缀）。
    pub version: String,
    /// 当前状态。
    pub state: StoreState,
}

/// PD 客户端：列举 Store 与查询 PD 版本。
pub trait PdClient: Send + Sync {
    /// 获取集群全部 Store。
    fn GetStores(&self) -> Result<Vec<Store>, TikvError>;
    /// 获取 PD 版本字符串。
    fn GetPDVersion(&self) -> Result<String, TikvError>;
}

/// TiKV 运行模式：Normal 常规服务；Import 导入加速。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SwitchMode {
    /// 正常模式。
    Normal,
    /// 导入模式。
    Import,
}

/// 键空间半开区间 `[start, end)`，用于按范围切换模式。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KeyRange {
    /// 区间起点（含）。
    pub start: Vec<u8>,
    /// 区间终点（不含）。
    pub end: Vec<u8>,
}

/// 单节点 TiKV 客户端能力。
pub trait TiKvClient: Send {
    /// 切换导入/正常模式，可限定键范围。
    fn SwitchMode(&mut self, mode: SwitchMode, ranges: &[KeyRange]) -> Result<(), TikvError>;
    /// 触发指定 level 的手动压缩。
    fn Compact(&mut self, level: i32, resource_group: &str) -> Result<(), TikvError>;
    /// 拉取 Prometheus 文本指标。
    fn FetchMetrics(&mut self) -> Result<String, TikvError>;
}

/// 按地址建立 `TiKvClient` 连接。
pub trait TiKvConnector: Send + Sync {
    /// 连接到给定地址。
    fn Connect(&self, address: &str) -> Result<Box<dyn TiKvClient>, TikvError>;
}

/// 连接后执行一次性动作并返回结果。
pub fn withTiKVConnection<T>(
    connector: &dyn TiKvConnector,
    address: &str,
    action: impl FnOnce(&mut dyn TiKvClient) -> Result<T, TikvError>,
) -> Result<T, TikvError> {
    let mut client = connector.Connect(address)?;
    action(client.as_mut())
}

/// 对状态 ≤ `max_state` 的所有 Store 并行执行 `action`；首个错误胜出。
pub fn ForAllStores(
    pd: &dyn PdClient,
    max_state: StoreState,
    action: impl Fn(Store) -> Result<(), TikvError> + Sync,
) -> Result<(), TikvError> {
    let stores = pd
        .GetStores()?
        .into_iter()
        .filter(|store| store.state <= max_state)
        .collect::<Vec<_>>();
    let error = Arc::new(Mutex::new(None));
    // 使用 scoped 线程避免跨线程借用生命周期问题。
    std::thread::scope(|scope| {
        for store in stores {
            let error = Arc::clone(&error);
            let action = &action;
            scope.spawn(move || {
                if let Err(cause) = action(store) {
                    let mut first_error = error.lock().unwrap();
                    if first_error.is_none() {
                        *first_error = Some(cause);
                    }
                }
            });
        }
    });
    let result = error.lock().unwrap().take();
    result.map_or(Ok(()), Err)
}

/// 将 `Unimplemented` 视为成功，兼容旧版 TiKV 缺少 RPC 的情况。
pub fn ignoreUnimplementedError(result: Result<(), TikvError>) -> Result<(), TikvError> {
    match result {
        Err(TikvError::Unimplemented(_)) => Ok(()),
        other => other,
    }
}

/// 在指定 Store 上切换模式，忽略未实现错误。
pub fn SwitchModeOnStore(
    connector: &dyn TiKvConnector,
    address: &str,
    mode: SwitchMode,
    ranges: &[KeyRange],
) -> Result<(), TikvError> {
    withTiKVConnection(connector, address, |client| {
        ignoreUnimplementedError(client.SwitchMode(mode, ranges))
    })
}

/// `SwitchModeOnStore` 的简写别名。
pub fn SwitchMode(
    connector: &dyn TiKvConnector,
    address: &str,
    mode: SwitchMode,
    ranges: &[KeyRange],
) -> Result<(), TikvError> {
    SwitchModeOnStore(connector, address, mode, ranges)
}

/// 在指定 Store 上触发 Compact，忽略未实现错误。
pub fn Compact(
    connector: &dyn TiKvConnector,
    address: &str,
    level: i32,
    resource_group: &str,
) -> Result<(), TikvError> {
    withTiKVConnection(connector, address, |client| {
        ignoreUnimplementedError(client.Compact(level, resource_group))
    })
}

/// 通过指标推断当前是 Import 还是 Normal 模式。
pub fn FetchMode(connector: &dyn TiKvConnector, address: &str) -> Result<SwitchMode, TikvError> {
    withTiKVConnection(connector, address, |client| {
        FetchModeFromMetrics(&client.FetchMetrics()?)
    })
}

/// 匹配 RocksDB `hard_pending_compaction_bytes_limit` 配置指标行。
static FETCH_MODE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"\btikv_config_rocksdb\{cf="default",name="hard_pending_compaction_bytes_limit"\} ([^\n]+)"#,
    )
    .unwrap()
});

/// 从指标文本解析模式：指标原始值严格等于 `0` 表示 Import，否则 Normal。
pub fn FetchModeFromMetrics(metrics: &str) -> Result<SwitchMode, TikvError> {
    let value = FETCH_MODE_RE
        .captures(metrics)
        .and_then(|captures| captures.get(1))
        .ok_or_else(|| TikvError::InvalidData("import mode status is not exposed".into()))?
        .as_str();
    Ok(if value == "0" {
        SwitchMode::Import
    } else {
        SwitchMode::Normal
    })
}

/// 远端库名信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DBInfo {
    /// 数据库名。
    pub name: String,
}

/// 远端表名信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableInfo {
    /// 表名。
    pub name: String,
}

/// 通过 TLS 等通道拉取远端 schema 的抽象。
pub trait RemoteSchema: Send + Sync {
    /// 列举数据库。
    fn FetchDatabases(&self) -> Result<Vec<DBInfo>, TikvError>;
    /// 列举指定 schema 下的表。
    fn FetchTables(&self, schema: &str) -> Result<Vec<TableInfo>, TikvError>;
}

/// 拉取远端全部库模型。
pub fn FetchRemoteDBModelsFromTLS(remote: &dyn RemoteSchema) -> Result<Vec<DBInfo>, TikvError> {
    remote.FetchDatabases()
}

/// 拉取指定 schema 下的表模型。
pub fn FetchRemoteTableModelsFromTLS(
    remote: &dyn RemoteSchema,
    schema: &str,
) -> Result<Vec<TableInfo>, TikvError> {
    remote.FetchTables(schema)
}

/// 校验语义化版本是否落在 `[required_min, required_max)`。
fn check_version(
    component: &str,
    found: &Version,
    required_min: &Version,
    required_max: &Version,
) -> Result<(), TikvError> {
    if found < required_min {
        return Err(TikvError::Version(format!(
            "{component} version too old, required to be in [{required_min}, {required_max}), found '{found}'"
        )));
    }
    // Go 按 major 检查上界，避免上界 major 的 beta 版本因 semver 排序而漏过。
    if found.major >= required_max.major {
        return Err(TikvError::Version(format!(
            "{component} version too new, expected to be within [{required_min}, {}.0.0), found '{found}'",
            required_max.major
        )));
    }
    Ok(())
}

/// 检查 PD 版本是否在允许区间。
pub fn CheckPDVersion(
    pd: &dyn PdClient,
    required_min: &Version,
    required_max: &Version,
) -> Result<(), TikvError> {
    let version = pd.GetPDVersion()?;
    let found = Version::parse(version.strip_prefix('v').unwrap_or(&version))
        .map_err(|error| TikvError::Version(error.to_string()))?;
    check_version("PD", &found, required_min, required_max)
}

/// 对每个非 Tombstone Store 解析版本并回调。
pub fn ForTiKVVersions(
    pd: &dyn PdClient,
    action: impl Fn(Version, String) -> Result<(), TikvError> + Sync,
) -> Result<(), TikvError> {
    ForAllStores(pd, StoreState::Offline, move |store| {
        let component = format!("TiKV (at {})", store.address);
        let version = Version::parse(store.version.strip_prefix('v').unwrap_or(&store.version))
            .map_err(|error| TikvError::Version(format!("{component}: {error}")))?;
        action(version, component)
    })
}

/// 检查所有活跃 TiKV 版本是否在允许区间。
pub fn CheckTiKVVersion(
    pd: &dyn PdClient,
    required_min: &Version,
    required_max: &Version,
) -> Result<(), TikvError> {
    ForTiKVVersions(pd, |found, component| {
        check_version(&component, &found, required_min, required_max)
    })
}
