// Copyright 2026 AsterSQL.

// ANALYZE panic 行为测试 crate 入口。
//
// 对应 Go `pkg/executor/test/analyzetest/panictest` 包。
// 验证 analyze worker / 结果处理路径发生 panic 时，能转换成可返回的错误，
// 而不是让 panic 泄漏到会话层。

#![allow(dead_code)]

/// 包级 TestMain：stats cache 配额开关。
#[cfg(test)]
mod main_test;
/// panic 转 AnalyzeError 的核心用例与 SQL fixture。
#[cfg(test)]
mod panic_test;

/// 直接挂载生产 `pkg/executor/analyze_utils.rs`，避免为 panictest 拉起整棵
/// `astersql-executor` 依赖图；该文件仅依赖 `std`，与 executor lib 中的模块一致。
#[cfg(test)]
#[path = "../../../analyze_utils.rs"]
mod analyze_utils;
