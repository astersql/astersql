// Copyright 2026 AsterSQL.

// Cascades Join 变换规则子模块入口。
//
// 聚合与 Join 相关的逻辑变换规则（transformation rule），当前主要导出
// `join_to_apply`：将普通 Join 改写为 Apply（相关子查询/侧向连接语义），
// 以便后续解相关（decorrelate）规则继续处理。测试配置下挂载对应单元测试。

#![allow(non_snake_case)]

/// Join 转 Apply 的变换规则实现。
mod join_to_apply;
pub use join_to_apply::*;

#[cfg(test)]
mod join_to_apply_aster_unit_test;
