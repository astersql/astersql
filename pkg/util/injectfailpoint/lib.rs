// Copyright 2026 AsterSQL.

// Failpoint 注入辅助 crate：为 DXF 等路径提供随机错误注入能力。
//
// 对应 Go `pkg/util/injectfailpoint`。`random_retry` 实现按概率返回错误；
// 测试模块验证与 Go `rand.Float64` / `rand.Intn` 语义一致。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

pub mod random_retry;
pub use random_retry::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
