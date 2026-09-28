// Copyright 2026 AsterSQL.

// `util/column-mapping` crate 入口：列映射规则与行值改写。
//
// 对应 Go `pkg/util/column-mapping`。对外再导出 `column` 模块中的规则、
// 表达式与 `Mapping`；测试用全局锁保证 `SetPartitionRule` 串行化。

#![allow(non_snake_case, non_upper_case_globals)]

/// 列映射核心实现（规则、表达式、行值处理）。
pub mod column;
pub use column::*;

/// 表级规则选择器（trie）的再导出命名空间。
pub mod table_rule_selector {
    pub use table_rule_selector::*;
}

/// 保护 `SetPartitionRule` 全局位宽的测试互斥锁。
#[cfg(test)]
pub(crate) static PARTITION_RULE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// AsterSQL 迁移补充的列映射单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
