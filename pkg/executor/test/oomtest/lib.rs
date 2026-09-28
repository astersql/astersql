// Copyright 2026 AsterSQL.

// OOM（内存超限）执行器测试 crate 入口。
//
// 对应 Go `pkg/executor/test/oomtest` 包。验证 update/insert/replace/delete
// 等路径上 MemTracker 超限时的日志捕获（oomCapture）、rateLimitAction 委托，
// 以及 OOM Action 回退优先级（DefLogPriority）。
// 本 crate 仅在 `#[cfg(test)]` 下编译测试源，不导出生产 API。

#![allow(dead_code)]

/// OOM 日志捕获、MemTracker 超限语义与 Action 优先级用例。
#[cfg(test)]
mod oom_test;
