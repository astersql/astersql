// Copyright 2026 AsterSQL.

//! 流备份键区间（span）工具库入口；对齐 Go `br/pkg/streamhelper/spans`。
//!
//! 汇出有序 valued 树（`sorted`）、区间工具（`utils`）与按值索引结构（`value_sorted`），
//! 供 advancer/collector 合并 region 检查点进度。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

/// 按 StartKey 排序的 valued span 树（Merge / Traverse）。
#[path = "sorted.rs"]
pub mod sorted;

/// 区间重叠、折叠与字节比较辅助。
#[path = "utils.rs"]
pub mod utils;

/// 按 Value 组织的有序集合，用于取全局最小检查点。
#[path = "value_sorted.rs"]
pub mod value_sorted;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "sorted_test.rs"]
mod sorted_test;

#[cfg(test)]
#[path = "utils_test.rs"]
mod utils_test;

#[cfg(test)]
#[path = "value_sorted_test.rs"]
mod value_sorted_test;

/// 对外扁平再导出，保持与 Go 包级符号相近的调用面。
pub use sorted::*;
pub use utils::*;
pub use value_sorted::*;
