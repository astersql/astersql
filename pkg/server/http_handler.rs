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

// Status HTTP 路由清单与 handler 工具类型。
//
// `TikvHandlerTool` 从 `Server` 提取驱动名与 Domain 可用性，
// `routes()` 对齐 Go status 表面的路径分类，供 `http_status` 注册占位或真实 handler。

use crate::server::Server;

/// Status 路由按职责划分的 handler 类别。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandlerKind {
    /// `/status` 等基础状态。
    Status,
    /// 优化器相关：统计 dump、plan replayer、trace。
    Optimizer,
    /// 全局 settings。
    Settings,
    /// Schema / 表元数据。
    Schema,
    /// DDL 历史、owner、检查与 GC 状态。
    Ddl,
    /// DXF（分布式框架）调度与任务。
    Dxf,
    /// Ingest（导入写入）限流参数。
    Ingest,
    /// Region / 表范围 / scatter。
    Region,
    /// MVCC（多版本并发控制）键值与事务查询。
    Mvcc,
    /// 测试/调试接口。
    Test,
}

impl HandlerKind {
    /// 类别的人类可读名称，用于 503 错误文案。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Optimizer => "optimizer",
            Self::Settings => "settings",
            Self::Schema => "schema",
            Self::Ddl => "ddl",
            Self::Dxf => "distributed framework",
            Self::Ingest => "ingest",
            Self::Region => "region",
            Self::Mvcc => "mvcc",
            Self::Test => "test",
        }
    }
}

/// Dependencies shared by the TiKV status handlers. Keeping driver identity
/// and domain availability here makes illegal server construction explicit.
///
/// TiKV status handler 共用依赖：驱动名与 Domain 是否就绪。
#[derive(Clone, Debug)]
pub struct TikvHandlerTool {
    /// 键值存储驱动名称；空串视为非法。
    pub driver_name: String,
    /// Domain（服务域）是否已初始化。
    pub domain_available: bool,
}

impl TikvHandlerTool {
    /// 从运行中的 Server 抽取工具依赖。
    pub fn from_server(server: &Server) -> Self {
        Self {
            driver_name: server.driver().name().into(),
            domain_available: server.domain().is_some(),
        }
    }

    /// 校验驱动与 Domain；非法时返回错误信息。
    pub fn validate(&self) -> Result<(), String> {
        if self.driver_name.is_empty() {
            return Err("invalid key-value store driver".into());
        }
        if !self.domain_available {
            return Err("server domain is not initialized".into());
        }
        Ok(())
    }

    /// Route inventory mirrors the Go status surface. Concrete handler crates
    /// can replace each route independently without changing registration.
    ///
    /// 返回 (路径模式, 类别) 清单，对齐 Go status 路由表。
    pub fn routes(&self) -> Vec<(&'static str, HandlerKind)> {
        vec![
            ("/settings", HandlerKind::Settings),
            ("/schema", HandlerKind::Schema),
            ("/schema/{db}", HandlerKind::Schema),
            ("/schema/{db}/{table}", HandlerKind::Schema),
            ("/schema_storage", HandlerKind::Schema),
            ("/schema_storage/{db}", HandlerKind::Schema),
            ("/schema_storage/{db}/{table}", HandlerKind::Schema),
            (
                "/tables/{colID}/{colTp}/{colFlag}/{colLen}",
                HandlerKind::Schema,
            ),
            ("/ddl/history", HandlerKind::Ddl),
            ("/ddl/owner/resign", HandlerKind::Ddl),
            ("/ddl/check/{db}/{table}/{index}", HandlerKind::Ddl),
            ("/txn-gc-states", HandlerKind::Ddl),
            ("/dxf/schedule/status", HandlerKind::Dxf),
            ("/dxf/schedule", HandlerKind::Dxf),
            ("/dxf/schedule/tune", HandlerKind::Dxf),
            ("/dxf/task/active", HandlerKind::Dxf),
            ("/dxf/task/history", HandlerKind::Dxf),
            ("/dxf/schedule/max_concurrent_task", HandlerKind::Dxf),
            (
                "/dxf/import-into/history/job/{keyspace}/{job_id}",
                HandlerKind::Dxf,
            ),
            ("/dxf/task/{taskID}/max_runtime_slots", HandlerKind::Dxf),
            ("/ingest/max-batch-split-ranges", HandlerKind::Ingest),
            ("/ingest/max-split-ranges-per-sec", HandlerKind::Ingest),
            ("/ingest/max-ingest-inflight", HandlerKind::Ingest),
            ("/ingest/max-ingest-per-sec", HandlerKind::Ingest),
            ("/tables/{db}/{table}/regions", HandlerKind::Region),
            ("/tables/{db}/{table}/ranges", HandlerKind::Region),
            ("/tables/{db}/{table}/scatter", HandlerKind::Region),
            ("/tables/{db}/{table}/stop-scatter", HandlerKind::Region),
            ("/tables/{db}/{table}/disk-usage", HandlerKind::Region),
            ("/regions/meta", HandlerKind::Region),
            ("/regions/hot", HandlerKind::Region),
            ("/regions/{regionID}", HandlerKind::Region),
            ("/mvcc/key/{db}/{table}", HandlerKind::Mvcc),
            ("/mvcc/key/{db}/{table}/{handle}", HandlerKind::Mvcc),
            ("/mvcc/txn/{startTS}/{db}/{table}", HandlerKind::Mvcc),
            ("/mvcc/hex/{hexKey}", HandlerKind::Mvcc),
            ("/mvcc/index/{db}/{table}/{index}", HandlerKind::Mvcc),
            (
                "/mvcc/index/{db}/{table}/{index}/{handle}",
                HandlerKind::Mvcc,
            ),
            ("/test/{mod}/{op}", HandlerKind::Test),
            ("/test/delete/rowkey/{db}/{table}", HandlerKind::Test),
            (
                "/test/delete/indexkey/{db}/{table}/{index}",
                HandlerKind::Test,
            ),
            ("/test/ddl/hook", HandlerKind::Test),
            ("/test/ttl/trigger/{db}/{table}", HandlerKind::Test),
        ]
    }
}

/// Optimize trace dump handler 所需的地址与 Domain 状态。
#[derive(Clone, Debug)]
pub struct OptimizeTraceHandler {
    /// 对外广告地址。
    pub advertise_address: String,
    /// Status HTTP 端口。
    pub status_port: u16,
    /// Domain 是否可用。
    pub domain_available: bool,
}

/// Plan replayer dump handler 所需的地址与 Domain 状态。
#[derive(Clone, Debug)]
pub struct PlanReplayerHandler {
    /// 对外广告地址。
    pub advertise_address: String,
    /// Status HTTP 端口。
    pub status_port: u16,
    /// Domain 是否可用。
    pub domain_available: bool,
}

/// 从 Server 配置构造 OptimizeTraceHandler。
pub fn new_optimize_trace_handler(server: &Server) -> OptimizeTraceHandler {
    let config = server.config();
    OptimizeTraceHandler {
        advertise_address: config.host.clone(),
        status_port: config.status.port,
        domain_available: server.domain().is_some(),
    }
}

/// 从 Server 配置构造 PlanReplayerHandler。
pub fn new_plan_replayer_handler(server: &Server) -> PlanReplayerHandler {
    let config = server.config();
    PlanReplayerHandler {
        advertise_address: config.host.clone(),
        status_port: config.status.port,
        domain_available: server.domain().is_some(),
    }
}
