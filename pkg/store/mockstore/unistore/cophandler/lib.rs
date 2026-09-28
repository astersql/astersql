// Copyright 2026 AsterSQL.

// unistore cophandler crate 入口。
//
// 导出 Analyze、closure 执行、cop 请求处理与 MPP/TopN 等子模块，并挂入测试。

#![allow(dead_code)]

/// Analyze 统计收集。
pub mod analyze;
/// Closure 风格线性执行器。
pub mod closure_exec;
/// Coprocessor 请求类型与入口。
pub mod cop_handler;
/// MPP 任务与 tunnel。
pub mod mpp;
/// MPP 算子执行。
pub mod mpp_exec;
/// TopN 堆。
pub mod topn;

#[cfg(test)]
#[path = "analyze_test.rs"]
/// Analyze 与 Go 实现的一致性回归测试。
mod analyze_test;
#[cfg(test)]
#[path = "closure_exec_test.rs"]
/// Closure executor 与 Go 实现的一致性回归测试。
mod closure_exec_test;
#[cfg(test)]
#[path = "cop_handler_test.rs"]
/// cophandler 功能测试。
mod cop_handler_test;
#[cfg(test)]
#[path = "main_test.rs"]
/// 包级 TestMain 占位测试。
mod main_test;
#[cfg(test)]
#[path = "mpp_exec_test.rs"]
/// MPP executor 与 Go 实现的一致性回归测试。
mod mpp_exec_test;
#[cfg(test)]
#[path = "mpp_test.rs"]
/// MPP tunnel 与 Exchange 的 Go 一致性回归测试。
mod mpp_test;
#[cfg(test)]
#[path = "topn_test.rs"]
/// TopN heap 与 Go 实现的一致性回归测试。
mod topn_test;
