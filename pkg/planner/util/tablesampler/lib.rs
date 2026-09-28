// Copyright 2026 AsterSQL.

// 表采样（TABLESAMPLE）辅助 crate 根。
//
// 对外再导出 `sample` 模块中的 `TableSampleInfo` 等类型，
// 供物理计划构造阶段携带采样 AST、完整 Schema 与分区表列表。

#![allow(non_snake_case)]

/// 表采样信息结构与构造函数。
mod sample;

/// 再导出 `TableSampleInfo` / `NewTableSampleInfo`。
pub use sample::*;

#[cfg(test)]
#[path = "sample_test.rs"]
/// `NewTableSampleInfo` 与内存估算的单元测试。
mod sample_test;
