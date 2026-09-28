// Copyright 2026 AsterSQL.
// 对象存储 API（storeapi）crate 入口。
//
// 对应 Go 的 `pkg/objstore/storeapi`，抽象本地文件系统与各类云对象存储
//（Object Storage，按 key 读写的 blob 存储）的统一接口：读写、遍历、前缀、
// HTTP Range、权限探测等。本文件仅组织子模块并在测试配置下挂载单测。

#![allow(non_snake_case, non_upper_case_globals)]

/// 存储抽象、前缀规范化与权限相关类型定义。
pub mod storage;
pub use aws_smithy_runtime_api;
pub use storage::*;

/// 与 Go `storage_test.go` 对应的前缀与 HTTP Range 单测。
#[cfg(test)]
#[path = "storage_test.rs"]
mod storage_test;

/// Aster 迁移单元测试：覆盖 Prefix、BucketPrefix、权限常量与探测 key。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
