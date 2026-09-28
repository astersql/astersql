// Copyright 2023 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// TiFlash Compute 拓扑获取器（topology fetcher）。
//
// 从 AutoScaler（自动扩缩容服务）拉取 TiFlash Compute 节点拓扑；
// 拓扑即当前可用计算节点地址列表。支持 Mock / AWS / 测试实现，
// 并在 MPP 错误恢复（如内存超限）后重新获取拓扑。

#![allow(non_snake_case, non_upper_case_globals)]

use std::io::Read;
use std::sync::{Arc, RwLock};

use crate::config;
use serde::{Deserialize, Deserializer};

/// 全局拓扑获取器单例；初始化后供会话/执行层查询当前 CN（Compute Node）列表。
static globalTopoFetcher: RwLock<Option<Arc<dyn TopoFetcher>>> = RwLock::new(None);

/// AWS 固定池拓扑 HTTP 路径。
const awsFixedPoolHTTPPath: &str = "sharedfixedpool";
/// AWS 恢复并获取拓扑的 HTTP 路径。
const awsFetchHTTPPath: &str = "resume-and-get-topology";
/// HTTP GET 失败时的统一错误前缀。
const httpGetFailedErrMsg: &str = "get tiflash_compute topology failed";
/// 解析拓扑时间戳失败时的错误信息。
const parseTopoTSFailedErrMsg: &str = "parse timestamp of tiflash_compute topology failed";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("{0}")]
/// 拓扑获取过程中的错误，包装可读说明字符串。
pub struct TopoFetcherError(String);

impl TopoFetcherError {
    /// 由任意可转为 String 的消息构造错误。
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// MPP 错误恢复类型（MPPErrRecovery）：标识是否以及如何触发 AutoScaler 扩容/恢复。
/// RecoveryType is for MPPErrRecovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryType(pub u32);

impl RecoveryType {
    /// 无需恢复错误。
    /// RecoveryTypeNull means no need to recover an error.
    pub const RecoveryTypeNull: Self = Self(0);
    /// 内存超限错误，需要 AutoScaler 侧恢复（通常扩容 CN）。
    /// RecoveryTypeMemLimit means a memory-limit error needs recovery.
    pub const RecoveryTypeMemLimit: Self = Self(1);

    /// 将恢复类型转为 AutoScaler 可识别的字符串名。
    pub fn toString(&self) -> Result<String, TopoFetcherError> {
        match *self {
            Self::RecoveryTypeNull => Ok("Null".to_string()),
            Self::RecoveryTypeMemLimit => Ok("MemLimit".to_string()),
            _ => Err(TopoFetcherError::new(
                "unsupported recovery type for topo_fetcher",
            )),
        }
    }
}

/// 从 AutoScaler 拉取拓扑的抽象接口。
/// TopoFetcher fetches topology from an AutoScaler.
pub trait TopoFetcher: Send + Sync {
    /// 始终向 AutoScaler 拉取最新拓扑并返回；允许空列表。
    /// Always fetch topology from AutoScaler and return it. Empty topology is allowed.
    fn FetchAndGetTopo(&self) -> Result<Vec<String>, TopoFetcherError>;

    /// 先按恢复类型尝试恢复，再拉取新拓扑。
    /// Try recovery and then fetch new topology.
    fn RecoveryAndGetTopo(
        &self,
        recovery: RecoveryType,
        oriCNCnt: i32,
    ) -> Result<Vec<String>, TopoFetcherError>;
}

/// 按 AutoScaler 类型初始化全局获取器，逻辑对齐 Go 的 type switch。
/// InitGlobalTopoFetcher initializes the global fetcher. It mirrors the Go type switch.
pub fn InitGlobalTopoFetcher(
    typ: String,
    addr: String,
    clusterID: String,
    isFixedPool: bool,
) -> Result<(), TopoFetcherError> {
    // 校验集群 ID 与 AutoScaler 地址；按类型构造具体 Fetcher 并写入全局。
    log::info!(
        "init globalTopoFetcher: type={typ} addr={addr} clusterID={clusterID} isFixedPool={isFixedPool}"
    );
    if clusterID.is_empty() || addr.is_empty() {
        return Err(TopoFetcherError::new(format!(
            "ClusterID({clusterID}) or AutoScaler({addr}) addr is empty"
        )));
    }

    let fetcher: Arc<dyn TopoFetcher> = match config::GetAutoScalerType(&typ) {
        config::MockASType => Arc::new(NewMockAutoScalerFetcher(addr)),
        config::AWSASType => Arc::new(NewAWSAutoScalerFetcher(addr, clusterID, isFixedPool)),
        config::GCPASType => {
            return Err(TopoFetcherError::new(format!(
                "topo fetch not implemented yet({typ})"
            )));
        }
        config::TestASType => Arc::new(NewTestAutoScalerFetcher()),
        _ => {
            *globalTopoFetcher
                .write()
                .expect("global topology fetcher lock poisoned") = None;
            return Err(TopoFetcherError::new(format!(
                "unexpected topo fetch type. expect: {} or {} or {}, got {}",
                config::MockASStr,
                config::AWSASStr,
                config::GCPASStr,
                typ
            )));
        }
    };

    *globalTopoFetcher
        .write()
        .expect("global topology fetcher lock poisoned") = Some(fetcher);
    Ok(())
}

/// 返回当前全局拓扑获取器；未初始化则为 None。
/// GetGlobalTopoFetcher returns the current global fetcher.
pub fn GetGlobalTopoFetcher() -> Option<Arc<dyn TopoFetcher>> {
    globalTopoFetcher
        .read()
        .expect("global topology fetcher lock poisoned")
        .clone()
}

/// Mock AutoScaler 的拓扑获取器，用于本地/测试环境。
/// MockTopoFetcher fetches topology from MockAutoScaler.
pub struct MockTopoFetcher {
    topo: RwLock<Vec<String>>,
    addr: String,
}

/// 构造指向给定地址的 Mock 拓扑获取器。
pub fn NewMockAutoScalerFetcher(addr: String) -> MockTopoFetcher {
    MockTopoFetcher {
        topo: RwLock::new(Vec::with_capacity(8)),
        addr,
    }
}

impl TopoFetcher for MockTopoFetcher {
    fn FetchAndGetTopo(&self) -> Result<Vec<String>, TopoFetcherError> {
        self.fetchTopo()?;
        let topo = self.getTopo();
        log::debug!("FetchAndGetTopo: topo={topo:?}");
        Ok(topo)
    }

    fn RecoveryAndGetTopo(
        &self,
        _recovery: RecoveryType,
        _oriCNCnt: i32,
    ) -> Result<Vec<String>, TopoFetcherError> {
        Err(TopoFetcherError::new("RecoveryAndGetTopo not implemented"))
    }
}

impl MockTopoFetcher {
    /// 读取缓存中的拓扑副本。
    fn getTopo(&self) -> Vec<String> {
        self.topo
            .read()
            .expect("mock topology lock poisoned")
            .clone()
    }

    /// 通过 HTTP 从 Mock AutoScaler 拉取拓扑并更新缓存。
    fn fetchTopo(&self) -> Result<(), TopoFetcherError> {
        let url = format!("http://{}/fetch_topo", self.addr);
        log::info!("fetchTopo: url={url}");
        let newTopo = mockHTTPGetAndParseResp(&url)?;
        *self.topo.write().expect("mock topology lock poisoned") = newTopo;
        Ok(())
    }
}

/// 对 URL 发起 HTTP GET，返回原始响应体；失败时包装统一错误前缀。
fn httpGetAndParseResp(url: &str) -> Result<Vec<u8>, TopoFetcherError> {
    let response = ureq::get(url)
        .call()
        .map_err(|error| TopoFetcherError::new(format!("{httpGetFailedErrMsg}: {error}")))?;
    let mut body = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut body)
        .map_err(|error| TopoFetcherError::new(format!("{httpGetFailedErrMsg}: {error}")))?;
    Ok(body)
}

/// Mock 响应解析：UTF-8 文本，以分号分隔节点地址。
fn mockHTTPGetAndParseResp(url: &str) -> Result<Vec<String>, TopoFetcherError> {
    let body = httpGetAndParseResp(url)?;
    let body = String::from_utf8_lossy(&body);
    if body.is_empty() {
        return Err(TopoFetcherError::new("topo list is empty"));
    }
    Ok(body.split(';').map(str::to_owned).collect())
}

/// AWS AutoScaler 拓扑获取器：拉取并按时间戳缓存拓扑。
/// AWSTopoFetcher fetches and caches topology from AWSAutoScaler.
pub struct AWSTopoFetcher {
    state: RwLock<AWSTopoFetcherState>,
    addr: String,
    clusterID: String,
    isFixedPool: bool,
}

/// AWS 获取器内部可变状态：节点列表与拓扑时间戳（用于避免旧数据覆盖新数据）。
struct AWSTopoFetcherState {
    topo: Vec<String>,
    topoTS: i64,
}

#[derive(Default, Debug, Clone, Deserialize)]
#[serde(default)]
#[allow(non_snake_case)]
/// AWS `resume-and-get-topology` JSON 响应体字段映射。
struct resumeAndGetTopologyResult {
    #[serde(
        rename = "hasError",
        default,
        deserialize_with = "deserializeNullAsDefault"
    )]
    HasError: i32,
    #[serde(
        rename = "errorInfo",
        default,
        deserialize_with = "deserializeNullAsDefault"
    )]
    ErrorInfo: String,
    #[serde(
        rename = "state",
        default,
        deserialize_with = "deserializeNullAsDefault"
    )]
    State: String,
    #[serde(
        rename = "topology",
        default,
        deserialize_with = "deserializeNullAsDefault"
    )]
    Topology: Vec<String>,
    #[serde(
        rename = "timestamp",
        default,
        deserialize_with = "deserializeNullAsDefault"
    )]
    Timestamp: String,
}

/// 对齐 Go `encoding/json`：对象字段为 `null` 时保留目标类型零值。
fn deserializeNullAsDefault<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// 构造 AWS 拓扑获取器；`isFixed` 表示使用固定池路径而非动态恢复路径。
pub fn NewAWSAutoScalerFetcher(addr: String, clusterID: String, isFixed: bool) -> AWSTopoFetcher {
    AWSTopoFetcher {
        state: RwLock::new(AWSTopoFetcherState {
            topo: Vec::with_capacity(8),
            topoTS: -1,
        }),
        addr,
        clusterID,
        isFixedPool: isFixed,
    }
}

impl TopoFetcher for AWSTopoFetcher {
    fn FetchAndGetTopo(&self) -> Result<Vec<String>, TopoFetcherError> {
        self.fetchAndGetTopo(RecoveryType::RecoveryTypeNull, 0)
    }

    fn RecoveryAndGetTopo(
        &self,
        recovery: RecoveryType,
        oriCNCnt: i32,
    ) -> Result<Vec<String>, TopoFetcherError> {
        self.fetchAndGetTopo(recovery, oriCNCnt)
    }
}

impl AWSTopoFetcher {
    /// 统一拉取入口：校验恢复参数，固定池可命中缓存，否则走 HTTP 拉取。
    fn fetchAndGetTopo(
        &self,
        recovery: RecoveryType,
        oriCNCnt: i32,
    ) -> Result<Vec<String>, TopoFetcherError> {
        // 仅支持 Null / MemLimit；MemLimit 时原始 CN 数不能为 0。
        if recovery != RecoveryType::RecoveryTypeNull
            && recovery != RecoveryType::RecoveryTypeMemLimit
        {
            return Err(TopoFetcherError::new(format!(
                "topo_fetcher cannot handle error: {}",
                recovery.0
            )));
        }
        if recovery == RecoveryType::RecoveryTypeMemLimit && oriCNCnt == 0 {
            return Err(TopoFetcherError::new("ori CN count should not be zero"));
        }

        // 固定池：非空缓存直接返回；动态池：带恢复参数拉取。
        if self.isFixedPool {
            let (cachedTopo, _) = self.getTopo();
            if !cachedTopo.is_empty() {
                return Ok(cachedTopo);
            }
            self.fetchFixedPoolTopo()?;
        } else {
            self.fetchTopo(recovery, oriCNCnt)?;
        }
        let topo = self.getTopo().0;
        log::info!("AWSTopoFetcher FetchAndGetTopo done: curTopo={topo:?}");
        Ok(topo)
    }

    /// 仅当新拓扑时间戳更新时才写入缓存，防止并发下旧响应覆盖新状态。
    fn tryUpdateTopo(
        &self,
        newTopo: &resumeAndGetTopologyResult,
    ) -> Result<bool, TopoFetcherError> {
        let newTS = newTopo
            .Timestamp
            .parse::<i64>()
            .map_err(|_| TopoFetcherError::new(parseTopoTSFailedErrMsg))?;

        let cachedTS = self
            .state
            .read()
            .expect("AWS topology lock poisoned")
            .topoTS;
        if cachedTS >= newTS {
            return Ok(false);
        }

        let mut state = self.state.write().expect("AWS topology lock poisoned");
        if state.topoTS > newTS {
            return Ok(false);
        }
        state.topo = newTopo.Topology.clone();
        state.topoTS = newTS;
        Ok(true)
    }

    /// 从固定池 HTTP 路径拉取拓扑。
    fn fetchFixedPoolTopo(&self) -> Result<(), TopoFetcherError> {
        let url = format!("http://{}/{}", self.addr, awsFixedPoolHTTPPath);
        log::info!("fetchFixedPoolTopo: url={url}");
        let newTopo = awsHTTPGetAndParseResp(&url)?;
        self.tryUpdateTopo(&newTopo)?;
        Ok(())
    }

    /// 动态拉取：构造 query（集群 ID；MemLimit 时附加 recovery/cn_cnt）。
    fn fetchTopo(&self, recovery: RecoveryType, oriCNCnt: i32) -> Result<(), TopoFetcherError> {
        let mut url = url::Url::parse(&format!("http://{}/{}", self.addr, awsFetchHTTPPath))
            .map_err(|_| TopoFetcherError::new(httpGetFailedErrMsg))?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("tidbclusterid", &self.clusterID);
            if recovery == RecoveryType::RecoveryTypeMemLimit {
                query.append_pair("recovery", &recovery.toString()?);
                query.append_pair("cn_cnt", &oriCNCnt.to_string());
            }
        }
        log::info!("fetchTopo: url={url}");
        let newTopo = awsHTTPGetAndParseResp(url.as_str())?;
        self.tryUpdateTopo(&newTopo)?;
        Ok(())
    }

    /// 返回缓存拓扑及其时间戳。
    fn getTopo(&self) -> (Vec<String>, i64) {
        let state = self.state.read().expect("AWS topology lock poisoned");
        (state.topo.clone(), state.topoTS)
    }
}

/// AWS 响应解析：将 JSON 反序列化为 `resumeAndGetTopologyResult`。
fn awsHTTPGetAndParseResp(url: &str) -> Result<resumeAndGetTopologyResult, TopoFetcherError> {
    let body = httpGetAndParseResp(url)?;
    serde_json::from_slice(&body).map_err(|_| TopoFetcherError::new(httpGetFailedErrMsg))
}

/// 单元测试用获取器：始终返回空拓扑。
/// TestTopoFetcher returns an empty topology list for unit tests.
pub struct TestTopoFetcher {}

/// 构造测试用空拓扑获取器。
pub fn NewTestAutoScalerFetcher() -> TestTopoFetcher {
    TestTopoFetcher {}
}

impl TopoFetcher for TestTopoFetcher {
    fn FetchAndGetTopo(&self) -> Result<Vec<String>, TopoFetcherError> {
        Ok(Vec::new())
    }

    fn RecoveryAndGetTopo(
        &self,
        _recovery: RecoveryType,
        _oriCNCnt: i32,
    ) -> Result<Vec<String>, TopoFetcherError> {
        Err(TopoFetcherError::new("RecoveryAndGetTopo not implemented"))
    }
}
