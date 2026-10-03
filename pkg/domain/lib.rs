// Copyright 2026 AsterSQL.

// `domain` 包入口：TiDB Domain（域）子系统。
//
// Domain 是会话之上的全局服务容器，管理 InfoSchema、DDL、统计信息、
// 慢查询、Plan Replayer、系统变量缓存等。本文件声明子模块并重新导出
// 对外常用类型（`Domain`、`DomainConfig`、InfoSchema 加载相关抽象等）。

#![allow(dead_code)]

/// AutoID 分配器到 Domain 规范 KV 的事务适配层。
mod autoid_store;
/// Domain 核心实现与配置。
pub mod canonical_domain;
/// Domain 运行时与生命周期。
pub mod domain;
/// Domain 侧系统变量相关逻辑。
pub mod domain_sysvars;
/// Domain 上下文（session / store 绑定）。
pub mod domainctx;
/// 从 Domain 提取诊断 / 运维信息。
pub mod extract;
/// 历史统计信息异步 dump worker。
pub mod historical_stats;
/// Optimizer Trace 目录命名。
pub mod optimize_trace;
/// Plan Replayer（执行计划回放）相关。
pub mod plan_replayer;
/// Plan Replayer dump 文件处理。
pub mod plan_replayer_dump;
pub mod resource_group_runtime;
/// RU（Request Unit）统计。
pub mod ru_stats;
/// Runaway（失控查询）管控。
pub mod runaway;
pub mod ruv2_reporter;
/// Schema 一致性检查。
pub mod schema_checker;
/// 系统变量缓存。
pub mod sysvar_cache;
/// 测试辅助工具。
pub mod test_helper;
/// Top-N 慢查询收集。
pub mod topn_slow_query;

/// 统计任务使用的共享 Prometheus 指标入口。
///
/// 公开重导出使依赖 Domain 的统计集成测试可读取与运行时相同的 collector，避免
/// 为测试重复定义计数器。
pub use astersql_metrics as metrics;
/// SQL Killer：按条件终止会话 SQL。
pub use astersql_util_sqlkiller::sqlkiller::SQLKiller;
/// InfoSchema 加载与 DDL 元数据变更服务抽象。
pub use canonical_domain::{
    DdlMetadataChange, DdlMetadataService, InfoSchemaLoader, KvInfoSchemaLoader, LoadedInfoSchema,
    StorageHandle,
};
/// Domain 主类型、配置、错误与统计上下文。
pub use domain::{
    AutoAnalyzeExecutor, Domain, DomainConfig, DomainError, DomainStatsContext, SysProcesses,
};

#[cfg(test)]
#[path = "canonical_domain_test.rs"]
mod canonical_domain_test;
#[cfg(test)]
mod db_test;
#[cfg(test)]
mod domain_sysvars_test;
#[cfg(test)]
mod domain_test;
#[cfg(test)]
mod domain_utils_test;
#[cfg(test)]
mod domainctx_test;
#[cfg(test)]
mod extract_test;
#[cfg(test)]
mod main_test;
#[cfg(test)]
mod optimize_trace_test;
#[cfg(test)]
mod plan_replayer_dump_test;
#[cfg(test)]
mod plan_replayer_handle_test;
#[cfg(test)]
mod plan_replayer_slow_log_test;
#[cfg(test)]
mod plan_replayer_test;
#[cfg(test)]
mod ru_stats_test;
#[cfg(test)]
mod runaway_test;
#[cfg(test)]
mod schema_checker_test;
#[cfg(test)]
mod sysvar_cache_test;
#[cfg(test)]
mod test_helper_test;
#[cfg(test)]
mod topn_slow_query_test;
