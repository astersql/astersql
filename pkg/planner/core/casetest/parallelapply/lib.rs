// Copyright 2026 AsterSQL.

// `parallelapply` casetest crate 入口。
//
// 聚合并行 Apply（相关子查询 / LATERAL 的并行执行算子）相关用例：TestMain 初始化语义，
// 以及 hint / 语法 / explain 文本扫描辅助的直连真实测试。仅在 `cfg(test)` 下挂载子模块。

#![allow(dead_code)]

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// 并行 Apply：LATERAL / hint / ordered plan 等直连生产 API 测试。
#[cfg(test)]
#[path = "parallel_apply_test.rs"]
mod parallel_apply_test;
