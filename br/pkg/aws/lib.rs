// Copyright 2026 AsterSQL.

//! BR AWS 辅助包入口：导出 EBS 快照相关实现。
//! 对应 Go `br/pkg/aws`，供云盘备份/恢复路径调用。
//! 测试经 path 挂载 parity 与 ebs 单测。
//! 本文件只装配模块边界，业务在 `ebs`。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

#[path = "ebs.rs"]
pub mod ebs;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "ebs_test.rs"]
mod ebs_test;

pub use ebs::*;
