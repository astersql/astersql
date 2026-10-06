// Copyright 2026 AsterSQL.

// executor 内部 exec 子模块入口。
//
// 封装执行器公共基座（`executor`）与索引使用率上报（`indexusage`）：
// 前者提供执行器生命周期与行批次消费约定，后者统计各索引在查询中的
// 访问次数、行数与表行占比，供优化器与统计信息侧消费。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals
)]

/// Ordered LIMIT 的语句级自适应准入控制器。
pub mod adaptive_limit_controller;
/// 执行器公共基类型与接口。
pub mod executor;
/// 索引使用率采样与上报。
pub mod indexusage;

// 执行器基类与 indexusage 的单元测试仅在 test 配置下编译。
#[cfg(test)]
mod adaptive_limit_controller_test;
#[cfg(test)]
mod executor_test;
#[cfg(test)]
mod indexusage_test;
