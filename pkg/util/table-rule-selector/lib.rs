// Copyright 2026 AsterSQL.

// 表规则选择器 crate 入口：按 schema/table 通配 pattern 检索规则。
//
// 对应 Go `pkg/util/table-rule-selector`。核心实现在 `trie_selector`；
// 支持 `*`/`?`/`[range]` 通配，以及 Insert/Replace/Append/Remove/Match。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// trie 选择器实现与 `Selector` 接口。
pub mod trie_selector;
/// 再导出选择器公共 API。
pub use trie_selector::*;

/// 对照 Go 的选择器主流程单测。
#[cfg(test)]
mod selector_test;

/// 对照 Go 字符串迭代边界的 UTF-8 回归测试。
#[cfg(test)]
mod trie_selector_test;

/// AsterSQL 迁移补充的选择器回归测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
