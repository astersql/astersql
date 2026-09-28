// Copyright 2026 AsterSQL.

// `ucaimpl` crate 入口：导出 Unicode CI 校对器实现代码生成器。
//
// 对应 Go `pkg/util/collate/ucaimpl`：将 `main.rs` 中的模板渲染逻辑作为
// `generator` 模块公开，并用迁移期单元测试校验生成结果与 Go 产物一致。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

#[path = "main.rs"]
/// 模板渲染与 `generate_file` / `Data` 等生成 API（实现见 `main.rs`）。
pub mod generator;
pub use generator::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// 生成输出与 Go 参考文件逐字节对照的迁移测试。
mod migration_aster_unit_test;
