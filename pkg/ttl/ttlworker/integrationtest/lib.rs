// Copyright 2026 AsterSQL.

// TTL worker 集成测试 crate 入口。
//
// 在 `#[cfg(test)]` 下挂载会话池 helper、`JobManager`/`TtlJobAdapter` 与 timer 同步相关测试模块。

#![allow(dead_code)]

#[cfg(test)]
mod helpers_test;
#[cfg(test)]
mod manager_job_adapter_test;
#[cfg(test)]
mod timer_sync_test;
