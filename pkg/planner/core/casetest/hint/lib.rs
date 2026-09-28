// Copyright 2026 AsterSQL.

// Optimizer Hint（优化器提示）用例测试 crate 入口。
//
// Hint 是写在 SQL 注释 `/*+ ... */` 中的优化器指令，用于指定索引、连接算法、
// 查询块归属等。本 crate 在 `cfg(test)` 下挂载 `hint_test` 与 `main_test`。

#![allow(dead_code)]

/// Hint 解析、QBHintHandler、ParseStmtHints/ParsePlanHints 等直连用例。
#[cfg(test)]
mod hint_test;
#[cfg(test)]
mod main_test;
