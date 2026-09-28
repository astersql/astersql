// Copyright 2026 AsterSQL.

// 统计 Handle 类型与接口定义 crate 入口。
//
// 聚合 `interfaces` 中的 trait/结构体，供 storage、syncload、autoanalyze 等子模块共享。

#![allow(non_snake_case)]

/// Handle 各子系统的 trait 与 DTO 定义。
mod interfaces;
/// 对外再导出全部接口类型。
pub use interfaces::*;

#[cfg(test)]
/// 接口相关轻量单元测试。
#[path = "interfaces_test.rs"]
mod interfaces_test;
