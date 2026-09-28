// Copyright 2026 AsterSQL.

// 事务信息（TxnInfo）子 crate：汇总与明细导出。
//
// 对应 Go `session/txninfo`：[`summary`] 提供聚合视图，[`txn_info`] 描述单事务
// 起止时间戳、运行状态与 SQL digest 等可观测字段，供 `SHOW` / 诊断接口使用。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 事务信息汇总（按维度聚合）。
pub mod summary;
/// 单事务元数据与状态字段定义。
pub mod txn_info;

/// 迁移期单元测试（与 Go 行为对照）。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
