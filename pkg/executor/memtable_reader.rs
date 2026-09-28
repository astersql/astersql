// Copyright 2026 AsterSQL.
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

// 内存表（MemTable）读取器：集群信息类虚拟表的数据拉取。
//
// 对应 information_schema / 巡检相关的 `cluster_*` 表：通过 HTTP 与 PD API
// 拉取配置、节点信息、集群日志、热点 Region（数据分片）历史与 Region Peer 状态，
// 并按批返回行。Region 是 TiKV 中的键范围分片单位。

// 集群内存表（Information Schema 中的 cluster_* 等）读取执行器。
//
// 将 cluster_config / cluster_log / hot_regions_history / tikv_region_peers
// 等虚拟表的检索逻辑封装为可批返回的 Retriever；Region 是 TiKV 数据分片单位。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;
use std::thread;

/// 集群日志每次 `retrieve` 最多返回的行数。
/// cluster_log 每批最多返回的行数。
pub const clusterLogBatchSize: usize = 256;
/// 热点 Region 历史每次 `retrieve` 最多返回的行数。
/// 热点 Region 历史每批最多返回的行数。
pub const hotRegionsHistoryBatchSize: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 内存表路径错误：携带可读消息。
/// 内存表检索错误。
pub struct MemTableError(pub String);

impl fmt::Display for MemTableError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for MemTableError {}

/// 本模块 Result 别名。
/// 内存表操作的 Result 别名。
pub type MemTableResult<T = ()> = Result<T, MemTableError>;

#[derive(Clone, Debug, PartialEq)]
/// 虚拟表单元格：字符串、整数、浮点、时间戳或空。
/// 内存表单元格值。
pub enum datum {
    String(String),
    Int(i64),
    Unsigned(u64),
    Float(f64),
    Timestamp(String),
    Null,
}

/// 一行 datum。
/// 一行 datum。
pub type datumRow = Vec<datum>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 运行时统计描述（可注册到执行上下文）。
/// 运行时统计描述（可注册到会话）。
pub struct runtimeStats {
    pub description: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 无操作关闭器占位，满足部分 Retriever 的生命周期接口。
/// 无实际资源的关闭占位。
pub struct dummyCloser;

impl dummyCloser {
    /// 关闭时无副作用。
    pub fn close(&self) -> MemTableResult {
        Ok(())
    }

    /// 无运行时统计。
    /// cluster_log 暂无额外运行时统计。
    pub fn getRuntimeStats(&self) -> Option<runtimeStats> {
        None
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 集群节点描述：类型、地址与状态端口地址。
/// 集群节点：类型、访问地址与状态端口。
pub struct serverInfo {
    pub serverType: String,
    pub address: String,
    pub statusAddr: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 节点信息类别：负载、系统或硬件。
/// 拉取的服务器信息类别：负载/系统/硬件。
pub enum serverInfoType {
    LoadInfo,
    SystemInfo,
    HardwareInfo,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 从计划下推的集群表过滤：节点类型与实例地址。
/// cluster_config/info 等表的谓词提取：节点类型与实例过滤。
pub struct clusterTableExtractor {
    pub skipRequest: bool,
    pub nodeTypes: BTreeSet<String>,
    pub instances: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 集群日志表过滤：时间窗、级别、模式与节点范围。
/// cluster_log 谓词：时间窗、级别、模式与节点过滤。
pub struct clusterLogTableExtractor {
    pub skipRequest: bool,
    pub nodeTypes: BTreeSet<String>,
    pub instances: BTreeSet<String>,
    pub logLevels: BTreeSet<String>,
    pub patterns: Vec<String>,
    pub startTime: i64,
    pub endTime: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 热点 Region 历史表过滤条件。
/// 热点 Region 历史谓词：时间、Region/Store/Peer 与角色过滤。
pub struct hotRegionsHistoryTableExtractor {
    pub skipRequest: bool,
    pub startTime: i64,
    pub endTime: i64,
    pub regionIDs: Vec<u64>,
    pub storeIDs: Vec<u64>,
    pub peerIDs: Vec<u64>,
    pub isLearners: Vec<bool>,
    pub isLeaders: Vec<bool>,
    pub hotRegionTypes: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// TiKV Region Peer 表过滤：Store / Region ID。
/// tikv_region_peers 谓词：Store/Region ID 过滤。
pub struct tikvRegionPeersExtractor {
    pub skipRequest: bool,
    pub storeIDs: Vec<u64>,
    pub regionIDs: Vec<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 配置项：键、值与是否隐藏。
/// 单条配置项；hidden 表示对用户不可见。
pub struct configItem {
    pub key: String,
    pub value: String,
    pub hidden: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 远程日志搜索请求参数。
/// 远端日志搜索请求参数。
pub struct logSearchRequest {
    pub startTime: i64,
    pub endTime: i64,
    pub levels: Vec<String>,
    pub patterns: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 单条日志消息。
/// 单条日志：时间、级别与正文。
pub struct logMessage {
    pub time: i64,
    pub level: String,
    pub message: String,
}

/// 远程日志流：按批拉取消息。
/// 可分页拉取日志消息的流。
pub trait logStream: Send {
    /// 取下一批消息；流结束返回 `None`。
    fn next_messages(&mut self) -> MemTableResult<Option<Vec<logMessage>>>;
}

/// 可取消句柄：关闭日志检索时中止远程流。
/// 可取消的远程检索句柄。
pub trait cancellation: Send + Sync {
    /// 发出取消信号。
    fn cancel(&self);
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Region 热点到库表/索引的映射。
/// Region 到库表/索引的映射。
pub struct tableMapping {
    pub databaseName: String,
    pub tableName: String,
    pub tableID: i64,
    pub indexName: Option<String>,
    pub indexID: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Raft Peer：副本 ID、所在 Store 与是否 learner。
/// Region Peer：ID、Store 与是否 Learner。
pub struct peerInfo {
    pub id: i64,
    pub storeID: i64,
    pub isLearner: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 宕机 Peer 及其宕机时长（秒）。
/// 宕机 Peer 及其宕机秒数。
pub struct downPeerStat {
    pub peer: peerInfo,
    pub downSeconds: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Region 元数据：Peer 列表、pending/down 与 leader。
/// Region 拓扑：peers、pending、down 与 leader。
pub struct regionInfo {
    pub id: i64,
    pub peers: Vec<peerInfo>,
    pub pendingPeers: Vec<peerInfo>,
    pub downPeers: Vec<downPeerStat>,
    pub leader: Option<peerInfo>,
}

/// 运行时边界：权限、集群发现、HTTP/PD 拉取与告警注册。
/// 运行时边界：权限、HTTP、日志流、热点与 Region 元数据。
pub trait memTableRuntime: Send + Sync {
    fn executor_open(&self) -> MemTableResult;
    fn probe_transaction(&self) -> MemTableResult<bool>;
    fn activate_transaction(&self) -> MemTableResult;
    fn internal_http_schema(&self) -> String;
    fn has_config_privilege(&self) -> bool;
    fn has_process_privilege(&self) -> bool;
    fn cluster_servers(&self) -> MemTableResult<Vec<serverInfo>>;
    fn pd_servers(&self) -> MemTableResult<Vec<serverInfo>>;
    fn fetch_cluster_config(
        &self,
        server: &serverInfo,
        url: &str,
    ) -> MemTableResult<Vec<configItem>>;
    fn fetch_server_info(
        &self,
        server: &serverInfo,
        info_type: serverInfoType,
    ) -> MemTableResult<Vec<datumRow>>;
    fn new_log_cancellation(&self) -> Arc<dyn cancellation>;
    fn open_log_stream(
        &self,
        server: &serverInfo,
        remote: &str,
        request: &logSearchRequest,
        cancellation: Arc<dyn cancellation>,
    ) -> MemTableResult<Box<dyn logStream>>;
    fn fetch_hot_regions(
        &self,
        pd_server: &serverInfo,
        request: &HistoryHotRegionsRequest,
    ) -> MemTableResult<HistoryHotRegions>;
    fn ensure_tikv_storage(&self) -> MemTableResult;
    fn hot_region_table_mappings(
        &self,
        region: &HistoryHotRegion,
    ) -> MemTableResult<Vec<tableMapping>>;
    fn format_timestamp(&self, unix_millis: i64) -> MemTableResult<String>;
    fn all_regions(&self) -> MemTableResult<Vec<regionInfo>>;
    fn regions_by_store(&self, store_id: u64) -> MemTableResult<Vec<regionInfo>>;
    fn region_by_id(&self, region_id: u64) -> MemTableResult<Option<regionInfo>>;
    fn append_warning(&self, warning: MemTableError);
    fn register_runtime_stats(&self, stats: runtimeStats);
}

/// 按节点类型与实例地址过滤服务器列表。
/// 按节点类型与实例地址过滤服务器列表。
fn filter_servers(
    servers: Vec<serverInfo>,
    node_types: &BTreeSet<String>,
    instances: &BTreeSet<String>,
) -> Vec<serverInfo> {
    servers
        .into_iter()
        .filter(|server| {
            (node_types.is_empty() || node_types.contains(&server.serverType))
                && (instances.is_empty() || instances.contains(&server.address))
        })
        .collect()
}

#[derive(Clone, Debug)]
/// 巡检缓存中的表快照：行集或错误。
/// 巡检缓存中的表快照：行集或错误。
pub struct tableSnapshot {
    pub rows: datumRowSet,
    pub error: Option<MemTableError>,
}

/// 多行结果集。
/// 多行结果集。
pub type datumRowSet = Vec<datumRow>;

/// MemTable 物理算子：Open/Next/Close，可选巡检表缓存。
/// 内存表读取算子：委托 Retriever，可选巡检表缓存。
pub struct MemTableReaderExec {
    pub tableName: String,
    pub retriever: memTableRetriever,
    pub cacheRetrieved: bool,
    pub inspectionTableCache: Option<BTreeMap<String, tableSnapshot>>,
    pub runtime: Arc<dyn memTableRuntime>,
}

impl MemTableReaderExec {
    /// 判断表名是否可写入巡检共享缓存。
    /// 是否为可写入巡检缓存的 cluster_* 表。
    pub fn isInspectionCacheableTable(&self, tblName: &str) -> bool {
        matches!(
            tblName.to_ascii_lowercase().as_str(),
            "cluster_config"
                | "cluster_info"
                | "cluster_systeminfo"
                | "cluster_load"
                | "cluster_hardware"
        )
    }

    /// 打开执行器；若探测到事务则激活。
    // 事务（Transaction）激活确保后续读在正确会话上下文中
    /// 打开算子；若探测到事务则激活。
    pub fn Open(&mut self) -> MemTableResult {
        self.runtime.executor_open()?;
        if self.runtime.probe_transaction().unwrap_or(false) {
            self.runtime.activate_transaction()?;
        }
        Ok(())
    }

    /// 拉取下一批行；可缓存表首次命中后写入 inspection 缓存。
    /// 取下一批行；可缓存表首次填充 inspectionTableCache。
    pub fn Next(&mut self) -> MemTableResult<datumRowSet> {
        let table = self.tableName.to_ascii_lowercase();
        // 巡检可缓存表：首次读入缓存，后续 Next 返回空表示结束
        // 巡检场景：可缓存表首次读入 cache，后续 Next 返回空
        if self.inspectionTableCache.is_some() && self.isInspectionCacheableTable(&table) {
            if self.cacheRetrieved {
                return Ok(Vec::new());
            }
            self.cacheRetrieved = true;
            let cached = self
                .inspectionTableCache
                .as_ref()
                .and_then(|cache| cache.get(&table))
                .cloned();
            let snapshot = match cached {
                Some(snapshot) => snapshot,
                None => match self.retriever.read() {
                    Ok(rows) => tableSnapshot { rows, error: None },
                    Err(error) => tableSnapshot {
                        rows: Vec::new(),
                        error: Some(error),
                    },
                },
            };
            self.inspectionTableCache
                .as_mut()
                .expect("inspection cache checked above")
                .entry(table)
                .or_insert_with(|| snapshot.clone());
            return snapshot.error.map_or(Ok(snapshot.rows), Err);
        }
        self.retriever.read()
    }

    /// 注册运行时统计并关闭底层 Retriever。
    /// 注册运行时统计并关闭 Retriever。
    pub fn Close(&mut self) -> MemTableResult {
        if let Some(stats) = self.retriever.runtime_stats() {
            self.runtime.register_runtime_stats(stats);
        }
        self.retriever.shutdown()
    }
}

/// 具体虚拟表拉取器枚举。
/// 具体检索器枚举：配置/节点信息/日志/热点/Region peers。
pub enum memTableRetriever {
    ClusterConfig(clusterConfigRetriever),
    ClusterServerInfo(clusterServerInfoRetriever),
    ClusterLog(clusterLogRetriever),
    HotRegions(hotRegionsHistoryRetriver),
    RegionPeers(tikvRegionPeersRetriever),
}

impl memTableRetriever {
    /// 委托具体 Retriever 取行。
    /// 分派到对应 Retriever 的 retrieve。
    fn read(&mut self) -> MemTableResult<datumRowSet> {
        match self {
            Self::ClusterConfig(retriever) => retriever.retrieve(),
            Self::ClusterServerInfo(retriever) => retriever.retrieve(),
            Self::ClusterLog(retriever) => retriever.retrieve(),
            Self::HotRegions(retriever) => retriever.retrieve(),
            Self::RegionPeers(retriever) => retriever.retrieve(),
        }
    }

    /// 关闭资源（日志流需取消）。
    /// 关闭日志检索或 dummyCloser。
    fn shutdown(&mut self) -> MemTableResult {
        match self {
            Self::ClusterLog(retriever) => retriever.close(),
            Self::ClusterConfig(retriever) => retriever.dummyCloser.close(),
            Self::ClusterServerInfo(retriever) => retriever.dummyCloser.close(),
            Self::HotRegions(retriever) => retriever.dummyCloser.close(),
            Self::RegionPeers(retriever) => retriever.dummyCloser.close(),
        }
    }

    /// 取运行时统计（若有）。
    /// 取得可选运行时统计。
    fn runtime_stats(&self) -> Option<runtimeStats> {
        match self {
            Self::ClusterLog(retriever) => retriever.getRuntimeStats(),
            Self::ClusterConfig(retriever) => retriever.dummyCloser.getRuntimeStats(),
            Self::ClusterServerInfo(retriever) => retriever.dummyCloser.getRuntimeStats(),
            Self::HotRegions(retriever) => retriever.dummyCloser.getRuntimeStats(),
            Self::RegionPeers(retriever) => retriever.dummyCloser.getRuntimeStats(),
        }
    }
}

/// `cluster_config` 表：并发拉取各节点配置。
/// cluster_config 检索器（一次性）。
pub struct clusterConfigRetriever {
    pub dummyCloser: dummyCloser,
    pub retrieved: bool,
    pub extractor: clusterTableExtractor,
    pub runtime: Arc<dyn memTableRuntime>,
}

impl clusterConfigRetriever {
    /// 一次性拉取配置行；已取过或 skip 则返回空。
    /// 跳过或已检索则空；否则一次性拉取集群配置行。
    pub fn retrieve(&mut self) -> MemTableResult<datumRowSet> {
        if self.extractor.skipRequest || self.retrieved {
            return Ok(Vec::new());
        }
        self.retrieved = true;
        fetchClusterConfig(
            self.runtime.as_ref(),
            &self.extractor.nodeTypes,
            &self.extractor.instances,
        )
    }
}

/// 校验 CONFIG 权限后，按节点类型拼 HTTP URL 并并发拉取配置。
// 无状态地址的节点记告警并跳过
/// 并行向各节点 HTTP 拉取配置，过滤 hidden 并排序键名。
pub fn fetchClusterConfig(
    runtime: &dyn memTableRuntime,
    nodeTypes: &BTreeSet<String>,
    nodeAddrs: &BTreeSet<String>,
) -> MemTableResult<datumRowSet> {
    if !runtime.has_config_privilege() {
        return Err(MemTableError("CONFIG privilege required".into()));
    }
    let servers = filter_servers(runtime.cluster_servers()?, nodeTypes, nodeAddrs);
    let http_schema = runtime.internal_http_schema();
    // 按节点并发拉配置，完成后按原始 index 排序以稳定输出
    let mut results = thread::scope(|scope| {
        let mut handles = Vec::new();
        for (index, server) in servers.into_iter().enumerate() {
            if server.statusAddr.is_empty() {
                runtime.append_warning(MemTableError(format!(
                    "{} node {} does not contain status address",
                    server.serverType, server.address
                )));
                continue;
            }
            let http_schema = http_schema.clone();
            // 按节点类型拼配置 URL 并过滤 hidden 项
            handles.push(scope.spawn(move || {
                let url = match server.serverType.as_str() {
                    "pd" => format!("{http_schema}://{}/pd/api/v1/config", server.statusAddr),
                    "tikv" | "tidb" | "tiflash" | "ticdc" => {
                        format!("{http_schema}://{}/config", server.statusAddr)
                    }
                    "tiproxy" => format!(
                        "{http_schema}://{}/api/admin/config?format=json",
                        server.statusAddr
                    ),
                    "tso" => format!("{http_schema}://{}/tso/api/v1/config", server.statusAddr),
                    "scheduling" => {
                        format!(
                            "{http_schema}://{}/scheduling/api/v1/config",
                            server.statusAddr
                        )
                    }
                    unsupported => {
                        return (
                            index,
                            Err(MemTableError(format!(
                                "currently we do not support get config from node type: {}({})",
                                unsupported, server.address
                            ))),
                        );
                    }
                };
                let fetched = runtime.fetch_cluster_config(&server, &url).map(|items| {
                    let mut items = items
                        .into_iter()
                        .filter(|item| !item.hidden)
                        .collect::<Vec<_>>();
                    items.sort_by(|left, right| left.key.cmp(&right.key));
                    items
                        .into_iter()
                        .map(|item| {
                            vec![
                                datum::String(server.serverType.clone()),
                                datum::String(server.address.clone()),
                                datum::String(item.key),
                                datum::String(item.value),
                            ]
                        })
                        .collect::<datumRowSet>()
                });
                (index, fetched)
            }));
        }
        handles
            .into_iter()
            .map(|handle| {
                handle.join().unwrap_or_else(|_| {
                    (
                        usize::MAX,
                        Err(MemTableError("cluster config worker panicked".into())),
                    )
                })
            })
            .collect::<Vec<_>>()
    });
    results.sort_by_key(|(index, _)| *index);
    let mut rows = Vec::new();
    for (_, result) in results {
        match result {
            Ok(node_rows) => rows.extend(node_rows),
            Err(error) => runtime.append_warning(error),
        }
    }
    Ok(rows)
}

/// `cluster_load` / `cluster_systeminfo` / `cluster_hardware` 拉取器。
/// cluster_load/systeminfo/hardware 检索器。
pub struct clusterServerInfoRetriever {
    pub dummyCloser: dummyCloser,
    pub extractor: clusterTableExtractor,
    pub serverInfoType: serverInfoType,
    pub retrieved: bool,
    pub runtime: Arc<dyn memTableRuntime>,
}

impl clusterServerInfoRetriever {
    /// 按信息类型校验权限后并发拉取节点信息行。
    // Load/System 需 PROCESS；Hardware 需 CONFIG
    /// 按信息类型检查权限后并行拉取各节点信息。
    pub fn retrieve(&mut self) -> MemTableResult<datumRowSet> {
        match self.serverInfoType {
            serverInfoType::LoadInfo | serverInfoType::SystemInfo => {
                if !self.runtime.has_process_privilege() {
                    return Err(MemTableError("PROCESS privilege required".into()));
                }
            }
            serverInfoType::HardwareInfo => {
                if !self.runtime.has_config_privilege() {
                    return Err(MemTableError("CONFIG privilege required".into()));
                }
            }
        }
        if self.extractor.skipRequest || self.retrieved {
            return Ok(Vec::new());
        }
        self.retrieved = true;
        let servers = filter_servers(
            self.runtime.cluster_servers()?,
            &self.extractor.nodeTypes,
            &self.extractor.instances,
        );
        let runtime = self.runtime.as_ref();
        let info_type = self.serverInfoType;
        let mut results = thread::scope(|scope| {
            servers
                .into_iter()
                .enumerate()
                .map(|(index, server)| {
                    scope.spawn(move || (index, runtime.fetch_server_info(&server, info_type)))
                })
                .collect::<Vec<_>>()
                .into_iter()
                .filter_map(|handle| handle.join().ok())
                .collect::<Vec<_>>()
        });
        results.sort_by_key(|(index, _)| *index);
        Ok(results
            .into_iter()
            .filter_map(|(_, result)| result.ok())
            .flatten()
            .collect())
    }
}

/// 解析 failpoint 注入的服务器列表，格式 `type,addr,status;...`。
/// 解析 failpoint 注入的 `type,addr,status;` 服务器列表。
pub fn parseFailpointServerInfo(value: &str) -> MemTableResult<Vec<serverInfo>> {
    value
        .split(';')
        .filter(|server| !server.is_empty())
        .map(|server| {
            let parts = server.split(',').collect::<Vec<_>>();
            if parts.len() < 3 {
                return Err(MemTableError(format!(
                    "invalid failpoint server info: {server}"
                )));
            }
            Ok(serverInfo {
                serverType: parts[0].into(),
                address: parts[1].into(),
                statusAddr: parts[2].into(),
            })
        })
        .collect()
}

/// 单节点日志流及其当前缓冲消息。
/// 单个节点日志流及其当前缓冲消息。
pub struct logStreamResult {
    pub addr: String,
    pub typ: String,
    pub messages: Vec<logMessage>,
    pub stream: Box<dyn logStream>,
}

/// 按消息时间（及节点类型）归并的最小堆。
/// 按消息时间（再按类型）排序的最小堆，用于多流归并。
pub struct logResponseHeap(pub Vec<logStreamResult>);

impl logResponseHeap {
    /// 堆大小。
    /// 堆中元素个数。
    pub fn Len(&self) -> usize {
        self.0.len()
    }

    /// 比较堆元素优先级：更早时间优先，同时间比类型。
    /// 比较：更早时间优先，时间相同则类型字典序更小优先。
    pub fn Less(&self, i: usize, j: usize) -> bool {
        let left = &self.0[i];
        let right = &self.0[j];
        let left_time = left.messages.first().map(|message| message.time);
        let right_time = right.messages.first().map(|message| message.time);
        left_time < right_time || (left_time == right_time && left.typ < right.typ)
    }

    /// 交换堆元素。
    /// 交换堆中两元素。
    pub fn Swap(&mut self, i: usize, j: usize) {
        self.0.swap(i, j);
    }

    /// 上滤插入。
    /// 上滤插入日志流结果。
    pub fn Push(&mut self, value: logStreamResult) {
        self.0.push(value);
        let mut child = self.0.len() - 1;
        while child > 0 {
            let parent = (child - 1) / 2;
            if !self.Less(child, parent) {
                break;
            }
            self.Swap(child, parent);
            child = parent;
        }
    }

    /// 弹出堆顶并下滤恢复堆性质。
    /// 弹出时间最早的日志流结果并下滤。
    pub fn Pop(&mut self) -> Option<logStreamResult> {
        if self.0.is_empty() {
            return None;
        }
        let last = self.0.len() - 1;
        self.Swap(0, last);
        let result = self.0.pop();
        let mut parent = 0;
        loop {
            let left = parent * 2 + 1;
            if left >= self.0.len() {
                break;
            }
            let right = left + 1;
            let child = if right < self.0.len() && self.Less(right, left) {
                right
            } else {
                left
            };
            if !self.Less(child, parent) {
                break;
            }
            self.Swap(parent, child);
            parent = child;
        }
        result
    }
}

/// `cluster_log` 表：多节点日志流归并拉取。
/// cluster_log 检索器：多节点流 + 堆归并按批吐出。
pub struct clusterLogRetriever {
    pub isDrained: bool,
    pub retrieving: bool,
    pub heap: logResponseHeap,
    pub extractor: clusterLogTableExtractor,
    pub cancel: Option<Arc<dyn cancellation>>,
    pub runtime: Arc<dyn memTableRuntime>,
}

impl clusterLogRetriever {
    /// 校验 PROCESS 权限与过滤条件后打开各节点日志流。
    // 禁止无时间窗或无过滤的全量扫日志
    /// 校验权限与时间/过滤条件后启动多节点日志流。
    pub fn initialize(&mut self) -> MemTableResult<Vec<logStreamResult>> {
        if !self.runtime.has_process_privilege() {
            return Err(MemTableError("PROCESS privilege required".into()));
        }
        let servers = filter_servers(
            self.runtime.cluster_servers()?,
            &self.extractor.nodeTypes,
            &self.extractor.instances,
        );
        if self.extractor.startTime == 0 {
            return Err(MemTableError(
                "denied to scan logs, please specified the start time, such as `time > '2020-01-01 00:00:00'`".into(),
            ));
        }
        if self.extractor.endTime == 0 {
            return Err(MemTableError(
                "denied to scan logs, please specified the end time, such as `time < '2020-01-01 00:00:00'`".into(),
            ));
        }
        if self.extractor.patterns.is_empty()
            && self.extractor.logLevels.is_empty()
            && self.extractor.instances.is_empty()
            && self.extractor.nodeTypes.is_empty()
        {
            return Err(MemTableError(
                "denied to scan full logs (use `SELECT * FROM cluster_log WHERE message LIKE '%'` explicitly if intentionally)".into(),
            ));
        }
        let request = logSearchRequest {
            startTime: self.extractor.startTime,
            endTime: self.extractor.endTime,
            levels: self.extractor.logLevels.iter().cloned().collect(),
            patterns: self.extractor.patterns.clone(),
        };
        self.startRetrieving(servers, request)
    }

    /// 为每个有状态地址的节点打开远程日志流。
    // tidb/tiproxy 使用 statusAddr，其余使用业务 address
    /// 为各节点打开日志流；缺 statusAddr 则告警跳过。
    pub fn startRetrieving(
        &mut self,
        servers: Vec<serverInfo>,
        request: logSearchRequest,
    ) -> MemTableResult<Vec<logStreamResult>> {
        let cancellation = self.runtime.new_log_cancellation();
        self.cancel = Some(cancellation.clone());
        let runtime = self.runtime.as_ref();
        let results = thread::scope(|scope| {
            servers
                .into_iter()
                .filter_map(|server| {
                    if server.statusAddr.is_empty() {
                        runtime.append_warning(MemTableError(format!(
                            "{} node {} does not contain status address",
                            server.serverType, server.address
                        )));
                        return None;
                    }
                    let request = request.clone();
                    let cancellation = cancellation.clone();
                    Some(scope.spawn(move || {
                        let remote = if matches!(server.serverType.as_str(), "tidb" | "tiproxy") {
                            server.statusAddr.clone()
                        } else {
                            server.address.clone()
                        };
                        runtime
                            .open_log_stream(&server, &remote, &request, cancellation)
                            .map(|stream| logStreamResult {
                                addr: server.address,
                                typ: server.serverType,
                                messages: Vec::new(),
                                stream,
                            })
                    }))
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| {
                    handle.join().unwrap_or_else(|_| {
                        Err(MemTableError("cluster log worker panicked".into()))
                    })
                })
                .collect::<Vec<_>>()
        });
        let mut streams = Vec::new();
        for result in results {
            match result {
                Ok(stream) => streams.push(stream),
                Err(error) => self.runtime.append_warning(error),
            }
        }
        Ok(streams)
    }

    /// 首次初始化堆，此后按批弹出归并行直至堆空。
    /// 初始化多流堆后按批归并日志行。
    pub fn retrieve(&mut self) -> MemTableResult<datumRowSet> {
        if self.extractor.skipRequest || self.isDrained {
            return Ok(Vec::new());
        }
        if !self.retrieving {
            self.retrieving = true;
            let streams = match self.initialize() {
                Ok(streams) => streams,
                Err(error) => {
                    self.isDrained = true;
                    return Err(error);
                }
            };
            for mut stream in streams {
                match stream.stream.next_messages() {
                    Ok(Some(messages)) if !messages.is_empty() => {
                        stream.messages = messages;
                        self.heap.Push(stream);
                    }
                    Ok(_) => {}
                    Err(error) => self.runtime.append_warning(error),
                }
            }
        }
        let mut rows = Vec::new();
        // 多路归并：弹出最早消息，必要时从同一流再取一批压回堆
        // 多流归并：弹出最早消息，耗尽则再拉一批
        while self.heap.Len() > 0 && rows.len() < clusterLogBatchSize {
            let mut item = self.heap.Pop().expect("heap length checked");
            let head = item.messages.remove(0);
            rows.push(vec![
                datum::Timestamp(self.runtime.format_timestamp(head.time)?),
                datum::String(item.typ.clone()),
                datum::String(item.addr.clone()),
                datum::String(head.level.to_ascii_uppercase()),
                datum::String(head.message),
            ]);
            if item.messages.is_empty() {
                match item.stream.next_messages() {
                    Ok(Some(messages)) if !messages.is_empty() => {
                        item.messages = messages;
                        self.heap.Push(item);
                    }
                    Ok(_) => {}
                    Err(error) => self.runtime.append_warning(error),
                }
            } else {
                self.heap.Push(item);
            }
        }
        self.isDrained = self.heap.Len() == 0;
        Ok(rows)
    }

    /// 取消远程日志检索。
    /// 取消进行中的日志检索。
    pub fn close(&mut self) -> MemTableResult {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        Ok(())
    }

    /// 日志 Retriever 暂无统计。
    pub fn getRuntimeStats(&self) -> Option<runtimeStats> {
        None
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 向 PD 查询历史热点 Region 的请求。
/// PD 历史热点 Region 查询请求。
pub struct HistoryHotRegionsRequest {
    pub startTime: i64,
    pub endTime: i64,
    pub regionIDs: Vec<u64>,
    pub storeIDs: Vec<u64>,
    pub peerIDs: Vec<u64>,
    pub isLearners: Vec<bool>,
    pub isLeaders: Vec<bool>,
    pub hotRegionTypes: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// PD 返回的历史热点 Region 列表容器。
/// 历史热点 Region 列表包装。
pub struct HistoryHotRegions {
    pub historyHotRegion: Vec<HistoryHotRegion>,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 单条历史热点 Region 记录。
/// 单条历史热点：流量、速率与键范围等。
pub struct HistoryHotRegion {
    pub updateTime: i64,
    pub regionID: u64,
    pub storeID: u64,
    pub peerID: u64,
    pub isLearner: bool,
    pub isLeader: bool,
    pub hotRegionType: String,
    pub hotDegree: i64,
    pub flowBytes: f64,
    pub keyRate: f64,
    pub queryRate: f64,
    pub startKey: String,
    pub endKey: String,
}

#[derive(Clone, Debug)]
/// 单个 PD 查询结果：地址、消息或错误。
/// 单个 PD 返回的热点结果或错误。
pub struct hotRegionsResult {
    pub addr: String,
    pub messages: HistoryHotRegions,
    pub error: Option<MemTableError>,
}

#[derive(Clone, Debug, Default)]
/// 按 updateTime / hotDegree 归并的最小堆。
/// 按 updateTime/hotDegree 归并的最小堆。
pub struct hotRegionsResponseHeap(pub Vec<hotRegionsResult>);

impl hotRegionsResponseHeap {
    /// 堆大小。
    /// 堆中元素个数。
    pub fn Len(&self) -> usize {
        self.0.len()
    }

    /// 更早更新时间优先；同时间比热度。
    /// 比较：更早 updateTime 优先，相同则更小 hotDegree 优先。
    pub fn Less(&self, i: usize, j: usize) -> bool {
        let left = self.0[i].messages.historyHotRegion.first();
        let right = self.0[j].messages.historyHotRegion.first();
        match (left, right) {
            (Some(left), Some(right)) => {
                left.updateTime < right.updateTime
                    || (left.updateTime == right.updateTime && left.hotDegree < right.hotDegree)
            }
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// 交换堆元素。
    /// 交换堆中两元素。
    pub fn Swap(&mut self, i: usize, j: usize) {
        self.0.swap(i, j);
    }

    /// 上滤插入。
    /// 上滤插入热点结果。
    pub fn Push(&mut self, value: hotRegionsResult) {
        self.0.push(value);
        let mut child = self.0.len() - 1;
        while child > 0 {
            let parent = (child - 1) / 2;
            if !self.Less(child, parent) {
                break;
            }
            self.Swap(child, parent);
            child = parent;
        }
    }

    /// 弹出堆顶并下滤。
    /// 弹出最早更新的热点结果并下滤。
    pub fn Pop(&mut self) -> Option<hotRegionsResult> {
        if self.0.is_empty() {
            return None;
        }
        let last = self.0.len() - 1;
        self.Swap(0, last);
        let result = self.0.pop();
        let mut parent = 0;
        loop {
            let left = parent * 2 + 1;
            if left >= self.0.len() {
                break;
            }
            let right = left + 1;
            let child = if right < self.0.len() && self.Less(right, left) {
                right
            } else {
                left
            };
            if !self.Less(child, parent) {
                break;
            }
            self.Swap(parent, child);
            parent = child;
        }
        result
    }
}

/// `tikv_hot_regions_history` 类表的拉取器（拼写沿用 Go）。
/// 热点 Region 历史检索器（命名沿用 Go 侧拼写）。
pub struct hotRegionsHistoryRetriver {
    pub dummyCloser: dummyCloser,
    pub isDrained: bool,
    pub retrieving: bool,
    pub heap: hotRegionsResponseHeap,
    pub extractor: hotRegionsHistoryTableExtractor,
    pub runtime: Arc<dyn memTableRuntime>,
}

impl hotRegionsHistoryRetriver {
    /// 校验 PROCESS 与时间窗后向各 PD 发起热点查询。
    /// 校验权限与时间窗后向各 PD 拉取热点历史。
    pub fn initialize(&self) -> MemTableResult<Vec<hotRegionsResult>> {
        if !self.runtime.has_process_privilege() {
            return Err(MemTableError("PROCESS privilege required".into()));
        }
        let pd_servers = self.runtime.pd_servers()?;
        if self.extractor.startTime == 0 {
            return Err(MemTableError(
                "denied to scan hot regions, please specified the start time, such as `update_time > '2020-01-01 00:00:00'`".into(),
            ));
        }
        if self.extractor.endTime == 0 {
            return Err(MemTableError(
                "denied to scan hot regions, please specified the end time, such as `update_time < '2020-01-01 00:00:00'`".into(),
            ));
        }
        let request = HistoryHotRegionsRequest {
            startTime: self.extractor.startTime,
            endTime: self.extractor.endTime,
            regionIDs: self.extractor.regionIDs.clone(),
            storeIDs: self.extractor.storeIDs.clone(),
            peerIDs: self.extractor.peerIDs.clone(),
            isLearners: self.extractor.isLearners.clone(),
            isLeaders: self.extractor.isLeaders.clone(),
            hotRegionTypes: Vec::new(),
        };
        self.startRetrieving(pd_servers, request)
    }

    /// 对每个 PD × 热点类型组合并发请求。
    /// 按热点类型并行请求各 PD，汇总为 hotRegionsResult。
    pub fn startRetrieving(
        &self,
        pdServers: Vec<serverInfo>,
        request: HistoryHotRegionsRequest,
    ) -> MemTableResult<Vec<hotRegionsResult>> {
        let runtime = self.runtime.as_ref();
        Ok(thread::scope(|scope| {
            let mut handles = Vec::new();
            for server in pdServers {
                for hot_type in &self.extractor.hotRegionTypes {
                    let server = server.clone();
                    let mut request = request.clone();
                    request.hotRegionTypes = vec![hot_type.clone()];
                    handles.push(scope.spawn(move || {
                        match runtime.fetch_hot_regions(&server, &request) {
                            Ok(messages) => hotRegionsResult {
                                addr: server.statusAddr,
                                messages,
                                error: None,
                            },
                            Err(error) => hotRegionsResult {
                                addr: server.statusAddr,
                                messages: HistoryHotRegions::default(),
                                error: Some(error),
                            },
                        }
                    }));
                }
            }
            handles
                .into_iter()
                .map(|handle| {
                    handle.join().unwrap_or_else(|_| hotRegionsResult {
                        addr: String::new(),
                        messages: HistoryHotRegions::default(),
                        error: Some(MemTableError("hot regions worker panicked".into())),
                    })
                })
                .collect()
        }))
    }

    /// 初始化堆后按批弹出，并展开为带库表映射的行。
    /// 初始化热点堆后按批展开带 schema 的行。
    pub fn retrieve(&mut self) -> MemTableResult<datumRowSet> {
        if self.extractor.skipRequest || self.isDrained {
            return Ok(Vec::new());
        }
        if !self.retrieving {
            self.retrieving = true;
            let results = match self.initialize() {
                Ok(results) => results,
                Err(error) => {
                    self.isDrained = true;
                    return Err(error);
                }
            };
            for result in results {
                if let Some(error) = result.error.clone() {
                    self.runtime.append_warning(error);
                } else if !result.messages.historyHotRegion.is_empty() {
                    self.heap.Push(result);
                }
            }
        }
        self.runtime.ensure_tikv_storage()?;
        let mut rows = Vec::new();
        // 按批弹出热点记录并展开 schema 映射行
        // 按批弹出热点并展开为带 schema 的行
        while self.heap.Len() > 0 && rows.len() < hotRegionsHistoryBatchSize {
            let mut item = self.heap.Pop().expect("heap length checked");
            let region = item.messages.historyHotRegion.remove(0);
            rows.extend(self.getHotRegionRowWithSchemaInfo(&region)?);
            if !item.messages.historyHotRegion.is_empty() {
                self.heap.Push(item);
            }
        }
        self.isDrained = self.heap.Len() == 0;
        Ok(rows)
    }

    /// 将热点 Region 映射到库表/索引后组装输出行。
    /// 将热点 Region 展开为带库表/索引 schema 的多行。
    pub fn getHotRegionRowWithSchemaInfo(
        &self,
        hotRegion: &HistoryHotRegion,
    ) -> MemTableResult<datumRowSet> {
        let mappings = self.runtime.hot_region_table_mappings(hotRegion)?;
        let update_time = self.runtime.format_timestamp(hotRegion.updateTime)?;
        Ok(mappings
            .into_iter()
            .map(|mapping| {
                let (index_name, index_id) = match (mapping.indexName, mapping.indexID) {
                    (Some(name), Some(id)) => {
                        (datum::String(name.to_ascii_uppercase()), datum::Int(id))
                    }
                    _ => (datum::Null, datum::Null),
                };
                vec![
                    datum::Timestamp(update_time.clone()),
                    datum::String(mapping.databaseName.to_ascii_uppercase()),
                    datum::String(mapping.tableName.to_ascii_uppercase()),
                    datum::Int(mapping.tableID),
                    index_name,
                    index_id,
                    datum::Unsigned(hotRegion.regionID),
                    datum::Unsigned(hotRegion.storeID),
                    datum::Unsigned(hotRegion.peerID),
                    datum::Int(i64::from(hotRegion.isLearner)),
                    datum::Int(i64::from(hotRegion.isLeader)),
                    datum::String(hotRegion.hotRegionType.to_ascii_uppercase()),
                    datum::Int(hotRegion.hotDegree),
                    datum::Float(hotRegion.flowBytes),
                    datum::Float(hotRegion.keyRate),
                    datum::Float(hotRegion.queryRate),
                ]
            })
            .collect())
    }
}

/// `tikv_region_peers` 表：Region 副本状态行。
/// tikv_region_peers：按 Store/Region 过滤并展开 Peer 行。
pub struct tikvRegionPeersRetriever {
    pub dummyCloser: dummyCloser,
    pub extractor: tikvRegionPeersExtractor,
    pub retrieved: bool,
    pub runtime: Arc<dyn memTableRuntime>,
}

impl tikvRegionPeersRetriever {
    /// 按 Store/Region 过滤收集 Region，再打包 Peer 行。
    // 两者皆空则扫描全部 Region
    /// 按 Store/Region 过滤收集 Region，再 pack 为 peer 行。
    pub fn retrieve(&mut self) -> MemTableResult<datumRowSet> {
        if self.extractor.skipRequest || self.retrieved {
            return Ok(Vec::new());
        }
        self.retrieved = true;
        self.runtime.ensure_tikv_storage()?;
        let mut regions = Vec::new();
        let mut regions_by_store = Vec::new();
        let mut region_map = BTreeMap::<i64, regionInfo>::new();
        let store_map = self
            .extractor
            .storeIDs
            .iter()
            .map(|store_id| *store_id as i64)
            .collect::<BTreeSet<_>>();

        // 无过滤：全量 Region
        // 无过滤条件：返回全部 Region 的 peers
        if self.extractor.storeIDs.is_empty() && self.extractor.regionIDs.is_empty() {
            return self.packTiKVRegionPeersRows(self.runtime.all_regions()?, &store_map);
        }
        for store_id in &self.extractor.storeIDs {
            for region in self.runtime.regions_by_store(*store_id)? {
                if !region_map.contains_key(&region.id) {
                    regions_by_store.push(region.clone());
                    region_map.insert(region.id, region);
                }
            }
        }
        if self.extractor.regionIDs.is_empty() {
            return self.packTiKVRegionPeersRows(regions_by_store, &store_map);
        }
        for region_id in &self.extractor.regionIDs {
            if let Some(region) = region_map.get(&(*region_id as i64)) {
                regions.push(region.clone());
            } else if self.extractor.storeIDs.is_empty()
                && let Some(region) = self.runtime.region_by_id(*region_id)?
            {
                regions.push(region);
            }
        }
        self.packTiKVRegionPeersRows(regions, &store_map)
    }

    /// 若指定了 Store 过滤且当前 Store 不在集合中则跳过。
    /// 若指定了 Store 过滤且该 Store 不在集合中则跳过。
    pub fn isUnexpectedStoreID(&self, storeID: i64, storeMap: &BTreeSet<i64>) -> bool {
        !self.extractor.storeIDs.is_empty() && !storeMap.contains(&storeID)
    }

    /// 将 Region Peer 展开为行，标注 NORMAL/PENDING/DOWN 状态。
    /// 将 Region 拓扑展开为 peer 行，标注 DOWN/PENDING/NORMAL。
    pub fn packTiKVRegionPeersRows(
        &self,
        regionsInfo: Vec<regionInfo>,
        storeMap: &BTreeSet<i64>,
    ) -> MemTableResult<datumRowSet> {
        let mut rows = Vec::new();
        for region in regionsInfo {
            let pending = region
                .pendingPeers
                .iter()
                .map(|peer| peer.id)
                .collect::<BTreeSet<_>>();
            let down = region
                .downPeers
                .iter()
                .map(|stat| (stat.peer.id, stat.downSeconds))
                .collect::<BTreeMap<_, _>>();
            for peer in region.peers {
                if self.isUnexpectedStoreID(peer.storeID, storeMap) {
                    continue;
                }
                // DOWN > PENDING > NORMAL
                let (status, down_seconds) = if let Some(seconds) = down.get(&peer.id) {
                    (datum::String("DOWN".into()), datum::Int(*seconds))
                } else if pending.contains(&peer.id) {
                    (datum::String("PENDING".into()), datum::Null)
                } else {
                    (datum::String("NORMAL".into()), datum::Null)
                };
                rows.push(vec![
                    datum::Int(region.id),
                    datum::Int(peer.id),
                    datum::Int(peer.storeID),
                    datum::Int(i64::from(peer.isLearner)),
                    datum::Int(i64::from(
                        region
                            .leader
                            .as_ref()
                            .is_some_and(|leader| leader.id == peer.id),
                    )),
                    status,
                    down_seconds,
                ]);
            }
        }
        Ok(rows)
    }
}
