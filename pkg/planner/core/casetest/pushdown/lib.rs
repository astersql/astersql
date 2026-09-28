// Copyright 2026 AsterSQL.

// `pushdown` casetest crate 入口。
//
// 谓词/算子下推（pushdown）相关用例：优化器将过滤、投影等尽量推到存储引擎
// （TiKV/TiFlash）侧执行，以减少网络传输与计算量。本 crate 在 `cfg(test)` 下挂载
// `main_test` 与 `push_down_test`。

#![allow(dead_code)]

/// 对应 Go TestMain：common test 初始化语义。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// 谓词/算子下推规划用例。
#[cfg(test)]
#[path = "push_down_test.rs"]
mod push_down_test;
