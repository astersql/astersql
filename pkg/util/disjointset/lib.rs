// Copyright 2026 AsterSQL.

// 并查集（disjoint set / union-find）工具 crate 入口。
//
// 导出稠密整数版 `SimpleIntSet` 与通用稀疏版 `Set`；测试配置下挂载 Go 对应测试、

#![allow(non_snake_case)]

/// 稠密整数并查集实现。
mod int_set;
/// 通用（可哈希）元素并查集实现。
mod set;

pub use int_set::*;
pub use set::*;

#[cfg(test)]
/// 稠密整数并查集单元测试。
mod int_set_test;

#[cfg(test)]
/// 通用并查集单元测试。
mod set_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// AsterSQL 迁移补充回归测试。
mod migration_aster_unit_test;
