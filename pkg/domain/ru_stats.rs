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

// 资源组 RU（Request Unit，请求单元）历史统计写入与 GC。
//
// RU 衡量一次请求消耗的标准化资源量。本模块按固定区间（默认 24h）从 PD
// Resource Manager 拉取累计 RU，计算相对上期的增量，写入
// `mysql.request_unit_by_group`，并按保留期批量清理过期行。

// limitations under the License.

// pub const maxRetryCount: i32 = 10;
// pub const ruStatsInterval: time::Duration = 24 * time::Hour;
// 只保留最近 3 个月（最多 92 天）的 request unit 历史行。
// pub const ruStatsGCDuration: time::Duration = 92 * ruStatsInterval;
// pub const gcBatchSize: i64 = 1000;
//
// RUStatsWriter 对应 Go 结构体：负责把 RU 历史数据写入 mysql.request_unit_by_group。
// pub struct RUStatsWriter {
// Go 中这些字段为了单测可见而导出；保留 pub 形状。
//     pub Interval: time::Duration,
//     pub RMClient: pd::ResourceManagerClient,
//     pub InfoCache: *mut infoschema::InfoCache,
//     pub store: kv::Storage,
//     pub sessPool: util::SessionPool,
// 缓存当前时间，方便 Go 单测控制时间点。
//     pub StartTime: time::Time,
// }
//
// NewRUStatsWriter 对应 Go 构造函数：从 Domain 中取 PD client、info cache、store 和系统 session pool。
// pub fn NewRUStatsWriter(do_: &Domain) -> Box<RUStatsWriter> {
//     Box::new(RUStatsWriter {
//         Interval: ruStatsInterval,
//         RMClient: do_.GetPDClient(),
//         InfoCache: do_.infoCache,
//         store: do_.store.clone(),
//         sessPool: do_.sysSessionPool.clone(),
//         StartTime: time::Time::default(),
//     })
// }
//
// impl Domain {
// requestUnitsWriterLoop 对应 Go 后台循环：owner 节点按 interval 写入 RU 历史数据并 GC 旧行。
//     pub fn requestUnitsWriterLoop(&mut self) {
//         if intest::InTest {
// Go 在单测中不启动循环，避免后台任务影响测试时序。
//             return;
//         }
//
//         let mut ruWriter = NewRUStatsWriter(self);
//         loop {
//             let start = time::Now();
//             let mut count = 0;
//             let lastTime = GetLastExpectedTime(start, ruWriter.Interval);
//             if self.DDL().OwnerManager().IsOwner() {
//                 let mut err: Option<errors::Error> = None;
//                 loop {
//                     ruWriter.StartTime = time::Now();
//                     err = ruWriter.DoWriteRUStatistics(context::Background()).err();
//                     if err.is_none() {
//                         break;
//                     }
//                     logutil::BgLogger().Error("failed to insert request_unit_by_group data", zap::Error(err.clone()), zap::Int("retry", count));
//                     count += 1;
//                     if count > maxRetryCount {
//                         break;
//                     }
//                     time::Sleep(time::Second);
//                 }
//
// 写入后尝试清理过期行；失败只记录日志，下一轮继续尝试。
//                 if let Err(gc_err) = ruWriter.GCOutdatedRecords(lastTime) {
//                     logutil::BgLogger().Warn("[ru_stats] gc outdated rowd failed, will try next time.", zap::Error(gc_err));
//                 }
//
//                 logutil::BgLogger().Info(
//                     "[ru_stats] finish write ru historical data",
//                     zap::String("end_time", lastTime.Format(time::DateTime)),
//                     zap::Stringer("interval", ruStatsInterval),
//                     zap::Stringer("cost", time::Since(start)),
//                     zap::Error(err),
//                 );
//             }
//
//             let nextTime = lastTime.Add(ruStatsInterval);
//             let dur = time::Until(nextTime);
//             let timer = time::NewTimer(dur);
// Go select 同时等待 Domain 退出信号和定时器；只保留控制流意图。
//             select! {
//                 _ = self.exit.recv() => return,
//                 _ = timer.C.recv() => {}
//             }
//         }
//     }
// }
//
// GetLastExpectedTime 对应 Go 的同名函数：在本地时区计算最近一次应写入的 RU 结束时间。
// pub fn GetLastExpectedTime(now: time::Time, interval: time::Duration) -> time::Time {
//     GetLastExpectedTimeTZ(now, interval, time::Local)
// }
//
// GetLastExpectedTimeTZ 对应 Go 的测试辅助函数：允许传入指定时区，并用 UTC 计算以兼容 DST。
// pub fn GetLastExpectedTimeTZ(now: time::Time, interval: time::Duration, mut tz: *mut time::Location) -> time::Time {
//     if tz.is_null() {
//         tz = time::Local;
//     }
//     let (year, month, day) = now.Date();
//     let start = time::Date(year, month, day, 0, 0, 0, 0, tz);
// Go 显式转 int64 是为了绕过 durationcheck；这里保留整数计数语义。
//     let count = (now.Sub(start) / interval) as i64;
//     let targetDur = time::Duration(count) * interval;
//     start.In(time::UTC).Add(targetDur).In(tz)
// }
//
// impl RUStatsWriter {
// DoWriteRUStatistics 对应 Go 主流程：去重、读取最新快照、必要时拉取 PD RU stats 并落库。
//     pub fn DoWriteRUStatistics(&mut self, ctx: context::Context) -> Result<(), errors::Error> {
//         let lastEndTime = GetLastExpectedTime(self.StartTime, self.Interval);
//         let isInserted = self.isLatestDataInserted(lastEndTime)?;
//         if isInserted {
//             logutil::BgLogger().Info("[ru_stats] ru data is already inserted, skip", zap::Stringer("end_time", lastEndTime));
//             return Ok(());
//         }
//
//         let lastStats = self.loadLatestRUStats()?;
//         let mut needFetchData = true;
//         if let Some(ref stats) = lastStats {
//             if let Some(ref latest) = stats.Latest {
//                 needFetchData = latest.EndTime != lastEndTime;
//             }
//         }
//
//         let mut ruStats = lastStats;
//         if needFetchData {
//             let stats = self.fetchResourceGroupStats(ctx)?;
//             let mut current = meta::RUStats {
//                 Latest: Some(meta::DailyRUStats {
//                     EndTime: lastEndTime,
//                     Stats: stats,
//                 }),
//                 Previous: None,
//             };
//             if let Some(last) = ruStats {
//                 current.Previous = last.Latest;
//             }
//             self.persistLatestRUStats(&current)?;
//             ruStats = Some(current);
//         }
//
//         self.insertRUStats(ruStats.as_ref())
//     }
//
// fetchResourceGroupStats 对应 Go：从 PD resource manager 拉取 RU，并按 InfoSchema 过滤仍存在的资源组。
//     pub fn fetchResourceGroupStats(&self, ctx: context::Context) -> Result<Vec<meta::GroupRUStats>, errors::Error> {
//         let groups = self.RMClient.ListResourceGroups(ctx, pd::WithRUStats).map_err(errors::Trace)?;
//         let infos = unsafe { (*self.InfoCache).GetLatest() };
//         let mut res = Vec::with_capacity(groups.len());
//         for g in groups {
//             let (groupInfo, exists) = infos.ResourceGroupByName(ast::NewCIStr(&g.Name));
//             if !exists {
//                 continue;
//             }
//             res.push(meta::GroupRUStats {
//                 ID: groupInfo.ID,
//                 Name: groupInfo.Name.O,
//                 RUConsumption: g.RUStats,
//             });
//         }
//         Ok(res)
//     }
//
// loadLatestRUStats 对应 Go：从 KV snapshot 读取保存在 meta 中的最新 RU 统计。
//     pub fn loadLatestRUStats(&self) -> Result<Option<meta::RUStats>, errors::Error> {
//         let snapshot = self.store.GetSnapshot(kv::MaxVersion);
//         let metaStore = meta::NewReader(snapshot);
//         metaStore.GetRUStats()
//     }
//
// persistLatestRUStats 对应 Go：在新事务中写回 meta.RUStats。
//     pub fn persistLatestRUStats(&self, stats: &meta::RUStats) -> Result<(), errors::Error> {
//         let ctx = kv::WithInternalSourceType(context::Background(), kv::InternalTxnOthers);
//         kv::RunInNewTxn(ctx, self.store.clone(), true, |_ctx: context::Context, txn: kv::Transaction| {
//             meta::NewMutator(txn).SetRUStats(stats)
//         })
//     }
//
// isLatestDataInserted 对应 Go 的去重查询：检查目标时间段是否已经存在写入记录。
//     pub fn isLatestDataInserted(&self, lastEndTime: time::Time) -> Result<bool, errors::Error> {
//         let end = lastEndTime.Format(time::DateTime);
//         let start = lastEndTime.Add(-ruStatsInterval).Format(time::DateTime);
//         let rows = runaway::ExecRCRestrictedSQL(
//             self.sessPool.clone(),
//             "SELECT 1 from mysql.request_unit_by_group where start_time = %? and end_time = %? limit 1",
//             vec![start, end],
//         ).map_err(errors::Trace)?;
//         Ok(!rows.is_empty())
//     }
//
// insertRUStats 对应 Go：生成 REPLACE SQL 后通过 restricted SQL 执行。
//     pub fn insertRUStats(&self, stats: Option<&meta::RUStats>) -> Result<(), errors::Error> {
//         let sql = generateSQL(stats);
//         if sql.is_empty() {
//             return Ok(());
//         }
//         runaway::ExecRCRestrictedSQL(self.sessPool.clone(), &sql, None)?;
//         Ok(())
//     }
//
// GCOutdatedRecords 对应 Go：按批删除超过保留期的历史行。
//     pub fn GCOutdatedRecords(&self, lastEndTime: time::Time) -> Result<(), errors::Error> {
//         let gcEndDate = lastEndTime.Add(-ruStatsGCDuration).Format(time::DateTime);
//         let countSQL = format!("SELECT count(*) FROM mysql.request_unit_by_group where end_time <= '{}'", gcEndDate);
//         let rows = runaway::ExecRCRestrictedSQL(self.sessPool.clone(), &countSQL, None).map_err(errors::Trace)?;
//         let totalCount = rows[0].GetInt64(0);
//
//         let loopCount = (totalCount + gcBatchSize - 1) / gcBatchSize;
//         for _ in 0..loopCount {
//             let sql = format!("DELETE FROM mysql.request_unit_by_group where end_time <= '{}' order by end_time limit {}", gcEndDate, gcBatchSize);
//             runaway::ExecRCRestrictedSQL(self.sessPool.clone(), &sql, None).map_err(errors::Trace)?;
//         }
//
//         Ok(())
//     }
// }
//
// generateSQL 对应 Go 的 SQL 拼接函数：根据最新和前一次 RU 累计值计算增量。
// pub fn generateSQL(stats: Option<&meta::RUStats>) -> String {
//     let Some(stats) = stats else {
//         return String::new();
//     };
//
//     let mut buf = String::from("REPLACE INTO mysql.request_unit_by_group(start_time, end_time, resource_group, total_ru) VALUES ");
//     let mut prevStats = std::collections::HashMap::<String, meta::GroupRUStats>::new();
//     if let Some(ref previous) = stats.Previous {
//         for g in &previous.Stats {
//             if g.RUConsumption.is_some() {
//                 prevStats.insert(g.Name.clone(), g.clone());
//             }
//         }
//     }
//
//     let end = stats.Latest.EndTime.Format(time::DateTime);
//     let start = stats.Latest.EndTime.Add(-ruStatsInterval).Format(time::DateTime);
//     let mut count = 0;
//     for g in &stats.Latest.Stats {
//         if g.RUConsumption.is_none() {
//             logutil::BgLogger().Warn("group ru consumption statistics data is empty", zap::String("name", &g.Name), zap::Int64("id", g.ID));
//             continue;
//         }
//         let mut ru = g.RUConsumption.RRU + g.RUConsumption.WRU;
//         if let Some(prev) = prevStats.get(&g.Name) {
//             if prev.RUConsumption.is_some() && g.ID == prev.ID {
//                 ru -= prev.RUConsumption.RRU + prev.RUConsumption.WRU;
//             }
//         }
// 忽略过小增量，保持 Go 中 ru < 1.0 直接跳过的规则。
//         if ru < 1.0 {
//             continue;
//         }
//         if count > 0 {
//             buf.push(',');
//         }
//         let rowData = format!("(\"{}\", \"{}\", \"{}\", {})", start, end, g.Name, ru as i64);
//         buf.push_str(&rowData);
//         count += 1;
//     }
//     if count == 0 {
//         return String::new();
//     }
//     buf.push(';');
//     buf
// }
// */
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
