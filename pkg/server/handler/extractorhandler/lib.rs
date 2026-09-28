// Copyright 2026 AsterSQL.

// extractorhandler crate 根：导出 Extract 任务 HTTP 处理实现，并挂接测试模块。
//
// Extract 用于按时间窗口抽取执行计划等信息；本 crate 对齐 Go 的
// `pkg/server/handler/extractorhandler` 包结构。

#![allow(dead_code, non_snake_case)]

pub mod extractor;
pub use extractor::*;

#[cfg(test)]
mod extract_test;
#[cfg(test)]
mod extractor_test;
#[cfg(test)]
mod main_test;
