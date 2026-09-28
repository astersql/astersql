// Copyright 2026 AsterSQL.

// optimizor handler crate：优化器相关 HTTP 接口（trace、plan replayer、统计信息）。
//
// 对应 Go `pkg/server/handler/optimizor`，用于下载优化追踪、回放计划与统计 dump。

#![allow(dead_code)]

pub mod optimize_trace;
pub mod plan_replayer;
pub mod statistics_handler;

#[cfg(test)]
mod main_test;
#[cfg(test)]
mod optimize_trace_test;
#[cfg(test)]
mod plan_replayer_test;
#[cfg(test)]
mod statistics_handler_test;
