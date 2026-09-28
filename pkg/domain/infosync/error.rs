// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// infosync 包错误类型定义。
//
// 覆盖 InfoSyncer 未初始化、PD（Placement Driver）HTTP 客户端缺失、
// Prometheus 地址未配置，以及领域服务/外部/JSON 编解码等错误场景。

#[derive(Debug, thiserror::Error)]
/// infosync 操作中可能出现的错误枚举。
pub enum Error {
    /// 全局 InfoSyncer 尚未初始化。
    #[error("infoSyncer is not initialized")]
    NotInitialized,
    /// PD HTTP 客户端为空，无法访问集群调度接口。
    #[error("pd http cli is nil")]
    PdHttpClientMissing,
    /// etcd 拓扑中未找到 Prometheus 地址。
    #[error("prometheus address is not set")]
    PrometheusAddressNotSet,
    /// 领域服务错误（错误码 8243），通常不可重试。
    #[error("[domain:8243]{0}")]
    DomainService(String),
    /// 外部依赖返回的通用错误消息。
    #[error("{0}")]
    External(String),
    /// JSON 序列化/反序列化失败。
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// infosync 模块统一的 Result 别名。
pub type Result<T> = std::result::Result<T, Error>;
