// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 统计信息解锁（unlock stats）实现。
//
// 在解除表/分区的统计锁定前，先把 `mysql.stats_table_locked` 中累计的增量
// （count / modify_count）合并回 `mysql.stats_meta`，再删除锁定行。
// 锁定期间修改量缓存在 locked 表，解锁时需回灌，保证优化器看到的行数与修改计数连续。

use crate::{
    GetLockedTables, RestrictedSQLExecutor, StatsError, StatsLockTable,
    generateStableSkippedPartitionsMessage, generateStableSkippedTablesMessage,
    query_lock::QueryLockedTablesWithExecutor, unlockAction, unlockedStatus,
};
use std::collections::HashMap;
/// 从锁定表读取指定 table_id 的 count / modify_count 增量。
pub const selectDeltaSQL: &str =
    "SELECT count, modify_count FROM mysql.stats_table_locked WHERE table_id = %?";
/// 将增量合并进 stats_meta：更新 version，累加 count（下限为 0）与 modify_count。
pub const updateDeltaSQL: &str = "UPDATE mysql.stats_meta SET version = %?, count = IF(count + %? > 0, count + %?, 0), modify_count = modify_count + %? WHERE table_id = %?";
/// 删除指定 table_id 在 stats_table_locked 中的锁定记录。
pub const DeleteLockSQL: &str = "DELETE FROM mysql.stats_table_locked WHERE table_id = %?";
/// 批量解锁表：对已锁定的表及其分区回灌增量并删锁；未锁定的表记入跳过列表并生成稳定提示消息。
pub fn RemoveLockedTables(
    s: &mut dyn RestrictedSQLExecutor,
    tables: &HashMap<i64, StatsLockTable>,
) -> Result<String, StatsError> {
    // 先查出当前全部已锁定的表/分区 ID
    let all = QueryLockedTablesWithExecutor(s)?;
    let mut ids = Vec::new();
    // 收集待解锁的物理 ID：表 ID 及其分区 ID
    for (id, t) in tables {
        ids.push(*id);
        ids.extend(t.PartitionInfo.keys().copied())
    }
    let locked = GetLockedTables(&all, &ids);
    let mut skipped = Vec::new();
    for (id, t) in tables {
        // 未锁定则跳过，避免误操作
        if !locked.contains_key(id) {
            skipped.push(t.FullName.clone());
            continue;
        }
        // 回灌表级增量后删除表锁
        updateStatsForTable(s, *id)?;
        deleteLock(s, *id)?;
        // 同步处理该表下已锁定的分区
        for pid in t.PartitionInfo.keys() {
            if locked.contains_key(pid) {
                updateStatsForPartition(s, *pid, *id)?;
                deleteLock(s, *pid)?;
            }
        }
    }
    Ok(generateStableSkippedTablesMessage(
        tables.len(),
        skipped,
        unlockAction,
        unlockedStatus,
    ))
}
/// 解锁某表下的若干分区；若整表仍处于锁定状态则整体跳过，避免分区与表锁定状态不一致。
pub fn RemoveLockedPartitions(
    s: &mut dyn RestrictedSQLExecutor,
    tid: i64,
    name: &str,
    parts: &HashMap<i64, String>,
) -> Result<String, StatsError> {
    let all = QueryLockedTablesWithExecutor(s)?;
    // 整表已锁时不允许单独解分区锁
    if all.contains_key(&tid) {
        return Ok(format!("skip unlocking partitions of locked table: {name}"));
    }
    let ids: Vec<_> = parts.keys().copied().collect();
    let locked = GetLockedTables(&all, &ids);
    let mut skipped = Vec::new();
    for id in &ids {
        if !locked.contains_key(id) {
            skipped.push(parts[id].clone())
        } else {
            // 分区增量同时回灌到分区自身与所属表
            updateStatsForPartition(s, *id, tid)?;
            deleteLock(s, *id)?;
        }
    }
    Ok(generateStableSkippedPartitionsMessage(
        &ids,
        name,
        skipped,
        unlockAction,
        unlockedStatus,
    ))
}
/// 通过执行器将 count / modify_count 增量应用到指定物理 ID 的 stats_meta。
fn updateDelta(
    s: &mut dyn RestrictedSQLExecutor,
    count: i64,
    modify: i64,
    id: i64,
) -> Result<(), StatsError> {
    s.ApplyStatsDelta(id, count, modify)
}
/// 读取表锁定行中的增量并写回该表的 stats_meta。
fn updateStatsForTable(s: &mut dyn RestrictedSQLExecutor, id: i64) -> Result<(), StatsError> {
    let (c, m) = getStatsDeltaFromTableLocked(s, id)?;
    updateDelta(s, c, m, id)
}
/// 读取分区锁定增量，分别应用到分区与所属表（tid）的元数据。
fn updateStatsForPartition(
    s: &mut dyn RestrictedSQLExecutor,
    pid: i64,
    tid: i64,
) -> Result<(), StatsError> {
    let (c, m) = getStatsDeltaFromTableLocked(s, pid)?;
    updateDelta(s, c, m, pid)?;
    updateDelta(s, c, m, tid)
}
/// 从 stats_table_locked 取出指定 ID 的 (count, modify_count)。
fn getStatsDeltaFromTableLocked(
    s: &mut dyn RestrictedSQLExecutor,
    id: i64,
) -> Result<(i64, i64), StatsError> {
    s.LockedStatsDelta(id)
}

/// 删除指定物理 ID 的统计锁定行。
fn deleteLock(s: &mut dyn RestrictedSQLExecutor, id: i64) -> Result<(), StatsError> {
    s.DeleteStatsLock(id)
}
