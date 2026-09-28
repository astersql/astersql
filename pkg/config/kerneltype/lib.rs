// Copyright 2026 AsterSQL.

// kerneltype crate：内核类型（Kernel Type）识别模块。
//
// 数据库内核存在两种形态：
// - `classic`（经典内核）：传统的部署形态，计算与存储按经典架构组织；
// - `nextgen`（下一代内核）：新一代架构形态，通常面向存算分离等新特性。
//
// 本 crate 通过 Cargo 的 `nextgen` feature（编译期特性开关）在编译时
// 决定当前二进制属于哪种内核类型，并对外统一导出对应实现，
// 供配置系统等上层代码在运行时查询内核形态、做差异化行为。

// 未启用 nextgen 特性时，编译经典内核实现子模块。
#[cfg(not(feature = "nextgen"))]
mod classic;
/// 文档说明子模块（仅承载文档性内容）。
mod doc;
// 启用 nextgen 特性时，编译下一代内核实现子模块。
#[cfg(feature = "nextgen")]
mod nextgen;
// `type` 是 Rust 关键字，需用原始标识符 r#type 作为模块名；
// 该模块定义内核类型的公共类型与常量。
mod r#type;

// 按编译特性二选一地重导出对应内核实现，使调用方无需感知特性开关，
// 直接使用统一的公共接口即可。
#[cfg(not(feature = "nextgen"))]
pub use classic::*;
#[cfg(feature = "nextgen")]
pub use nextgen::*;
// 无条件重导出内核类型的公共定义。
pub use r#type::*;

// 单元测试模块：从 Go(TiDB) 迁移而来的测试，用 #[path] 指定测试文件路径。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
// 针对 r#type 模块的单元测试。
#[cfg(test)]
mod type_test;
