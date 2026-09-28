// Copyright 2026 AsterSQL.

// 表达式向量化代码生成器共享辅助库入口。
//
// 对应 Go 包 `expression/generator/helper`：集中定义 `TypeContext` 与各
// `types.EvalType` 的预置常量，供 compare/control/other/string/time 等生成器
// 在展开 Go 源码模板时复用同一套类型命名与固定长/变长列语义。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 共享类型模板上下文（`TypeContext`）与各 EvalType 预置常量实现。
pub mod helper;
/// 将 helper 中的类型上下文符号直接暴露给依赖本 crate 的生成器。
pub use helper::*;

/// 校验 TypeContext 预置常量与 Go helper 包取值一致的迁移单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
