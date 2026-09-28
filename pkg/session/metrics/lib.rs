// Copyright 2026 AsterSQL.

// `session/metrics` 子 crate：将会话相关 Prometheus 指标与遥测（Telemetry）拼装为可链接单元。
//
// 通过路径重导出公共 metrics / promutil / bindinfo / telemetry，并暴露
// [`session_metrics`] 中的会话级标签化句柄；单元测试验证与 Go 标签别名一致。

#![allow(non_snake_case, non_upper_case_globals, static_mut_refs)]

extern crate self as astersql_session_metrics;

#[path = "../../metrics/common/wrapper.rs"]
pub mod metricscommon;
#[path = "../../util/promutil/factory.rs"]
mod promutil_factory;
#[path = "../../util/promutil/registry.rs"]
mod promutil_registry;
/// Prometheus 工具：工厂与注册表的统一出口。
pub mod promutil {
    pub use crate::promutil_factory::*;
    pub use crate::promutil_registry::*;
}
#[path = "../../metrics/bindinfo.rs"]
pub mod bindinfo;
pub use bindinfo::*;
#[path = "../../metrics/telemetry.rs"]
pub mod telemetry;

/// 包级会话指标定义（含遥测），由 `metrics/session.rs` 内联生成。
pub mod metrics {
    pub(crate) static PACKAGE_INIT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    pub use crate::telemetry;
    include!("../../metrics/session.rs");
}

// telemetry.rs addresses the same source modules by their metrics-crate names.
pub use metrics as session;
pub use metrics::{LblDb, LblInternal, LblPhase, LblResourceGroup, LblSQLType, LblType};
#[path = "../../metrics/executor.rs"]
pub mod executor;

#[path = "metrics.rs"]
pub mod session_metrics;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
