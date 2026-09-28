// Copyright 2026 AsterSQL.

// DDL 失败路径测试 crate 入口。
//
// 聚合 `fail_db_test`（失败 DDL 的 schema/数据原子性）与 `main_test`
// （消除时间相关非确定性的套件配置）两个测试模块，对应 Go
// `pkg/ddl/tests/fail` 包。

#![allow(dead_code)]

#[cfg(test)]
mod fail_db_test;
#[cfg(test)]
mod main_test;
