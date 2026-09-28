// Copyright 2026 AsterSQL.

// TTL（Time-To-Live，存活时间）任务触发 HTTP handler 子包入口。
//
// TTL 用于按过期策略清理表数据；本包暴露手动触发指定库表 TTL job 的接口。

#![allow(dead_code)]

/// TTL job 触发 handler 实现。
pub mod ttl;

#[cfg(test)]
#[path = "ttl_test.rs"]
mod ttl_test;
