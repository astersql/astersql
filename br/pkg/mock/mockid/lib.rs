// Copyright 2026 AsterSQL.

//! mockid crate 入口：导出与 Go `br/pkg/mock/mockid` 对齐的测试用 ID 分配器，
//! 并通过 `parity_test` 校验 Alloc/Rebase 契约；本文件只做模块装配与再导出。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

// 实现体：AtomicU64 递增的 IDAllocator（对应 mockid.go）。
#[path = "mockid.rs"]
pub mod mockid;

// 对外平铺导出 NewIDAllocator / IDAllocator，保持与 Go 包级符号同形。
pub use mockid::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;
