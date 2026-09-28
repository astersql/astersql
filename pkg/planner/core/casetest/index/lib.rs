// Copyright 2026 AsterSQL.

// 索引访问路径规划用例测试 crate 入口。
//
// 优化器在索引扫描（IndexScan）与回表查找（IndexLookup）之间择优：
// 若所需列均被索引覆盖则为覆盖索引扫描（CoveringRange），否则需回表。
// 本 crate 在 `cfg(test)` 下挂载 `index_test` 与 `main_test`。

#![allow(dead_code)]

/// 覆盖索引扫描与 IndexLookup 选择逻辑回归。
#[cfg(test)]
mod index_test;
/// 对照 Go：确定性统计信息相关的 runtime 设置断言。
#[cfg(test)]
mod main_test;
