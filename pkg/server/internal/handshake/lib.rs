// Copyright 2026 AsterSQL.

// MySQL 初始握手（handshake）子 crate 根模块。
//
// 对外再导出 `handshake` 中的协议结构（如 Response41），
// 供 server 在认证阶段解析客户端握手响应。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 握手协议实现模块（Response41 等）。
pub mod handshake;
pub use handshake::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
