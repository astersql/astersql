// Copyright 2026 AsterSQL.

// `astersql-planner-funcdep` crate 入口：函数依赖（Functional Dependency, FD）图。
//
// 函数依赖描述「给定部分列的值可唯一确定另一些列的值」，优化器据此做列裁剪、
// 唯一键推导、外连接后依赖传播等。本 crate 暴露 `FDSet`/`fdEdge` 及文档说明模块，
// 并将 `FastIntSet` 作为列 unique id 集合的底层表示。

#![allow(non_snake_case, non_camel_case_types, dead_code)]

extern crate self as astersql_planner_funcdep;

/// Lax/Strict FD 与 Cond-FD 的理论说明（无推导实现）。
pub mod doc;
/// FD 图核心：边集合、闭包、投影、外连接传播等。
#[path = "fd_graph.rs"]
pub mod funcdep;
/// 列 unique id 的稠密整数集合，来自 `astersql_util_intset`。
pub mod intset {
    pub use astersql_util_intset::*;
}

pub use funcdep::*;

/// 迁移单元测试：闭包、等价类、常量提升等与 Go 对齐。
#[cfg(test)]
#[path = "doc_1_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// 算子级 ExtractFD 传播规则的 API 序列重放测试。
#[cfg(test)]
#[path = "extract_fd_test.rs"]
mod extract_fd_test;
