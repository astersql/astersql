// Copyright 2026 AsterSQL.

// 通用容器工具 crate 入口。
//
// 导出有界最小堆 `BoundedMinHeap` 与并发安全映射 `SyncMap`；测试配置下
// 挂载对应 Go 单元测试与迁移回归用例。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 有界最小堆：维护最好的 N 个元素。
pub mod bounded_min_heap;
/// 带容量提示的并发安全 map。
pub mod sync_map;
pub use bounded_min_heap::*;
pub use sync_map::*;

#[cfg(test)]
#[path = "bounded_min_heap_test.rs"]
/// 有界最小堆 Go 对齐单元测试。
mod bounded_min_heap_test;

#[cfg(test)]
#[path = "sync_map_test.rs"]
/// SyncMap Go 对齐单元测试。
mod sync_map_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// AsterSQL 迁移补充回归测试。
mod migration_aster_unit_test;
