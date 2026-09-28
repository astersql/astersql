// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 工作负载仓库分区管家（housekeeper）。
//
// 计算次日 02:00 触发时刻，为仓库表创建按日分区并按保留天数删除过期分区；
// 仅 owner 节点执行，避免多副本重复 DDL。

use crate::worker::{repositoryTable, worker as Worker};
use crate::*;
use chrono::{DateTime, Datelike, Duration, Local, TimeZone};

/// 计算距下一个本地 02:00 的等待时长（已过则推到次日）。
pub fn calcNextTick(now: DateTime<Local>) -> Duration {
    let today = Local
        .with_ymd_and_hms(now.year(), now.month(), now.day(), 2, 0, 0)
        .single()
        .unwrap();
    if today > now {
        today - now
    } else {
        today + Duration::days(1) - now
    }
}

/// 若目标表尚缺覆盖 `now` 的分区，则生成并执行 `ADD PARTITION`。
pub fn createPartition(
    backend: &dyn RepositoryBackend,
    table: &repositoryTable,
    now: DateTime<Local>,
) -> Result<(), String> {
    let existing = backend.partitions(&table.destTable)?;
    let mut ranges = String::new();
    // generatePartitionRanges 返回 true 表示已存在足够分区，无需 DDL。
    if !generatePartitionRanges(&mut ranges, &existing, now)? {
        execRetry(
            backend,
            &format!(
                "ALTER TABLE `{workloadSchema}`.`{}` ADD PARTITION ({ranges})",
                table.destTable
            ),
            &[],
        )?;
    }
    Ok(())
}

/// 按保留天数删除目标表上过期的按日分区。
pub fn dropOldPartition(
    backend: &dyn RepositoryBackend,
    table: &repositoryTable,
    now: DateTime<Local>,
    retention: i32,
) -> Result<(), String> {
    for partition in backend.partitions(&table.destTable)? {
        let time = parsePartitionName(&partition)?;
        if (now - time).num_days() >= i64::from(retention) {
            execRetry(
                backend,
                &format!(
                    "ALTER TABLE `{workloadSchema}`.`{}` DROP PARTITION `{partition}`",
                    table.destTable
                ),
                &[],
            )?;
        }
    }
    Ok(())
}

impl Worker {
    /// 为所有工作负载表确保当前日期分区存在。
    pub fn createAllPartitions(&self, now: DateTime<Local>) -> Result<(), String> {
        for table in self.workloadTables.lock().unwrap().iter() {
            createPartition(self.backend.as_ref(), table, now)?;
        }
        Ok(())
    }
    /// 按保留策略清理所有表的过期分区；`retention == 0` 表示禁用清理。
    pub fn dropOldPartitions(&self, now: DateTime<Local>, retention: i32) -> Result<(), String> {
        if retention == 0 {
            return Ok(());
        }
        let mut errors = Vec::new();
        // 单表失败不中断，最后合并错误返回。
        for table in self.workloadTables.lock().unwrap().iter() {
            if let Err(error) = dropOldPartition(self.backend.as_ref(), table, now, retention) {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
    /// 返回一次管家任务闭包：仅 owner 时创建分区并删除过期分区。
    pub fn getHouseKeeper<'a>(
        &'a self,
        now: DateTime<Local>,
    ) -> impl FnOnce() -> Result<(), String> + 'a {
        move || {
            if self.backend.is_owner() {
                self.createAllPartitions(now)?;
                self.dropOldPartitions(now, self.intervals().2)?;
            }
            Ok(())
        }
    }
    /// 启动管家任务（当前等同于 `getHouseKeeper`）。
    pub fn startHouseKeeper<'a>(
        &'a self,
        now: DateTime<Local>,
    ) -> impl FnOnce() -> Result<(), String> + 'a {
        self.getHouseKeeper(now)
    }
}
