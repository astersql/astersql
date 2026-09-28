// Copyright 2026 AsterSQL.

// DECIMAL / MyDecimal 内部 crate 入口。
//
// 通过 `#[path]` 挂接上级目录的 `mydecimal` 实现与迁移期单元测试，
// 供 types 子 crate 按依赖分组引用精确小数运算。

/// MySQL DECIMAL 精确小数实现（对齐 Go MyDecimal）。
#[path = "../../mydecimal.rs"]
pub mod mydecimal;

/// 迁移期 MyDecimal 单元测试。
#[cfg(test)]
#[path = "../../mydecimal_9_aster_unit_test.rs"]
mod mydecimal_9_aster_unit_test;
