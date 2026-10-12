// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 资源组 RU（Request Unit，请求单元）历史统计写入与 GC。
//
// RU 衡量一次请求消耗的标准化资源量。本模块按固定区间（默认 24h）从 PD
// Resource Manager 拉取累计 RU，计算相对上期的增量，写入
// `mysql.request_unit_by_group`，并按保留期批量清理过期行。

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 默认写入间隔：24 小时。
pub const RU_STATS_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// GC 保留窗口：约 3 个月（92 天）。
pub const RU_STATS_GC_DURATION: Duration = Duration::from_secs(92 * 24 * 60 * 60);
/// 每批删除的最大行数。
pub const GC_BATCH_SIZE: usize = 1000;

/// 单个资源组的累计 RU 快照。
#[derive(Clone, Debug, PartialEq)]
pub struct GroupRuStats {
    /// 资源组 ID；增量计算时要求与上期同名同 ID。
    pub id: i64,
    /// 资源组名。
    pub name: String,
    /// 累计读 RU（RRU）。
    pub read_ru: f64,
    /// 累计写 RU（WRU）。
    pub write_ru: f64,
}
/// 某一结束时刻的全部资源组快照。
#[derive(Clone, Debug, PartialEq)]
pub struct DailyRuStats {
    /// 本区间结束时间。
    pub end_time: SystemTime,
    /// 各资源组累计值。
    pub groups: Vec<GroupRuStats>,
}
/// 相邻两期快照：用 latest − previous 得到区间增量。
#[derive(Clone, Debug, PartialEq)]
pub struct RuStats {
    /// 上一期快照；缺失时增量等于最新累计。
    pub previous: Option<DailyRuStats>,
    /// 最新一期快照。
    pub latest: DailyRuStats,
}
/// 待写入 `request_unit_by_group` 的一行增量。
#[derive(Clone, Debug, PartialEq)]
pub struct RuStatsRow {
    /// 区间起点。
    pub start_time: SystemTime,
    /// 区间终点。
    pub end_time: SystemTime,
    /// 资源组名。
    pub resource_group: String,
    /// 区间内总 RU 增量（读+写）。
    pub total_ru: f64,
}

/// RU 统计持久化与拉取后端抽象。
pub trait RuStatsBackend {
    /// 目标时间段是否已写入（去重）。
    fn is_inserted(&self, start: SystemTime, end: SystemTime) -> Result<bool, String>;
    /// 读取 meta 中缓存的最新 RU 统计。
    fn load_latest(&self) -> Result<Option<RuStats>, String>;
    /// 从 PD 拉取当前资源组 RU。
    fn fetch_groups(&self) -> Result<Vec<GroupRuStats>, String>;
    /// 将最新快照写回 meta。
    fn persist_latest(&self, stats: &RuStats) -> Result<(), String>;
    /// 批量插入/替换历史行。
    fn insert_rows(&self, rows: &[RuStatsRow]) -> Result<(), String>;
    /// 删除 end_time ≤ cutoff 的一批行，返回实际删除数。
    fn delete_before(&self, end: SystemTime, batch_size: usize) -> Result<usize, String>;
}

/// 按区间写入 RU 历史并执行 GC 的写入器。
pub struct RuStatsWriter<B> {
    /// 写入间隔，须在 (0, 24h] 内。
    pub interval: Duration,
    /// 用于计算“最近应写入结束时间”的参考时钟。
    pub start_time: SystemTime,
    /// 后端实现。
    pub backend: B,
}

impl<B: RuStatsBackend> RuStatsWriter<B> {
    /// 主写入流程：校验区间 → 去重 → 必要时拉取并持久化快照 → 生成增量行并落库。
    pub fn write(&self) -> Result<Vec<RuStatsRow>, String> {
        if self.interval.is_zero() || self.interval > RU_STATS_INTERVAL {
            return Err("RU interval must be between zero and 24 hours".to_string());
        }
        let end = get_last_expected_time(self.start_time, self.interval)?;
        let start = end
            .checked_sub(self.interval)
            .ok_or_else(|| "RU interval underflow".to_string())?;
        // 已写入则跳过，避免 owner 重复插入。
        if self.backend.is_inserted(start, end)? {
            return Ok(Vec::new());
        }
        let previous_state = self.backend.load_latest()?;
        // 若缓存 latest 已是目标 end，复用；否则拉取新快照并把旧 latest 挪到 previous。
        let stats = match previous_state
            .as_ref()
            .filter(|stats| stats.latest.end_time == end)
            .cloned()
        {
            Some(stats) => stats,
            None => {
                let latest = DailyRuStats {
                    end_time: end,
                    groups: self.backend.fetch_groups()?,
                };
                let stats = RuStats {
                    previous: previous_state.map(|state| state.latest),
                    latest,
                };
                self.backend.persist_latest(&stats)?;
                stats
            }
        };
        let rows = generate_rows(&stats, self.interval);
        if !rows.is_empty() {
            self.backend.insert_rows(&rows)?;
        }
        Ok(rows)
    }

    /// 按保留期批量删除过期行，直到不足一批。
    pub fn gc_outdated_records(&self, end: SystemTime) -> Result<usize, String> {
        let cutoff = end
            .checked_sub(RU_STATS_GC_DURATION)
            .ok_or_else(|| "RU GC cutoff underflow".to_string())?;
        let mut deleted = 0;
        loop {
            let count = self.backend.delete_before(cutoff, GC_BATCH_SIZE)?;
            deleted += count;
            if count < GC_BATCH_SIZE {
                return Ok(deleted);
            }
        }
    }
}

/// 计算不晚于 `now` 的最近对齐结束时间（UNIX 纪元按 interval 取整）。
pub fn get_last_expected_time(now: SystemTime, interval: Duration) -> Result<SystemTime, String> {
    if interval.is_zero() {
        return Err("interval must not be zero".to_string());
    }
    let elapsed = now
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?;
    let seconds = interval.as_secs();
    if seconds == 0 {
        return Err("sub-second RU intervals are unsupported".to_string());
    }
    Ok(UNIX_EPOCH + Duration::from_secs(elapsed.as_secs() / seconds * seconds))
}

/// 用同名同 ID 的上期累计计算增量；增量 < 1 或非有限则跳过。
pub fn generate_rows(stats: &RuStats, interval: Duration) -> Vec<RuStatsRow> {
    let previous: BTreeMap<_, _> = stats
        .previous
        .as_ref()
        .into_iter()
        .flat_map(|day| &day.groups)
        .map(|group| (group.name.as_str(), group))
        .collect();
    stats
        .latest
        .groups
        .iter()
        .filter_map(|group| {
            let current = group.read_ru + group.write_ru;
            let old = previous
                .get(group.name.as_str())
                .filter(|old| old.id == group.id)
                .map(|old| old.read_ru + old.write_ru)
                .unwrap_or(0.0);
            // 对齐 Go：ru < 1.0 的微小增量忽略。
            let delta = current - old;
            if !delta.is_finite() || delta < 1.0 {
                return None;
            }
            Some(RuStatsRow {
                start_time: stats.latest.end_time.checked_sub(interval)?,
                end_time: stats.latest.end_time,
                resource_group: group.name.clone(),
                total_ru: delta,
            })
        })
        .collect()
}

/// 生成 `REPLACE INTO mysql.request_unit_by_group ...` SQL；资源组名做单引号转义。
pub fn generate_sql(rows: &[RuStatsRow]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let values = rows
        .iter()
        .map(|row| {
            let start = row
                .start_time
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let end = row
                .end_time
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            format!(
                "({start},{end},'{}',{})",
                row.resource_group.replace('\'', "''"),
                row.total_ru as i64
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "REPLACE INTO mysql.request_unit_by_group(start_time,end_time,resource_group,total_ru) VALUES {values};"
    )
}
