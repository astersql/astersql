// Copyright 2026 AsterSQL.

// `types` crate 根模块：聚合时间、十进制、字段类型、Datum、JSON 等子 crate 再导出。
//
// 对齐 Go `pkg/types` 包对外 API；测试模块在 `#[cfg(test)]` 下按文件挂载。

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

// —— 子 crate / 分组再导出（保持 Go 包级符号可达）——
pub use astersql_types_datum as datum;
pub use types_core_time as core_time;
pub use types_decimal as decimal;
pub use types_field_group as field;
pub use types_file_group as file_group;
pub use types_group_1 as scalar;
pub use types_group_1::{
    Context, DefaultStmtFlags, DefaultStmtNoWarningContext, Flags, NewContext, StrictContext,
};
pub use types_group_4 as metadata;
pub use types_json_binary as json_binary;
pub use types_json_functions as json_functions;
pub use types_json_path as json_path;
pub use types_time as time;
pub use types_vector as vector;

/// EXPLAIN 输出格式常量与辅助。
mod explain_format;
pub use explain_format::*;

/// 类型系统相关错误定义（按路径挂载 `errors.rs`）。
#[path = "errors.rs"]
pub mod errors;
pub use errors::*;

// —— 单元测试与 Aster 迁移对照测试 ——
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
mod benchmark_test;
#[cfg(test)]
mod binary_literal_test;
#[cfg(test)]
mod compare_test;
#[cfg(test)]
mod const_test;
#[cfg(test)]
mod context_test;
#[cfg(test)]
mod convert_test;
#[cfg(test)]
mod core_time_test;
#[cfg(test)]
mod datum_3_aster_unit_test;
#[cfg(test)]
mod datum_test;
#[cfg(test)]
mod enum_test;
#[cfg(test)]
mod errors_test;
#[cfg(test)]
mod etc_test;
#[cfg(test)]
mod export_test;
#[cfg(test)]
mod field_type_5_aster_unit_test;
#[cfg(test)]
mod field_type_test;
#[cfg(test)]
mod format_test;
#[cfg(test)]
mod fsp_test;
#[cfg(test)]
mod helper_test;
#[cfg(test)]
mod json_binary_7_aster_unit_test;
#[cfg(test)]
mod json_binary_functions_6_aster_unit_test;
#[cfg(test)]
mod json_binary_functions_test;
#[cfg(test)]
mod json_binary_test;
#[cfg(test)]
mod json_path_expr_8_aster_unit_test;
#[cfg(test)]
mod json_path_expr_test;
#[cfg(test)]
mod mydecimal_benchmark_test;
#[cfg(test)]
mod mydecimal_test;
#[cfg(test)]
mod overflow_add_sub_test;
#[cfg(test)]
mod overflow_duration_test;
#[cfg(test)]
mod overflow_test;
#[cfg(test)]
mod set_test;
#[cfg(test)]
mod time_test;
#[cfg(test)]
mod vector_test;

#[cfg(test)]
mod json_constants_test;
