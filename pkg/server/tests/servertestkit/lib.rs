// Copyright 2026 AsterSQL.

// server 测试工具包（servertestkit）crate 根模块。
//
// 对外再导出 `testkit` 中的 TiDB 测试套件构造与 TopSQL 辅助能力，
// 供 server 集成测试复用同一套启动/清理流程。

#![allow(dead_code)]

pub mod testkit;
pub use testkit::*;

#[cfg(test)]
mod testkit_test;
