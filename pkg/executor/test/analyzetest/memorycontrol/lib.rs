// Copyright 2026 AsterSQL.

// ANALYZE 内存控制测试 crate 入口。
//
// 对应 Go `pkg/executor/test/analyzetest/memorycontrol` 包。验证 ANALYZE
// 在实例内存限额、会话 Tracker（内存追踪器）附着/解绑，以及进程信息
// 存活/过期场景下的取消与回收语义。

#![allow(dead_code)]

/// 包级 TestMain：启用统计缓存内存配额并设置 StatsCacheMemQuota。
#[cfg(test)]
mod main_test;
/// ANALYZE 内存限额、Tracker 与会话取消链路用例。
#[cfg(test)]
mod memory_control_test;
