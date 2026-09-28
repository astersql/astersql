// Copyright 2026 AsterSQL.

// 外键（Foreign Key）执行器测试 crate 入口。
//
// 对应 Go `pkg/executor/test/fktest`：验证外键约束校验、引用动作
// （ON DELETE / ON UPDATE）以及 DDL 创建外键时的 SchemaState 推进
// （None → WriteOnly → WriteReorganization → Public）。
// 聚合 `foreign_key_test` 与包级 `main_test`。

#![allow(dead_code)]

/// 外键目录校验与 schema 状态机生命周期用例。
#[cfg(test)]
mod foreign_key_test;
/// 包级 TestMain：慢日志阈值等全局测试配置语义。
#[cfg(test)]
mod main_test;
