// Copyright 2026 AsterSQL.

//! `astersql_br_pkg_utils_iter` crate 入口：导出 BR 侧惰性迭代器工具。
//!
//! 模块分层：`iter` 核心类型 → `source*` 数据源 → `combinator*` 组合器。
//! 测试模块仅在 `cfg(test)` 下挂载，不进入发布产物。
//! 对外 `pub use` 扁平导出，调用方可直接 `use crate::{Map, CollectAll, ...}`。

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

/// 核心 Context / IterResult / CollectAll / Tap。
#[path = "iter.rs"]
pub mod iter;

/// 空迭代器等底层 source 辅助类型。
#[path = "source_types.rs"]
pub mod source_types;

/// FromSlice / OfRange / Fail / Func 等数据源工厂。
#[path = "source.rs"]
pub mod source;

/// TransformIter / FilterIter 等组合器实现类型。
#[path = "combinator_types.rs"]
pub mod combinator_types;

/// 公开组合器工厂（Map、Transform、FlatMap 等）。
#[path = "combinators.rs"]
pub mod combinators;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "source_test.rs"]
mod source_test;

#[cfg(test)]
#[path = "combinator_test.rs"]
mod combinator_test;

#[cfg(test)]
#[path = "as_seq_test.rs"]
mod as_seq_test;

#[cfg(test)]
#[path = "transform_backpressure_test.rs"]
mod transform_backpressure_test;

pub use combinator_types::*;
pub use combinators::*;
pub use iter::*;
pub use source::*;
pub use source_types::*;
