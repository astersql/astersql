// Copyright 2026 AsterSQL.

// `meta` crate 入口：TiDB 元数据（schema / table / AutoID / 策略等）读写与测试桩。
//
// 元数据存放在 KV 的 `m` 前缀下，经 `Mutator`（写）与 `Reader`（快照读）访问。
// 本文件挂载 harness 桩、核心 meta 实现、AutoID 访问器与 reader，并在测试配置下编译包级测试。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]
/// 自引用别名：测试通过该路径引用本 crate。
extern crate self as astersql_meta;
/// 测试用依赖桩：kv、structure、model 等内存实现。
mod harness;
/// 对外再导出 harness 中的桩类型与常量。
pub use harness::*;
/// 元数据键布局、Mutator 事务读写与序列化（对应 Go `meta.go`）。
mod meta;
/// 单表 AutoID / Sequence 字段访问器组合（对应 Go `meta_autoid.go`）。
mod meta_autoid;
/// 基于快照的只读元数据访问（对应 Go Reader）。
mod reader;
/// 对外再导出 meta 模块公开项。
pub use meta::*;
/// 对外再导出 AutoID 访问器相关公开项。
pub use meta_autoid::*;
/// 对外再导出 reader 公开项。
pub use reader::*;

/// Harness 依赖桩与 Go 对应契约测试。
#[cfg(test)]
#[path = "harness_test.rs"]
mod harness_test;
/// Mutator / 键助手 / 策略等核心行为测试。
#[cfg(test)]
#[path = "meta_test.rs"]
mod meta_test;
/// Go→Rust 迁移行为对齐测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
