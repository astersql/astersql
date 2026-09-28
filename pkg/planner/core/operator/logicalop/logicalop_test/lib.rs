// Copyright 2026 AsterSQL.

// 逻辑算子（logical operator）单元测试 crate 入口。
// 聚合各测试子模块，覆盖 Schema 克隆、谓词抽取、Cascades 夹具与计划执行边界用例。

#![allow(non_snake_case)]

/// Hash64 相等性相关测试。
#[cfg(test)]
mod hash64_equals_test;

/// 逻辑算子行为与拷贝语义测试。
#[cfg(test)]
mod logical_operator_test;

/// 内存表（mem table）谓词抽取器测试。
#[cfg(test)]
mod logical_mem_table_predicate_extractor_test;

/// Cascades 优化器测试夹具加载与入口。
#[cfg(test)]
mod main_test;

/// 逻辑计划推导与执行边界回归测试。
#[cfg(test)]
mod plan_execute_test;
