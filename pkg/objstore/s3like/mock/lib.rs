// Copyright 2026 AsterSQL.

// s3like mock 子包入口。
//
// 导出基于 mockall 的 `PrefixClient` 替身，供权限与存储单测配置期望行为。

#![allow(non_snake_case, non_upper_case_globals)]

extern crate self as s3like_mock;
pub mod client_mock;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
