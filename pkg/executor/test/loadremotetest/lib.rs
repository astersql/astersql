// Copyright 2026 AsterSQL.

// 远程 LOAD（IMPORT INTO / LOAD DATA FROM URL 等）测试 crate 入口。
//
// 对应 Go `pkg/executor/test/loadremotetest` 包。远程导入从对象存储或
// HTTP 拉取 CSV 等文件并写入表；本 crate 挂载配置校验、多文件行号、
// 单文件字段解析、表头规范与包级测试入口契约。

#![allow(dead_code)]

/// CSV 非法配置（空字段终止符）负向冒烟。
#[cfg(test)]
mod error_test;
/// 多记录连续 `ReadRow` 时 `row_id` 递增冒烟。
#[cfg(test)]
mod multi_file_test;
/// 单行 CSV：引号内逗号与 `\N` NULL 解析冒烟。
#[cfg(test)]
mod one_csv_test;
/// CSV 表头去反引号规范化为列名冒烟。
#[cfg(test)]
mod util_test;
