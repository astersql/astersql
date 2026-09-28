// Copyright 2026 AsterSQL.
#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

// S3 API Mock 子 crate 入口。
//
// 对应 Go 侧 `s3store/mock` 包：导出可注入的 `S3API` mock，
// 供 `s3store` 单元测试在不访问真实对象存储的情况下验证读写、列举与分片上传等行为。
// 对象存储（object store）此处指兼容 S3 协议的远程键值对象服务（如 AWS S3）。

pub mod s3api_mock;
/// 将 mock 实现中的类型与构造函数直接暴露给依赖方。
pub use s3api_mock::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
