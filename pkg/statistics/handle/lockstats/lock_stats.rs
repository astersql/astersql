// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 表/分区统计加锁与跳过消息生成。
//
// 将 table_id（及分区 ID）写入 `mysql.stats_table_locked`，并刷新 `stats_meta.version`；
// 已锁定对象跳过并生成稳定排序的提示文案。事务由 `WithSession(true, …)` 包裹。

use crate::{
    GetLockedTables, QueryLockedTables, RestrictedSQLExecutor, SessionRef, StatsError,
    StatsLockTable, query_lock::QueryLockedTablesWithExecutor,
};
use std::collections::HashMap;
/// 加锁动作文案片段，用于跳过消息。
pub const lockAction: &str = "locking";
/// 解锁动作文案片段。
pub const unlockAction: &str = "unlocking";
/// 已锁定状态文案。
pub const lockedStatus: &str = "locked";
/// 已解锁状态文案。
pub const unlockedStatus: &str = "unlocked";
/// 向 `stats_table_locked` 插入锁记录的 SQL 模板。
pub const insertSQL: &str = "INSERT INTO mysql.stats_table_locked (table_id) VALUES (%?) ON DUPLICATE KEY UPDATE table_id = %?";
/// 用当前 StartTS 更新 `stats_meta.version` 的 SQL 模板。
pub const updateMetaVersionSQL: &str =
    "UPDATE mysql.stats_meta SET version = %? WHERE table_id = %?";
/// 基于会话池的统计锁实现。
pub struct statsLockImpl {
    pool: SessionRef,
}
/// 构造统计锁句柄。
pub fn NewStatsLock(pool: SessionRef) -> statsLockImpl {
    statsLockImpl { pool }
}
impl statsLockImpl {
    /// 在事务中为多张表（含其分区）加统计锁，返回跳过说明。
    pub fn LockTables(&self, tables: &HashMap<i64, StatsLockTable>) -> Result<String, StatsError> {
        let mut result = None;
        self.pool.WithSession(true, &mut |executor| {
            result = Some(AddLockedTables(executor, tables)?);
            Ok(())
        })?;
        Ok(result.unwrap_or_default())
    }
    /// 在事务中为指定表的若干分区加锁；父表已锁则整表跳过。
    pub fn LockPartitions(
        &self,
        tid: i64,
        name: &str,
        parts: &HashMap<i64, String>,
    ) -> Result<String, StatsError> {
        let mut result = None;
        self.pool.WithSession(true, &mut |executor| {
            result = Some(AddLockedPartitions(executor, tid, name, parts)?);
            Ok(())
        })?;
        Ok(result.unwrap_or_default())
    }
    /// 在事务中移除多张表的统计锁。
    pub fn RemoveLockedTables(
        &self,
        tables: &HashMap<i64, StatsLockTable>,
    ) -> Result<String, StatsError> {
        let mut result = None;
        self.pool.WithSession(true, &mut |executor| {
            result = Some(crate::unlock_stats::RemoveLockedTables(executor, tables)?);
            Ok(())
        })?;
        Ok(result.unwrap_or_default())
    }
    /// 在事务中移除指定表的分区统计锁。
    pub fn RemoveLockedPartitions(
        &self,
        tid: i64,
        name: &str,
        parts: &HashMap<i64, String>,
    ) -> Result<String, StatsError> {
        let mut result = None;
        self.pool.WithSession(true, &mut |executor| {
            result = Some(crate::unlock_stats::RemoveLockedPartitions(
                executor, tid, name, parts,
            )?);
            Ok(())
        })?;
        Ok(result.unwrap_or_default())
    }
    /// 查询给定 ID 集合中当前仍被锁定的子集。
    pub fn GetLockedTables(&self, ids: &[i64]) -> Result<HashMap<i64, ()>, StatsError> {
        Ok(GetLockedTables(&QueryLockedTables(&self.pool)?, ids))
    }
    /// 测试用：返回全部已锁表 ID 映射（不清空持久化数据，命名对齐 Go）。
    pub fn GetTableLockedAndClearForTest(&self) -> Result<HashMap<i64, ()>, StatsError> {
        QueryLockedTables(&self.pool)
    }
}
/// 为尚未加锁的表与分区写入锁记录，并生成跳过已锁表的稳定消息。
pub fn AddLockedTables(
    s: &mut dyn RestrictedSQLExecutor,
    tables: &HashMap<i64, StatsLockTable>,
) -> Result<String, StatsError> {
    let locked = QueryLockedTablesWithExecutor(s)?;
    let mut ids = Vec::new();
    // 收集表 ID 及其分区 ID，用于批量过滤已锁集合。
    for (id, t) in tables {
        ids.push(*id);
        ids.extend(t.PartitionInfo.keys().copied())
    }
    let locked = GetLockedTables(&locked, &ids);
    let mut skipped = Vec::new();
    let mut to_lock = Vec::new();
    for (id, t) in tables {
        if locked.contains_key(id) {
            skipped.push(t.FullName.clone())
        } else {
            to_lock.push(*id);
        }
        // 分区独立加锁：父表未在 skipped 逻辑中阻止未锁分区。
        for pid in t.PartitionInfo.keys() {
            if !locked.contains_key(pid) {
                to_lock.push(*pid);
            }
        }
    }
    for id in to_lock {
        InsertLockAndUpdateVersion(s, id)?;
    }
    Ok(generateStableSkippedTablesMessage(
        tables.len(),
        skipped,
        lockAction,
        lockedStatus,
    ))
}
/// 为分区加锁；若父表已锁则直接返回跳过整表分区的提示。
pub fn AddLockedPartitions(
    s: &mut dyn RestrictedSQLExecutor,
    tid: i64,
    name: &str,
    parts: &HashMap<i64, String>,
) -> Result<String, StatsError> {
    let locked = QueryLockedTablesWithExecutor(s)?;
    if locked.contains_key(&tid) {
        return Ok(format!("skip locking partitions of locked table: {name}"));
    }
    let ids: Vec<_> = parts.keys().copied().collect();
    let locked = GetLockedTables(&locked, &ids);
    let mut skipped = Vec::new();
    let mut to_lock = Vec::new();
    for id in &ids {
        if locked.contains_key(id) {
            skipped.push(parts[id].clone())
        } else {
            to_lock.push(*id);
        }
    }
    for id in to_lock {
        InsertLockAndUpdateVersion(s, id)?;
    }
    Ok(generateStableSkippedPartitionsMessage(
        &ids,
        name,
        skipped,
        lockAction,
        lockedStatus,
    ))
}
/// 生成表级跳过消息：名称排序以保证多表提示稳定可测。
pub fn generateStableSkippedTablesMessage(
    count: usize,
    mut names: Vec<String>,
    action: &str,
    status: &str,
) -> String {
    names.sort();
    if names.is_empty() {
        return String::new();
    }
    let joined = names.join(", ");
    // 区分单表/多表，以及部分成功与全部跳过。
    if count > 1 {
        if count > names.len() {
            format!("skip {action} {status} tables: {joined}, other tables {status} successfully")
        } else {
            format!("skip {action} {status} tables: {joined}")
        }
    } else {
        format!("skip {action} {status} table: {joined}")
    }
}
/// 生成分区级跳过消息，语义与表级对应。
pub fn generateStableSkippedPartitionsMessage(
    ids: &[i64],
    table: &str,
    mut names: Vec<String>,
    action: &str,
    status: &str,
) -> String {
    names.sort();
    if names.is_empty() {
        return String::new();
    }
    let joined = names.join(", ");
    if ids.len() > 1 {
        if ids.len() > names.len() {
            format!(
                "skip {action} {status} partitions of table {table}: {joined}, other partitions {status} successfully"
            )
        } else {
            format!("skip {action} {status} partitions of table {table}: {joined}")
        }
    } else {
        format!("skip {action} {status} partition of table {table}: {joined}")
    }
}
/// 插入锁行并更新 `stats_meta.version`，使缓存可感知锁变更。
pub fn InsertLockAndUpdateVersion(
    s: &mut dyn RestrictedSQLExecutor,
    id: i64,
) -> Result<(), StatsError> {
    s.InsertStatsLock(id)?;
    s.UpdateStatsMetaVersion(id)?;
    Ok(())
}
