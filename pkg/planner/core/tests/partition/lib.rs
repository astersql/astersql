// Copyright 2026 AsterSQL.

// 分区表（partition）相关规划器集成测试的 crate 根模块。
//
// 通过 `#[path]` 挂载 `bench_test`（分区裁剪 / 分区表规划相关基准与行为用例）。
// 分区表将一行数据按分区键映射到不同物理分区；优化器可做分区裁剪（partition pruning），
// 只扫描谓词命中的分区以降低 I/O。

#![allow(dead_code)]

/// 分区表规划与裁剪相关的基准 / 行为测试。
#[cfg(test)]
#[path = "bench_test.rs"]
mod bench_test;

#[cfg(test)]
#[path = "partition_aster_unit_test.rs"]
mod partition_aster_unit_test;
