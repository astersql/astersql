// Copyright 2026 AsterSQL.

// 多值索引（Multi-Valued Index，MV-Index）表达式测试 crate 入口。
//
// 多值索引把 JSON 数组展开为多条索引项；本包聚合 `main_test` 环境与
// `multi_valued_index_test` 行为回归用例。

#![allow(dead_code)]

// 测试入口：环境初始化（表达式索引开关、系统时区）。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
// 核心用例：JSON 展开、索引键编解码与 MEMBER OF 语义。
#[cfg(test)]
#[path = "multi_valued_index_test.rs"]
mod multi_valued_index_test;
