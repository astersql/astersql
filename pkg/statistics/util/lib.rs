// Copyright 2026 AsterSQL.

// 统计信息工具子模块入口。
//
// 聚合 JSON 形态的表/列统计结构（直方图 Histogram、CMSketch、FMSketch 等），
// 供统计句柄在导入、导出与内存估算时复用；测试模块验证谓词列排序与内存占用汇总。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// JSON 形态的统计对象定义（表、列、谓词列等）。
pub mod json_objects;
pub use json_objects::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// 迁移期单元测试：谓词列按 ID 排序与 protobuf 内存占用合计。
mod migration_aster_unit_test;
