// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 查询当前已锁定的表/分区 ID。
//
// 从 `mysql.stats_table_locked` 读出 table_id，并提供按候选 ID 过滤的辅助函数。

use crate::{RestrictedSQLExecutor, SessionRef, StatsError};
use std::collections::HashMap;
/// 查询已锁表 ID 的 SQL（与 Go 常量对齐，执行器亦可走 `LockedTableIds`）。
pub const selectSQL: &str = "SELECT table_id FROM mysql.stats_table_locked";

/// 通过执行器拉取全部已锁 table_id，映射为 `HashMap<id, ()>` 便于快速 membership 判断。
pub(crate) fn QueryLockedTablesWithExecutor(
    executor: &mut dyn RestrictedSQLExecutor,
) -> Result<HashMap<i64, ()>, StatsError> {
    Ok(executor
        .LockedTableIds()?
        .into_iter()
        .map(|table_id| (table_id, ()))
        .collect())
}

/// 打开非事务会话查询全部已锁表 ID。
pub fn QueryLockedTables(session: &SessionRef) -> Result<HashMap<i64, ()>, StatsError> {
    let mut rows = None;
    session.WithSession(false, &mut |executor| {
        rows = Some(QueryLockedTablesWithExecutor(executor)?);
        Ok(())
    })?;
    Ok(rows.unwrap_or_default())
}
/// 从已锁全集中筛出 `ids` 里真正处于锁定状态的子集。
pub fn GetLockedTables(locked: &HashMap<i64, ()>, ids: &[i64]) -> HashMap<i64, ()> {
    ids.iter()
        .filter(|id| locked.contains_key(id))
        .map(|id| (*id, ()))
        .collect()
}
