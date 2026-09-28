// Copyright 2026 AsterSQL.

// infoschema 内部测试辅助 crate 的库入口。
//
// 再导出 mockstore，并公开 `sizer`（内存占用估算）与 `testkit`（元数据测试夹具）子模块。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// 再导出 mock 存储，供内部测试搭建假 TiKV。
pub use astersql_store_mockstore as mockstore;

/// 内存占用（Sizeof）估算实现。
pub mod sizer;
/// InfoSchema 测试用的元数据夹具与 TestStore。
pub mod testkit;

pub use sizer::*;
pub use testkit::*;

#[cfg(test)]
#[path = "sizer_test.rs"]
mod sizer_test;

#[cfg(test)]
#[path = "testkit_test.rs"]
mod testkit_test;
