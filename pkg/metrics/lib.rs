// Copyright 2026 AsterSQL.

// `astersql-metrics` crate 入口：汇总 TiDB 各子系统的 Prometheus 指标。
//
// 本文件声明公共标签常量、再导出兼容层与各子系统模块（DDL、DistSQL、Domain、
// Executor、GC 等）。子系统指标多在各自 `Init*Metrics` 中写入包级静态量；
// 测试配置下额外挂载单元测试与集成测试模块。

#![allow(non_snake_case, non_upper_case_globals, static_mut_refs)]

/// 将本 crate 以别名暴露给内部模块，便于机械迁移中的路径引用。
extern crate self as astersql_metrics;

/// 再导出 metrics-common 兼容 API。
pub mod metricscommon {
    pub use astersql_metrics_common::*;
}

/// 再导出 promutil（Prometheus 工具/工厂）API。
pub mod promutil {
    pub use astersql_util_promutil::*;
}

/// 再导出 Lightning 风格的通用 metric 集合。
pub mod metric {
    pub use astersql_lightning_metric::metric::*;
}

/// 绑定信息与 Prometheus 兼容封装。
mod bindinfo;
pub use bindinfo::*;

/// Session 相关指标入口。
mod session;
pub use session::*;

/// Prometheus 标签名：操作类型（action）。
pub const LblAction: &str = "action";
/// Prometheus 标签名：数据库名。
pub const LblDB: &str = "db";
/// Prometheus 标签名：类型（type）。
pub const LBL_TYPE: &str = "type";
/// Prometheus 标签名：结果（result）。
pub const LBL_RESULT: &str = "result";
/// 指标命名空间：tidb。
pub const TiDB: &str = "tidb";

/// BR（备份恢复）指标。
pub mod br;
/// gRPC channelz Prometheus 采集器。
mod channelz;
/// DDL 指标。
pub mod ddl;
/// DistSQL / Coprocessor 指标。
pub mod distsql;
/// Domain（schema/权限等）指标。
pub mod domain;
/// Executor 执行器指标。
pub mod executor;
/// 外部工作负载管理器指标。
pub mod external_workload;
/// GC Worker 指标。
pub mod gc_worker;
/// 全局排序 / 归并排序指标。
pub mod globalsort;
/// 数据导入指标注册入口。
pub mod import;
/// InfoSchema V2 缓存指标。
pub mod infoschema;
/// 日志备份指标。
pub mod log_backup;
/// 内存仲裁相关指标。
pub mod memory;
/// Meta 相关指标。
pub mod meta;
/// 包级初始化与注册中枢。
pub mod metrics;
/// Owner 选举相关指标。
pub mod owner;
/// RawKV 相关指标。
pub mod rawkv;
/// 资源组指标。
pub mod resource_group;
/// 资源管理器指标。
pub mod resourcemanager;
/// RU（Request Unit）v2 指标。
pub mod ru_v2;
/// Server 层指标。
pub mod server;
/// SLI（服务水平指标）相关。
pub mod sli;
/// 统计信息相关指标。
pub mod stats;
/// 语句摘要指标。
pub mod stmtsummary;
/// 遥测指标。
pub mod telemetry;
/// TiKV client-go 仪表盘兼容指标。
pub mod tikv_client_metrics;
/// TopSQL 指标。
pub mod topsql;
/// TTL（生存时间）任务指标。
pub mod ttl;

/// bindinfo 兼容层单元测试。
#[cfg(test)]
#[path = "bindinfo_1_aster_unit_test.rs"]
mod bindinfo_1_aster_unit_test;

/// metrics 包迁移相关单元测试。
#[cfg(test)]
#[path = "metrics_2_aster_unit_test.rs"]
mod metrics_2_aster_unit_test;

/// 测试入口 / 公共 fixture。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// metrics 对外行为测试。
#[cfg(test)]
#[path = "metrics_test.rs"]
mod metrics_test;

/// metrics 包内实现细节测试。
#[cfg(test)]
#[path = "metrics_internal_test.rs"]
mod metrics_internal_test;

/// gRPC channelz collector parity tests.
#[cfg(test)]
#[path = "channelz_test.rs"]
mod channelz_test;

/// External workload metric parity tests.
#[cfg(test)]
#[path = "external_workload_test.rs"]
mod external_workload_test;

/// GC worker metric parity tests.
#[cfg(test)]
#[path = "gc_worker_test.rs"]
mod gc_worker_test;

/// Telemetry cross-module metric ownership parity tests.
#[cfg(test)]
#[path = "telemetry_test.rs"]
mod telemetry_test;
