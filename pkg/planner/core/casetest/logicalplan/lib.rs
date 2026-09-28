// Copyright 2026 AsterSQL.

// `logicalplan` casetest crate 入口。
//
// 聚合逻辑计划（Logical Plan）构建相关用例：schema 推导、UNION 类型提升、
// year 列比较等。仅在 `cfg(test)` 下挂载子模块。
//
// 逻辑计划：优化前的关系代数算子树（投影、选择、连接、聚合等），
// 不含具体物理算子与存储引擎选择。

#![allow(dead_code)]

/// 对应 Go TestMain 的初始化生命周期断言。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// 逻辑计划构建器：EXISTS/GROUP BY schema、UNION 类型提升、year 比较回归。
#[cfg(test)]
#[path = "logical_plan_builder_test.rs"]
mod logical_plan_builder_test;
