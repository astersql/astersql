// Copyright 2026 AsterSQL.

//! Crate entry for `lightning/pkg/importer/mock`
//! (Go package `github.com/pingcap/tidb/lightning/pkg/importer/mock`).
//!
//! 这个入口把 importer 的 mock 侧能力拆成两层：
//! `stubs` 提供跨包依赖的最小替身类型，
//! `mock` 则组合这些替身，构造可预测的导入源和目标端假数据。
//! 因此这里的导出顺序也在表达依赖方向：
//! 先暴露底层占位边界，再暴露面向测试的高层辅助对象。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    unused_mut,
    unused_assignments,
    clippy::all
)]

#[path = "stubs.rs"]
mod stubs;
// 这些 re-export 让 mock 测试可以像 Go 同包代码一样直接拿到边界类型。
pub use stubs::ast;
pub use stubs::context;
pub use stubs::dbterror;
pub use stubs::errno;
pub use stubs::filter;
pub use stubs::model;
pub use stubs::mydump;
pub use stubs::objstore;
pub use stubs::pdhttp;
pub use stubs::units;
pub use stubs::{Error, MemStorage, Result};

#[path = "mock.rs"]
mod mock;
// 高层 mock 构造器依赖上面的 stub 类型来拼装测试输入与观测结果。
pub use mock::*;

#[cfg(test)]
#[path = "mock_test.rs"]
mod mock_test;
// 单元测试验证 mock 数据结构本身的行为是否稳定。

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;
// parity 测试额外检查 Rust 公共契约与 Go 测试预期是否保持一致。
