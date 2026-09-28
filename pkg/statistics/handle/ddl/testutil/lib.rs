// Copyright 2026 AsterSQL.

// DDL 统计测试辅助 crate 入口。
//
// 导出在事务包装下处理 DDL 事件、按类型查找事件通道消息等测试工具，
// 对应 Go `statistics/handle/ddl/testutil`。

#![allow(non_snake_case)]

/// DDL 测试工具函数与 trait 定义。
mod util;
pub use util::*;

#[cfg(test)]
#[path = "util_test.rs"]
/// 测试工具自身的单元测试。
mod util_test;
