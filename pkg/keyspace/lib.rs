// Copyright 2026 AsterSQL.

// `keyspace` crate 入口：多租户键空间（Keyspace）相关工具。
//
// Keyspace 用于在 nextgen TiDB 中隔离数据与操作，实现物理集群上的逻辑多租户。
// 本文件负责：
// - 放宽命名风格以贴近 Go 迁移产物；
// - 重导出配置、部署模式与内核类型依赖；
// - 挂载 `doc` / `keyspace` / `username_policy` 子模块并对外再导出；
// - 在测试配置下编译迁移对齐测试与包级单元测试。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 自引用别名：测试与子模块通过该路径引用本 crate。
extern crate self as astersql_keyspace;

/// 重导出全局配置 crate（别名 `config`）。
pub use config_crate as config;
/// 将配置 crate 的公开项提升到本 crate 根。
pub use config_crate::*;
/// 部署模式（Starter / Premium 等），影响用户名前缀策略。
pub use deploymode_dependency::deploymode;
/// 内核类型判定（Classic / NextGen）。
pub use kerneltype;

/// 包级文档（Go doc.go 对应说明）。
pub mod doc;
/// Keyspace 核心 API：编解码、etcd 命名空间、配置读取与日志字段。
mod keyspace;
/// 对外再导出 keyspace 模块全部公开项。
pub use keyspace::*;
/// 用户名校验与前缀策略实现。
mod username_policy;
/// 对外再导出 username_policy 模块全部公开项。
pub use username_policy::*;

/// Go→Rust 迁移行为对齐测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

/// Keyspace 包级单元测试（含隔离子进程用例）。
#[cfg(test)]
#[path = "keyspace_test.rs"]
mod keyspace_test;
