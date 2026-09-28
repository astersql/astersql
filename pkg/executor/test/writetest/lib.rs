// Copyright 2026 AsterSQL.

// 写路径（write path）执行器测试 crate 入口。
//
// 聚合 `main_test` 与 `write_test`：覆盖 DML 写入错误报告等行为，
// 与生产侧 `write` 模块的列名/行号错误格式对齐。

#![allow(dead_code)]
#[cfg(test)]
mod main_test;
#[cfg(test)]
mod write_test;
