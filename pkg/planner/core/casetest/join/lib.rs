// Copyright 2026 AsterSQL.

// `join` casetest crate 入口。
//
// 聚合 JOIN（连接）相关用例：半连接顺序 hint、NULL-safe 等值、条件简化、
// join key 保留及历史 issue 回归。仅在 `cfg(test)` 下挂载子模块。
//
// JOIN：将两表按谓词匹配行并输出；物理实现含 HashJoin、IndexJoin 等。

#![allow(dead_code)]

/// JOIN hint 解析与历史回归的可执行用例（对应 Go join_test.go）。
#[cfg(test)]
#[path = "join_test.rs"]
mod join_test;

/// 对应 Go TestMain：全局配置（如 stats cache 内存配额）初始化断言。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
