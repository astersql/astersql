// Copyright 2026 AsterSQL.

// Schema casetest crate 入口。
//
// 聚合「找不到列」等 schema 绑定相关回归：USING JOIN、视图/CTE 解析面。
// 仅在 `cfg(test)` 下挂载子模块。
//
// Schema：优化器与执行器使用的表结构/列元数据；列绑定失败会报 cannot find column。

#![allow(dead_code)]

/// 「找不到列」语法面回归（对应 Go cannot_find_column_test.go）。
#[cfg(test)]
#[path = "cannot_find_column_test.rs"]
mod cannot_find_column_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
