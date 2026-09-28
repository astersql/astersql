// Copyright 2026 AsterSQL.

// 全局配置同步（globalconfigsync）crate 入口。
//
// 导出 [`globalconfig`] 中的同步器与 PD 客户端接口，供 Domain 在
// 变量变更时把配置推送到集群全局配置存储。

extern crate self as astersql_domain_globalconfigsync;

/// 全局配置同步实现（Syncer / Client / Item）。
pub mod globalconfig;
pub use globalconfig::*;

#[cfg(test)]
#[path = "globalconfig_test.rs"]
mod globalconfig_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
