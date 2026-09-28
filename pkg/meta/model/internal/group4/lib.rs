// Copyright 2026 AsterSQL.

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

// Compatibility boundary for the former table-model slice.
//
// The formal model identity lives in `astersql-meta-model-group1`.  This
// crate deliberately re-exports that identity instead of defining parallel
// metadata structs, so legacy `group_4` imports and the package root share
// exactly the same `TableInfo`, `ColumnInfo`, and `IndexInfo` types.
//
// 兼容边界：本 crate 不再定义平行的表模型结构，而是单向再导出 group1
// 中的正式身份（`TableInfo` / `ColumnInfo` / `IndexInfo`），保证遗留
// `group_4` 导入与包根使用完全相同的类型。

pub use group_1::*;
