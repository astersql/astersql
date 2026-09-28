// Copyright 2026 AsterSQL.

// `traceevent` 集成测试 crate 入口。
//
// 通过 `#[path]` 挂载 `integration_test.rs`，对照 Go
// `pkg/util/traceevent/test` 的 session / FlightRecorder 集成场景。

#![allow(dead_code)]

/// 集成测试：模式 API、Go 参考场景字符串与端到端行为对照。
#[cfg(test)]
#[path = "integration_test.rs"]
mod integration_test;
