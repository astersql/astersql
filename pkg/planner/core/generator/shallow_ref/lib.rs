// Copyright 2026 AsterSQL.

// 逻辑算子浅引用（ShallowRef / Copy-on-Write）代码生成器 crate 入口。
//
// 浅引用（ShallowRef）为逻辑算子树提供写时复制语义：共享未修改字段，仅在变更时复制。
// 本模块导出 `shallow_ref_generator`，并在测试配置下挂载同目录单测。

#![allow(dead_code, non_snake_case)]

/// 浅引用生成器实现。
pub mod shallow_ref_generator;

/// 再导出生成器公开 API。
pub use shallow_ref_generator::*;

#[cfg(test)]
#[path = "shallow_ref_test.rs"]
mod shallow_ref_test;

#[cfg(test)]
#[path = "shallow_ref_generator_test.rs"]
mod shallow_ref_generator_test;
