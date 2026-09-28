// Copyright 2026 AsterSQL.

// lockstore crate 入口：单写多读的 arena 跳表锁存储。
//
// 锁存储（lockstore）在 mock TiKV 中保存事务锁键值；底层用 arena（分块内存池）
// 与跳表（skiplist）实现，支持内存复用、有序迭代以及文件加载/转储。
// 本文件声明 arena、iterator、load_dump、lockstore 子模块并统一再导出。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// arena 分块内存分配与延迟复用。
pub mod arena;
/// MemStore 有序双向迭代器。
pub mod iterator;
/// 锁存储二进制加载与原子转储。
pub mod load_dump;
/// 跳表 MemStore 核心实现。
pub mod lockstore;
pub use arena::*;
pub use iterator::*;
pub use load_dump::*;
pub use lockstore::*;

/// 迁移相关单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

/// 加载与转储的 Go/Rust 一致性测试。
#[cfg(test)]
#[path = "load_dump_test.rs"]
mod load_dump_test;

/// MemStore / 迭代器 / 并发读写测试。
#[cfg(test)]
#[path = "lockstore_test.rs"]
mod lockstore_test;

/// 测试入口配置。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

// Go TestMain initializes the process before m.Run, including filtered/empty
// runs. A normal #[test] cannot provide that ordering under Rust's harness.
// These loader sections run once, before libtest starts any test threads.
#[cfg(test)]
#[used]
#[cfg_attr(
    target_vendor = "apple",
    unsafe(link_section = "__DATA,__mod_init_func")
)]
#[cfg_attr(
    all(unix, not(target_vendor = "apple")),
    unsafe(link_section = ".init_array")
)]
#[cfg_attr(windows, unsafe(link_section = ".CRT$XCU"))]
static INITIALIZE_TEST_ENVIRONMENT: extern "C" fn() = {
    extern "C" fn initialize() {
        testsetup::SetupForCommonTest();
    }
    initialize
};
