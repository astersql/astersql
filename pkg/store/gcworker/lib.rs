// Copyright 2026 AsterSQL.

// GC Worker（垃圾回收后台工作者）crate 入口。
//
// GC（Garbage Collection）负责清理 MVCC（多版本并发控制）中已过期、不再被
// 任何事务可见的历史版本。本 crate 导出 `gc_worker` 实现，并提供删除范围
// （delete-range）并发度计算，供调度边界在不启动后台 worker 时单独验证。

#![allow(dead_code)]

/// GC Worker 主实现模块：领导者租约、安全点推进、分布式/中心化 GC 等。
pub mod gc_worker;

/// Go-equivalent delete-range fan-out rule, exposed so the scheduling boundary
/// can be verified without starting the background worker.
///
/// 计算 delete-range（按键区间批量删除）任务的并发度。
/// `configured` 为配置上限，`automatic` 为真时按区间数量自动收缩，
/// `range_count` 为待删除区间数。
pub fn calculate_delete_range_concurrency(
    configured: usize,
    automatic: bool,
    range_count: usize,
) -> usize {
    // 配置上限除以并发除数，至少为 1。
    let maximum = (configured / gc_worker::ConcurrencyDivisor).max(1);
    // 按「每线程处理的请求数」估算所需线程数，至少为 1。
    let request_based = (range_count / gc_worker::RequestsPerThread).max(1);
    // 自动模式下取配置上限与请求估算的较小值；否则只用配置上限。
    if automatic {
        maximum.min(request_based)
    } else {
        maximum
    }
}

/// GC Worker 行为与常量的单元测试。
#[cfg(test)]
#[path = "gc_worker_test.rs"]
mod gc_worker_test;
/// 对应 Go TestMain 的默认常量与环境校验测试。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
