// Copyright 2026 AsterSQL.

// ddlhelper crate：DDL 测试辅助门面。
//
// 导出 [`BuildTableInfoFromASTForTest`]，并在测试配置下挂载单元测试模块。

#![allow(dead_code)]

/// AST → TableInfo 辅助实现。
mod helper;

pub use helper::BuildTableInfoFromASTForTest;

/// helper 行为的单元测试。
#[cfg(test)]
mod helper_aster_unit_test;
