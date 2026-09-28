// Copyright 2026 AsterSQL.

// executor 指标（metrics）子 crate 入口。
//
// 通过路径重导出 `pkg/metrics` 与 `promutil`，并挂载本目录的
// `executor_metrics`：把 Prometheus（监控指标库）向量按标签预绑定为
// 执行器侧可直接 observe/inc 的句柄。

#![allow(non_snake_case, non_upper_case_globals, static_mut_refs)]

extern crate self as astersql_executor_metrics;

/// 指标公共包装类型。
#[path = "../../../pkg/metrics/common/wrapper.rs"]
pub mod metricscommon;

#[path = "../../../pkg/util/promutil/factory.rs"]
mod promutil_factory;
#[path = "../../../pkg/util/promutil/registry.rs"]
mod promutil_registry;
/// Prometheus 工厂与注册表工具。
pub mod promutil {
    pub use crate::promutil_factory::*;
    pub use crate::promutil_registry::*;
}

/// bindinfo 相关指标定义。
#[path = "../../../pkg/metrics/bindinfo.rs"]
pub mod bindinfo;
/// 会话级指标定义。
#[path = "../../../pkg/metrics/session.rs"]
pub mod session;
pub use session::*;
/// 执行器级 Prometheus 向量定义。
#[path = "../../../pkg/metrics/executor.rs"]
pub mod metric_executor;
/// 服务端指标定义。
#[path = "../../../pkg/metrics/server.rs"]
pub mod server;

/// 聚合 server/session/executor 指标，并提供包初始化互斥锁。
pub mod metrics {
    pub use crate::metric_executor::*;
    pub use crate::server::*;
    pub use crate::session::*;
    /// 包级初始化互斥，避免并发重复注册。
    pub static PACKAGE_INIT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
}

/// 执行器侧预绑定指标变量与 phase 观察表。
#[path = "../../../pkg/executor/metrics/metrics.rs"]
pub mod executor_metrics;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
