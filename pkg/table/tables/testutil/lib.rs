// Copyright 2026 AsterSQL.

// 表测试工具包入口：再导出 indexcheck 等辅助模块，供 tables 相关单测复用。

#![allow(non_snake_case)]

/// 索引一致性检查辅助（对应 Go `indexcheck`）。
pub mod indexcheck;
/// 将 indexcheck 公共项提升到 crate 根，便于测试直接引用。
pub use indexcheck::*;

#[cfg(test)]
#[path = "indexcheck_test.rs"]
mod indexcheck_test;
