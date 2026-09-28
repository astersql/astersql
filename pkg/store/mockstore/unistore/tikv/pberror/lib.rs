// Copyright 2026 AsterSQL.
// pberror crate 入口：包装 TiKV `errorpb::Error` 并对外再导出。
//
// Region 错误（Region Error）指请求落到错误副本、错误 Store、
// Epoch 不匹配等场景时，服务端通过 protobuf 返回的可重试错误描述。
// 本文件声明 `pberror` 子模块，并将 `kvproto::errorpb` 与模块内类型统一再导出。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

pub mod pberror;
pub use kvproto::errorpb;
pub use pberror::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "pberror_test.rs"]
mod pberror_test;
