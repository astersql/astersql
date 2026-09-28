// Copyright 2026 AsterSQL.

// UCA（Unicode Collation Algorithm）权重数据与生成器子 crate 入口。
//
// 对应 Go `pkg/util/collate/ucadata`：导出公共哨兵常量、unicode 4.0.0 / 9.0.0
// 生成表，以及用于从 allkeys 文本再生这些表的 generator。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code,
    ambiguous_glob_reexports
)]

/// 公共常量（如 `LongRune8`）与 go:generate 说明。
pub mod data;
/// allkeys 解析与 Go 源码生成器。
#[path = "generator/lib.rs"]
pub mod generator;
/// Unicode 9.0.0 ai_ci 权重表（生成产物）。
pub mod unicode_0900_ai_ci_data_generated;
/// Unicode 4.0.0 unicode_ci 权重表（生成产物）。
pub mod unicode_ci_data_generated;
pub use data::*;
pub use unicode_0900_ai_ci_data_generated::*;
pub use unicode_ci_data_generated::*;

#[cfg(test)]
#[path = "data_1_aster_unit_test.rs"]
mod data_aster_unit_test;
#[cfg(test)]
#[path = "unicode_ci_data_generated_3_aster_unit_test.rs"]
mod unicode_0400_generated_aster_unit_test;
#[cfg(test)]
#[path = "unicode_0900_ai_ci_data_test.rs"]
mod unicode_0900_ai_ci_data_test;
#[cfg(test)]
#[path = "unicode_0900_ai_ci_data_generated_2_aster_unit_test.rs"]
mod unicode_0900_generated_aster_unit_test;
#[cfg(test)]
#[path = "unicode_ci_data_original_test.rs"]
mod unicode_ci_data_original_test;
#[cfg(test)]
#[path = "unicode_ci_data_test.rs"]
mod unicode_ci_data_test;
