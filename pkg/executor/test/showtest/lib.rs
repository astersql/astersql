// Copyright 2026 AsterSQL.

// SHOW 语句执行器测试 crate 入口。
//
// 对应 Go `pkg/executor/test/showtest` 包。SHOW（如 SHOW DATABASES、
// SHOW CREATE DATABASE）用于查询元数据与建库建表 DDL 文本；本 crate
// 挂载包级冒烟用例与 show 结果构造回归测试。

#![allow(dead_code)]

/// 包级 TestMain / 冒烟：校验 INFORMATION_SCHEMA 在库名列表中的排序位置。
#[cfg(test)]
mod main_test;
/// SHOW CREATE DATABASE 等结果串构造与转义规则的单元测试。
#[cfg(test)]
mod show_test;
