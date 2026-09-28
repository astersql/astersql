// Copyright 2026 AsterSQL.

// 集群 INFORMATION_SCHEMA 表测试包入口。
//
// 对应 Go `pkg/infoschema/test/clustertablestest`：声明并导出 `harness`
// 子模块，供 `cluster_tables_test` / `tables_test` 等用例复用特权门控、
// digest 归组、慢查询解析等自包含替身实现。`#![allow(dead_code)]` 用于
// 迁移期保留尚未被全部用例引用的辅助 API。

#![allow(dead_code)]

pub mod harness;
