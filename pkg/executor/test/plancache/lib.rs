// Copyright 2026 AsterSQL.

// 执行计划缓存（Plan Cache）测试 crate 入口。
//
// 对应 Go `pkg/executor/test/plancache` 包。Plan Cache 复用已优化的
// 执行计划，避免对参数化/预处理语句反复做解析与优化；本 crate 通过
// `#[path]` 挂载功能用例与包级 TestMain。仅在 `#[cfg(test)]` 下编译，
// 不导出生产 API。

#![allow(dead_code)]

/// 包级 TestMain：公共测试初始化入口。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
/// Plan Cache 命中/失效与相关执行器行为用例。
#[cfg(test)]
#[path = "plan_cache_test.rs"]
mod plan_cache_test;
