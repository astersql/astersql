// Copyright 2026 AsterSQL.

//! 预分配表/分区 ID 包入口：导出 `alloc` 实现。
//! 对应 Go `br/pkg/restore/internal/prealloc_table_id`，在 restore 前
//! 批量预占 global ID，避免与集群现有 ID 冲突；checkpoint 可复用同一区间。
//! 测试经 `#[path]` 挂载，不与实现混编。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

#[path = "alloc.rs"]
pub mod alloc;

// 扁平再导出，调用方直接使用 Allocator / PreallocIDs 等类型与构造函数。
pub use alloc::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "alloc_test.rs"]
mod alloc_test;
