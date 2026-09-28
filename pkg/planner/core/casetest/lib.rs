// Copyright 2026 AsterSQL.

// `casetest` 顶层 crate 入口。
//
// 聚合 planner/core 用例测试中未划入独立子 crate 的模块：集成回归、
// 计划树、统计信息与 TiFlash 谓词下推等。仅在 `cfg(test)` 下挂载子模块。
//
// 执行计划（plan）：优化器为 SQL 选定的算子树；物理计划可下推 TiKV/TiFlash。

#![allow(dead_code)]

/// 跨组件集成回归：verbose explain、TiFlash isolation、fix-control 等。
#[cfg(test)]
mod integration_test;
/// 对应 Go TestMain 的初始化生命周期断言。
#[cfg(test)]
pub(crate) mod main_test;
/// 通用计划树 / explain 形状用例。
#[cfg(test)]
mod plan_test;
/// 统计信息（ANALYZE / 代价估计）相关用例。
#[cfg(test)]
mod stats_test;
/// TiFlash 谓词下推（predicate push-down）相关用例。
#[cfg(test)]
mod tiflash_predicate_push_down_test;
