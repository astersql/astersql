// Copyright 2026 AsterSQL.

// `txn` 会话事务测试包入口。
//
// 挂接 autocommit、惰性事务初始化、自动重试、只读提交与 rollback 等事务行为测试模块。

#![allow(dead_code)]

/// 测试入口与全局环境准备（对应 Go `TestMain`）。
#[cfg(test)]
mod main_test;
/// 事务状态位、commit/rollback、membuffer 与失败路径用例。
#[cfg(test)]
mod txn_test;
