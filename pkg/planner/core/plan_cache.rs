// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 计划缓存（Plan Cache）相关的会话侧辅助函数。
//
// 覆盖参数值写入会话上下文、USE_PLAN_CACHE 提示判定，以及 Point Get
// 执行器是否可安全复用的条件检查（需自动提交、非事务内、非过期读，
// 且语句 schema 版本与当前 schema 版本一致）。

use crate::{Datum, PlanCacheStmt};

/// 将执行参数写入已缓存语句的参数标记，并校验参数个数。
pub fn SetParameterValuesIntoSCtx(
    stmt: &mut PlanCacheStmt,
    params: Vec<Datum>,
) -> Result<(), String> {
    if stmt.Params.len() != params.len() {
        return Err("wrong parameter count".into());
    }
    // Go 会先求值每个参数，再把 Datum 写回对应 marker 并标记 Execute 状态。
    // 此边界接收的已经是求值后的 Datum，因此必须保留值而非只记录状态。
    for (marker, datum) in stmt.Params.iter_mut().zip(params) {
        marker.datum = Some(datum);
        marker.in_execute = true;
    }
    Ok(())
}
/// 判断预编译 SQL 或绑定（binding）是否带有 USE_PLAN_CACHE 提示。
pub fn containUsePlanCacheHintInPreparedSQLOrBinding(
    stmt: &PlanCacheStmt,
    binding_hint: bool,
    matched: bool,
) -> bool {
    stmt.HasUsePlanCacheHint || binding_hint && matched
}
/// 判断 Point Get 执行器是否可安全复用：自动提交、非事务、非 stale 读且 schema 版本一致。
pub fn IsSafeToReusePointGetExecutor(
    autocommit: bool,
    in_txn: bool,
    stale: bool,
    stmt_version: i64,
    schema_version: i64,
) -> bool {
    autocommit && !in_txn && !stale && stmt_version == schema_version
}
