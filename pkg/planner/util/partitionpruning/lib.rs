// Copyright 2026 AsterSQL.

// 分区剪枝（partition pruning）工具 crate。
//
// 分区剪枝根据查询谓词推断只需扫描哪些分区定义下标，避免全表分区扫描。
// 对外再导出 [`partition_prune`] 中的 `partition_pruning` 等 API。

#![allow(dead_code)]

/// 分区剪枝核心实现：按 Hash/Range/List 等分区类型过滤分区集合。
pub mod partition_prune;
/// 再导出分区剪枝公共 API。
pub use partition_prune::*;

#[cfg(test)]
mod partition_prune_test;
