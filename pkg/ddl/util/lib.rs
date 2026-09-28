// Copyright 2026 AsterSQL.

// DDL 工具子 crate 入口。
//
// 聚合失效表锁检测、通用工具（etcd/会话/删除范围/作业暂停等）
// 以及 schema 版本 watcher，并向外 re-export。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// 失效表锁检测。
mod dead_table_lock_checker;
/// 失效表锁检测单元测试。
#[cfg(test)]
#[path = "dead_table_lock_checker_test.rs"]
mod dead_table_lock_checker_test;
/// 通用 DDL 工具类型与函数。
mod util;
/// etcd schema 路径 watcher 实现（独立文件路径挂载）。
#[path = "watcher.rs"]
mod watcher_impl;

/// 导出失效表锁检测 API。
pub use dead_table_lock_checker::*;
/// 导出通用工具 API。
pub use util::*;
/// 导出 watcher API。
pub use watcher_impl::*;

/// util 单元测试模块。
#[cfg(test)]
#[path = "util_test.rs"]
mod util_test;
