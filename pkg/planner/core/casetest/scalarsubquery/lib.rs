// Copyright 2026 AsterSQL.

// `scalarsubquery` casetest crate 入口。
//
// 聚合标量子查询相关用例：TestMain 初始化语义，以及 explain / explain analyze
// 场景下的解析与输出裁剪。仅在 `cfg(test)` 下挂载子模块。

#![allow(dead_code)]

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// 标量子查询 explain / explain analyze 语法与裁剪辅助测试。
#[cfg(test)]
#[path = "cases_test.rs"]
mod cases_test;
