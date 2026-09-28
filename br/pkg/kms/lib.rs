// Copyright 2026 AsterSQL.

//! `astersql_br_pkg_kms` crate 入口：汇总 KMS 公共类型、厂商后端与测试替身。
//! 模块加载顺序为 stubs → common → kms → aws/gcp；对外统一 re-export。
//! 对齐 Go `br/pkg/kms` 包边界，不在此文件实现业务逻辑。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

// 配置/客户端桩：真实 SDK 未接线前的依赖边界。
#[path = "stubs.rs"]
pub mod stubs;

// 加密/明文密钥包装与算法标签。
#[path = "common.rs"]
pub mod common;

// Provider trait 定义。
#[path = "kms.rs"]
pub mod kms;

// AWS KMS 后端。
#[path = "aws.rs"]
pub mod aws;

// GCP KMS 后端。
#[path = "gcp.rs"]
pub mod gcp;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "aws_test.rs"]
mod aws_test;

#[cfg(test)]
#[path = "gcp_test.rs"]
mod gcp_test;

#[cfg(test)]
#[path = "kms_test.rs"]
mod kms_test;

// 将各子模块公开符号提升到 crate 根，便于 master_key 等调用方直接 use。
pub use aws::*;
pub use common::*;
pub use gcp::*;
pub use kms::*;
pub use stubs::*;
