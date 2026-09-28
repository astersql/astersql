// Copyright 2026 AsterSQL.

// Union 执行器子模块入口。
//
// 提供 `UNION`/`UNION ALL` 相关执行器实现：并发拉取多个子计划（child）结果并汇总输出。

#![allow(dead_code)]

pub mod union;

#[cfg(test)]
mod union_aster_unit_test;
