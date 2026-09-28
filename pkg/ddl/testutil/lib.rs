// Copyright 2026 AsterSQL.

// DDL 测试工具（testutil）crate 入口。
//
// 提供 Operator 管线测试用的 Source/Sink 通道，以及会话并发执行、
// schema 状态匹配、表模式切换等公共辅助，供各 DDL 测试包复用。

#![allow(dead_code)]

/// Operator 测试 Source/Sink 与无缓冲通道实现。
pub mod operator;
#[cfg(test)]
mod operator_test;
/// DDL 测试运行时 trait 与表状态/表模式辅助函数。
pub mod testutil;
