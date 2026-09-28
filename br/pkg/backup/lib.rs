// Copyright 2026 AsterSQL.
//
// 本模块是 `br/pkg/backup` 的 Rust 入口。
// 这里不承载具体备份算法，而是负责把各子模块按 Go 包结构重新拼装出来。
// 调用方通常只依赖本文件导出的公共符号，而不会关心实现散落在哪个子文件。
// 因此这里的 `mod` 顺序需要和真实依赖方向保持一致，避免测试或导出路径漂移。
// 下面先注册生产代码模块，再在 `#[cfg(test)]` 下挂接 Rust 侧对齐测试。
// 这样可以保证正常编译不会意外引入测试专用依赖。
// 最后的 `pub use` 则模拟 Go 包的“平铺导出”体验。
// 上层 crate 只需 `use crate::backup::*` 即可拿到常用入口。
// 对迁移任务来说，本文件也是检查哪些子模块已经接入 Rust 的总索引。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    unused_mut,
    clippy::all
)]

#[path = "stubs.rs"]
pub mod stubs;

#[path = "limit.rs"]
pub mod limit;

#[path = "store.rs"]
pub mod store;

#[path = "schema.rs"]
pub mod schema;

#[path = "client.rs"]
pub mod client;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "limit_test.rs"]
mod limit_test;

#[cfg(test)]
#[path = "store_test.rs"]
mod store_test;

#[cfg(test)]
#[path = "schema_test.rs"]
mod schema_test;

#[cfg(test)]
#[path = "schema_merge_option_test.rs"]
mod schema_merge_option_test;

#[cfg(test)]
#[path = "client_test.rs"]
mod client_test;

pub use client::*;
// 继续导出各子模块公共符号，维持与 Go `package backup` 接近的使用方式。
pub use limit::*;
pub use schema::*;
pub use store::*;
pub use stubs::*;
