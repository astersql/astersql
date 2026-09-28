// Copyright 2026 AsterSQL.

// TTL 专用会话（session）门面 crate。
//
// 封装 TTL 扫描/删除路径所需的会话抽象：SQL 执行、事务模式、时区复位、
// 语句打断与相位追踪钩子。对外 re-export `session` 子模块中的类型与 `new_session`。

#![allow(dead_code, non_snake_case)]

/// TTL 会话实现与相关 trait / 类型定义。
pub mod session;

pub use session::*;

#[cfg(test)]
#[path = "session_test.rs"]
mod session_test;
#[cfg(test)]
#[path = "sysvar_test.rs"]
mod sysvar_test;
