// Copyright 2026 AsterSQL.

// 内存相关执行器测试 crate 入口。
//
// 对应 Go `pkg/executor/test/memtest` 包。验证语句级 MemTracker（内存追踪器）
// 在 cleanup 后回落到原消耗，以及全局内存仲裁器（Global Mem Arbitrator）
// 对 work-mode / soft-limit / server-limit 的解析与生效语义。
// 本 crate 仅在 `#[cfg(test)]` 下编译测试源，不导出生产 API。

#![allow(dead_code)]

/// 包级 TestMain：公共测试初始化与慢日志阈值等全局配置。
#[cfg(test)]
mod main_test;
/// 语句 MemTracker cleanup 与全局内存仲裁器单元用例。
#[cfg(test)]
mod mem_test;
