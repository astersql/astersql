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

// 历史统计（Historical Stats）落盘与元数据记录。
//
// 在启用历史统计后，将表/分区的统计快照按块写入存储，并维护
// `HistoricalStatsMeta`（版本、行数、变更计数、来源）；供计划回放、
// 统计对比等按版本回溯使用。

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// 单块历史统计编码的最大列侧字节上限（5 MiB），用于分块切分。
pub const MAX_COLUMN_SIZE: usize = 5 << 20;

/// `mysql.stats_history.create_time` uses the same microsecond DATETIME text
/// representation as Go's `time.Now().Format("2006-01-02 15:04:05.999999")`.
pub type HistoryTimestamp = String;

/// 历史统计元数据：表 ID、统计版本、变更计数、行数与写入来源。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HistoricalStatsMeta {
    pub table_id: i64,
    pub version: u64,
    pub modify_count: i64,
    pub row_count: i64,
    pub source: String,
}

/// 历史统计路径上的轻量错误包装。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error(pub String);
/// 一次可落盘的历史表统计快照：版本、分区版本列表与整表编码。
#[derive(Clone, Debug, Default)]
pub struct HistoricalTable {
    pub version: u64,
    pub partition_versions: Vec<u64>,
    pub encoded: Vec<u8>,
}

/// 历史统计持久化后端：开关查询、元数据读写与分块插入。
pub trait HistoryStore: Send + Sync {
    /// 是否开启历史统计功能。
    fn historical_enabled(&self) -> Result<bool, Error>;
    /// 按表 ID 与版本读取 `(modify_count, row_count)`；无则 `None`。
    fn stats_meta(&self, table_id: i64, version: u64) -> Result<Option<(i64, i64)>, Error>;
    /// 写入或替换指定版本的元数据历史记录。
    fn replace_meta_history(
        &self,
        table_id: i64,
        modify_count: i64,
        count: i64,
        version: u64,
        source: &str,
    ) -> Result<(), Error>;
    /// 插入一块历史统计二进制数据（带序号与时间戳）。
    fn insert_history_block(
        &self,
        table_id: i64,
        block: &[u8],
        sequence: usize,
        version: u64,
        timestamp: &str,
    ) -> Result<(), Error>;
}
/// 当前内存/缓存中的统计快照导出与分块能力。
pub trait StatsSnapshot: Send + Sync {
    /// 导出指定库表（或分区）的历史表快照；无可用统计则 `None`。
    fn dump_stats(
        &self,
        database: &str,
        table_id: i64,
        is_partition: bool,
    ) -> Result<Option<HistoricalTable>, Error>;
    /// 表统计是否已初始化（未初始化时可跳过元数据记录）。
    fn table_initialized(&self, table_id: i64) -> bool;
    /// 将快照按 `block_size` 切成多块字节序列。
    fn blocks(&self, table: &HistoricalTable, block_size: usize) -> Result<Vec<Vec<u8>>, Error>;
}

/// 历史统计门面：组合 `HistoryStore` 与 `StatsSnapshot` 完成落盘与元数据写入。
pub struct StatsHistory {
    store: Arc<dyn HistoryStore>,
    snapshot: Arc<dyn StatsSnapshot>,
}
impl StatsHistory {
    /// 构造历史统计处理器。
    pub fn new(store: Arc<dyn HistoryStore>, snapshot: Arc<dyn StatsSnapshot>) -> Self {
        Self { store, snapshot }
    }
    /// 导出并分块写入表/分区历史统计；无快照时返回版本 0。
    pub fn record_historical_stats_to_storage(
        &self,
        database: &str,
        table_id: i64,
        is_partition: bool,
    ) -> Result<u64, Error> {
        let Some(table) = self.snapshot.dump_stats(database, table_id, is_partition)? else {
            return Ok(0);
        };
        record_historical_stats_to_storage(
            self.store.as_ref(),
            self.snapshot.as_ref(),
            table_id,
            &table,
        )
    }
    /// 批量为多张表记录历史元数据；`enforce` 为真时跳过“已初始化”检查。
    pub fn record_historical_stats_meta(
        &self,
        version: u64,
        source: &str,
        enforce: bool,
        table_ids: &[i64],
    ) {
        if version == 0 {
            return;
        }
        // 过滤非法 ID，并在非 enforce 时要求表统计已初始化。
        let targets = table_ids
            .iter()
            .copied()
            .filter(|id| *id != 0 && (enforce || self.snapshot.table_initialized(*id)))
            .collect::<Vec<_>>();
        if !self.store.historical_enabled().unwrap_or(false) {
            return;
        }
        for table_id in targets {
            let _ = record_historical_stats_meta(self.store.as_ref(), version, source, table_id);
        }
    }
    /// 查询历史统计功能开关。
    pub fn check_historical_stats_enable(&self) -> Result<bool, Error> {
        self.store.historical_enabled()
    }
}

/// 读取当前版本 meta 并写入历史元数据表；无效 ID/版本或缺失 meta 时报错。
pub fn record_historical_stats_meta(
    store: &dyn HistoryStore,
    version: u64,
    source: &str,
    table_id: i64,
) -> Result<(), Error> {
    if table_id == 0 || version == 0 {
        return Err(Error(format!(
            "tableID {table_id}, version {version} are invalid"
        )));
    }
    let Some((modify_count, count)) = store.stats_meta(table_id, version)? else {
        return Err(Error("no historical meta stats can be recorded".into()));
    };
    store.replace_meta_history(table_id, modify_count, count, version, source)
}

/// 将快照分块后按序号写入存储，版本取分区版本最大值（无分区则用表版本）。
pub fn record_historical_stats_to_storage(
    store: &dyn HistoryStore,
    snapshot: &dyn StatsSnapshot,
    physical_id: i64,
    table: &HistoricalTable,
) -> Result<u64, Error> {
    // 分区表取各分区版本 max，保证历史版本覆盖最新分区统计。
    let version = table
        .partition_versions
        .iter()
        .copied()
        .max()
        .unwrap_or(table.version);
    let blocks = snapshot.blocks(table, MAX_COLUMN_SIZE)?;
    let timestamp = current_history_timestamp();
    for (sequence, block) in blocks.iter().enumerate() {
        if let Err(error) =
            store.insert_history_block(physical_id, block, sequence, version, &timestamp)
        {
            return Err(error);
        }
    }
    Ok(version)
}

/// Format the current UTC time with the precision used by the Go history path.
fn current_history_timestamp() -> HistoryTimestamp {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = elapsed.as_secs();
    let days = (seconds / 86_400) as i64;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_date_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{:06}",
        seconds_of_day / 3_600,
        seconds_of_day / 60 % 60,
        seconds_of_day % 60,
        elapsed.subsec_micros(),
    )
}

/// Convert days since 1970-01-01 to a Gregorian UTC date.
fn civil_date_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    // Howard Hinnant's civil-from-days algorithm, with the Unix epoch offset.
    let shifted = days_since_epoch + 719_468;
    let era = if shifted >= 0 {
        shifted / 146_097
    } else {
        (shifted - 146_096) / 146_097
    };
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = month_part + if month_part < 10 { 3 } else { -9 };
    let year = year + if month <= 2 { 1 } else { 0 };
    (year, month as u32, day as u32)
}
