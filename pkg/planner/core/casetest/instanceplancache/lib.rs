// Copyright 2026 AsterSQL.

// `instanceplancache` casetest crate 入口。
//
// 聚合实例级执行计划缓存（Instance Plan Cache）相关用例：内置函数、并发、
// TPC-C、DML 及杂项行为。仅在 `cfg(test)` 下挂载子模块。
//
// Instance Plan Cache：在 TiDB 实例范围内跨会话共享的 prepared 语句执行计划缓存，
// 与会话级 plan cache 相对；命中时可通过 `@@last_plan_from_cache` 观察。

#![allow(dead_code)]

#[cfg(test)]
#[path = "support.rs"]
mod support;

/// 内置函数与 instance plan cache 交互的测试。
#[cfg(test)]
#[path = "builtin_func_test.rs"]
mod builtin_func_test;
/// 多会话并发场景下 instance plan cache 的正确性测试。
#[cfg(test)]
#[path = "concurrency_test.rs"]
mod concurrency_test;
/// TPC-C 风格负载下的 instance plan cache 并发压测。
#[cfg(test)]
#[path = "concurrency_tpcc_test.rs"]
mod concurrency_tpcc_test;
/// DML（INSERT/UPDATE/DELETE）语句的 instance plan cache 行为测试。
#[cfg(test)]
#[path = "dml_test.rs"]
mod dml_test;
/// 对应 Go TestMain / 主压测编排：随机表、worker 与 query pattern。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
/// 变量、binding、事务、DDL、权限、分区裁剪等杂项 instance plan cache 用例。
#[cfg(test)]
#[path = "others_test.rs"]
mod others_test;
