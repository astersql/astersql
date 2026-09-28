// Copyright 2026 AsterSQL.
// Lightning 手动内存分配包：提供与 Go `manual` 对齐的 `New`/`Free` 与带引用计数的 `Allocator`。
//
// 用于导入链路中绕过常规堆分配路径、统一大块字节缓冲的生命周期管理。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 带可选引用计数的分配器封装。
pub mod allocator;
/// 默认（含 cgo 路径语义）的手动分配与释放实现。
pub mod manual;
/// 无 cgo 构建下的回退分配实现。
pub mod manual_nocgo;

pub use allocator::Allocator;
pub use manual::{Free, MaxArrayLen, New};

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
mod allocator_test;
