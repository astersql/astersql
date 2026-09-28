// Copyright 2026 AsterSQL.

// `intervaltimezone` 测试 crate 入口。
//
// 挂接时区污染相关的 `interval_timezone_test` 与公共 harness 的 `main_test`，
// 验证分析作业 start_time 使用会话时区而非被污染的系统时区。

#![allow(dead_code)]

#[cfg(test)]
#[path = "interval_timezone_test.rs"]
/// 时区污染场景下的失败间隔查询测试。
mod interval_timezone_test;

#[cfg(test)]
#[path = "main_test.rs"]
/// 公共测试 harness 初始化（对应 Go TestMain / SetupForCommonTest）。
mod main_test;
