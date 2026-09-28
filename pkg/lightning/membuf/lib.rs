// Copyright 2026 AsterSQL.
// Lightning membuf 包：块池与顺序分配 Buffer，配合内存 Limiter 控制导入峰值。
//
// 将大量小对象切到固定大小块中，降低分配与 GC/堆压力；可选 Limiter 限制借用块总量。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 块池、Buffer 与切片位置句柄。
pub mod buffer;
/// 可阻塞/非阻塞的内存配额限制器。
pub mod limiter;
pub use buffer::*;
pub use limiter::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
