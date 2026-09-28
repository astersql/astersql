// Copyright 2026 AsterSQL.

// 系统会话（syssession）crate 根模块。
//
// 导出内部会话包装、高级会话池及测试辅助；测试模块仅在 `cfg(test)` 下编译。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 会话包装、所有权转移与 SQL 代理执行。
mod session;
pub use session::*;
/// 可复用系统会话池。
mod pool;
pub use pool::*;
/// 测试用会话构造与扩展方法。
mod session_test_util;
pub use session_test_util::*;

#[cfg(test)]
mod main_test;
#[cfg(test)]
mod pool_test;
#[cfg(test)]
mod session_integration_test;
#[cfg(test)]
mod session_test;
#[cfg(test)]
mod session_test_util_test;
