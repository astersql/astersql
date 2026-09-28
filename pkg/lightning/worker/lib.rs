// Copyright 2026 AsterSQL.

// Lightning worker 池子 crate 入口。
//
// 提供有界 `Pool`/`Worker`：每个 Worker 是一枚并发许可（token），
// 通过有界通道借出与归还，用于限制 Lightning 导入路径上的并行度。
// 同时再导出 `lightning_metric`，供池内观测空闲 worker 数与申请耗时。

#![allow(non_snake_case)]

/// Worker 池实现：Apply / Recycle / HasWorker。
pub mod worker;
/// 再导出 Lightning 指标包，供本 crate 与调用方统一观测。
pub use lightning_metric as metric;
pub use worker::*;

#[cfg(test)]
#[path = "worker_test.rs"]
mod worker_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
