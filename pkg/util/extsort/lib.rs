// Copyright 2026 AsterSQL.

// `util/extsort` crate 入口：外部排序（external sort）相关组件。
//
// 对应 Go `util/extsort`。当排序数据量超过内存预算时，会把中间结果spill（溢出）
// 到磁盘再归并；本 crate 导出磁盘排序器与通用外部排序器，供执行器等模块复用。

#![allow(dead_code)]

/// 基于磁盘 spill 的排序器实现。
pub mod disk_sorter;
/// 通用外部排序器封装。
pub mod external_sorter;
pub use disk_sorter::*;
pub use external_sorter::*;

#[cfg(test)]
#[path = "disk_sorter_test.rs"]
/// 磁盘排序器单元测试。
mod disk_sorter_test;
#[cfg(test)]
#[path = "external_sorter_test.rs"]
/// 外部排序器单元测试。
mod external_sorter_test;
#[cfg(test)]
#[path = "disk_sorter_1_aster_unit_test.rs"]
/// 迁移对照的 Aster 单元测试。
mod migration_aster_unit_test;
