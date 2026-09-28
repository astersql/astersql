// Copyright 2026 AsterSQL.

// Fast Create Table（快速建表）DDL 测试 crate 入口。
//
// 对应 Go `pkg/ddl/tests/fastcreatetable`：验证系统变量
// `tidb_enable_fast_create_table` 开关、合并建表作业（Merged Job）
// 以及 AUTO_INCREMENT 起始值在快速建表路径下的行为。
// 使用 `#[path = ...]` 显式挂载同目录测试源文件。

#![allow(dead_code)]

#[cfg(test)]
#[path = "fastcreatetable_test.rs"]
mod fastcreatetable_test;
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
