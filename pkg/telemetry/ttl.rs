// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// TTL（生存时间）遥测：作业开关、删除行/延迟时间直方图。
//
// 从会话上下文汇总配置了 TTL 的表数量，按桶统计昨日删除行数与
// 作业延迟小时数；SQL 常量对应 Go 侧查询 `mysql.tidb_ttl_job_history`。

use crate::SessionContext;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
/// 按表汇总一日删除行数的 SQL（对应 Go 常量）。
pub const selectDeletedRowsOneDaySQL: &str = "SELECT parent_table_id, CAST(SUM(deleted_rows) AS SIGNED)\n\t\t\tFROM\n\t\t\t    mysql.tidb_ttl_job_history\n\t\t\tWHERE\n\t\t\t\tstatus != 'running'\n\t\t\t    AND create_time >= CURDATE() - INTERVAL 7 DAY\n\t\t\t    AND finish_time >= CURDATE() - INTERVAL 1 DAY\n\t\t\t    AND finish_time < CURDATE()\n\t\t\tGROUP BY parent_table_id;";
/// 查询 TTL 作业延迟（分钟）的 SQL。
pub const selectDelaySQL: &str = "SELECT\n\t\t\tparent_table_id, TIMESTAMPDIFF(MINUTE, MIN(tm), CURDATE()) AS ttl_minutes\n\t\t\tFROM\n\t\t\t\t(\n\t\t\t\t\tSELECT\n\t\t\t\t\t\ttable_id,\n\t\t\t\t\t\tparent_table_id,\n\t\t\t\t\t\tMAX(ttl_expire) AS tm\n\t\t\t\t\tFROM\n\t\t\t\t\t\tmysql.tidb_ttl_job_history\n\t\t\t\t\tWHERE\n\t\t\t\t\t\tcreate_time > CURDATE() - INTERVAL 7 DAY\n\t\t\t\t\t\tAND finish_time < CURDATE()\n\t\t\t\t\t\tAND status = 'finished'\n\t\t\t\t\t\tAND JSON_VALID(summary_text)\n\t\t\t\t\t\tAND summary_text ->> \"$.scan_task_err\" IS NULL\n\t\t\t\t\tGROUP BY\n\t\t\t\t\t\ttable_id, parent_table_id\n\t\t\t\t) t\n\t\t\tGROUP BY parent_table_id;";
/// 进程级 TTL 作业是否启用。
static TTL_JOB_ENABLED: AtomicBool = AtomicBool::new(false);
/// 设置 TTL 作业启用标志。
pub fn SetTTLJobEnabled(v: bool) {
    TTL_JOB_ENABLED.store(v, Ordering::Release)
}
/// 直方图单个桶：上界、是否为 +∞ 桶、落入计数。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ttlHistItem {
    /// 桶上界（不含）；`None` 时配合 `LessThanMax`。
    pub LessThan: Option<i64>,
    /// 是否为“大于等于最大上界”的溢出桶。
    pub LessThanMax: bool,
    /// 落入该桶的样本计数。
    pub Count: i64,
}
/// TTL 用量计数器：表规模与两套直方图。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ttlUsageCounter {
    /// TTL 作业全局是否开启。
    pub TTLJobEnabled: bool,
    /// 配置了 TTL 的 public 表数量。
    pub TTLTables: i64,
    /// 其中 TTL 已启用的表数量。
    pub TTLJobEnabledTables: i64,
    /// 直方图对应日期（昨日，`YYYY-MM-DD`）。
    pub TTLHistDate: String,
    /// 按删除行数分桶的直方图。
    pub TableHistWithDeleteRows: Vec<ttlHistItem>,
    /// 按延迟小时分桶的直方图。
    pub TableHistWithDelayTime: Vec<ttlHistItem>,
}
/// 对应 Go 的 `int64` 指针包装：返回 `Some(value)`。
pub fn int64Pointer(value: i64) -> Option<i64> {
    Some(value)
}
impl ttlUsageCounter {
    /// 将删除行数落入第一个满足上界的桶并自增 Count。
    pub fn UpdateTableHistWithDeleteRows(&mut self, rows: i64) {
        for item in &mut self.TableHistWithDeleteRows {
            if item.LessThanMax || item.LessThan.is_some_and(|v| rows < v) {
                item.Count += 1;
                return;
            }
        }
    }
    /// 将延迟小时数按 `count` 增量落入对应桶。
    pub fn UpdateTableHistWithDelayTime(&mut self, count: i32, hours: i64) {
        for item in &mut self.TableHistWithDelayTime {
            if item.LessThanMax || item.LessThan.is_some_and(|v| hours < v) {
                item.Count += count as i64;
                return;
            }
        }
    }
}
/// 计算“昨天”的公历日期字符串（Unix 日序 + Howard 算法变体）。
fn yesterday() -> String {
    // 自 epoch 起的整天数减一，得到昨日日序。
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86400
        - 1;
    // 将日序转为公历年月日（与 Go 侧 civil 日期算法对齐）。
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    format!("{:04}-{:02}-{:02}", y + (m <= 2) as i64, m, d)
}
/// 从会话上下文汇总 TTL 用量：表计数与删除/延迟直方图。
pub fn getTTLUsageInfo(ctx: &SessionContext) -> ttlUsageCounter {
    // 初始化固定桶边界：删除行 1e4…1e7/+∞，延迟 1/6/24/72/+∞ 小时。
    let mut counter = ttlUsageCounter {
        TTLJobEnabled: TTL_JOB_ENABLED.load(Ordering::Acquire),
        TTLHistDate: yesterday(),
        TableHistWithDeleteRows: [10_000, 100_000, 1_000_000, 10_000_000]
            .into_iter()
            .map(|v| ttlHistItem {
                LessThan: Some(v),
                ..Default::default()
            })
            .chain(std::iter::once(ttlHistItem {
                LessThanMax: true,
                ..Default::default()
            }))
            .collect(),
        TableHistWithDelayTime: [1, 6, 24, 72]
            .into_iter()
            .map(|v| ttlHistItem {
                LessThan: Some(v),
                ..Default::default()
            })
            .chain(std::iter::once(ttlHistItem {
                LessThanMax: true,
                ..Default::default()
            }))
            .collect(),
        ..Default::default()
    };
    // 统计 public 且配置了 TTL 的表。
    let mut tables = HashSet::new();
    for schema in &ctx.Schemas {
        for table in &schema.Tables {
            if table.Public && table.TTLEnabled.is_some() {
                counter.TTLTables += 1;
                if table.TTLEnabled == Some(true) {
                    counter.TTLJobEnabledTables += 1
                }
                tables.insert(table.ID);
            }
        }
    }
    // 注入失败时跳过删除行直方图更新。
    if !ctx.FailTTLDeletedQuery {
        for rows in &ctx.TTLDeletedRows {
            counter.UpdateTableHistWithDeleteRows(*rows)
        }
    }
    // 有历史的表按实际延迟入桶；其余记入最大延迟桶。
    if !ctx.FailTTLDelayQuery {
        let mut history = HashSet::new();
        for (id, hours) in &ctx.TTLDelayHours {
            if tables.contains(id) {
                history.insert(*id);
                counter.UpdateTableHistWithDelayTime(1, *hours)
            }
        }
        counter.UpdateTableHistWithDelayTime((tables.len() - history.len()) as i32, i64::MAX)
    }
    counter
}
