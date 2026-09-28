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

// 统计信息读写门面与共享存储抽象。
//
// 定义 `SqlStore`（执行系统表 SQL）、表/列统计内存结构，以及 `StatsReadWriter`：
// 封装 ANALYZE 结果保存、元数据版本更新、按快照加载表统计，并维护内存缓存。

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Eq, PartialEq)]
/// 存储层错误包装，持有可读错误信息。
pub struct Error(pub String);

/// 将内部字符串原样输出为 Display。
impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Debug, PartialEq)]
/// SQL 查询结果单元格取值（对应简化后的 datum）。
pub enum Value {
    Null,
    Int(i64),
    UInt(u64),
    Float(f64),
    Bytes(Vec<u8>),
    Text(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 一行查询结果；按列下标提取整型/字节/文本。
pub struct Row(pub Vec<Value>);

impl Row {
    /// 读取第 index 列为有符号整数；类型不匹配时返回 0。
    pub fn int(&self, index: usize) -> i64 {
        match self.0.get(index) {
            Some(Value::Int(value)) => *value,
            Some(Value::UInt(value)) => *value as i64,
            _ => 0,
        }
    }
    /// 读取第 index 列为无符号整数；类型不匹配时返回 0。
    pub fn uint(&self, index: usize) -> u64 {
        match self.0.get(index) {
            Some(Value::UInt(value)) => *value,
            Some(Value::Int(value)) => *value as u64,
            _ => 0,
        }
    }
    /// 读取第 index 列为字节；Text 按 UTF-8 字节返回。
    pub fn bytes(&self, index: usize) -> Vec<u8> {
        match self.0.get(index) {
            Some(Value::Bytes(value)) => value.clone(),
            Some(Value::Text(value)) => value.as_bytes().to_vec(),
            _ => Vec::new(),
        }
    }
    /// 将字节列按有损 UTF-8 解码为字符串。
    pub fn text(&self, index: usize) -> String {
        String::from_utf8_lossy(&self.bytes(index)).into_owned()
    }
}

/// 统计系统表访问抽象：执行 SQL，并提供 GC 相关默认实现。
/// GC（Garbage Collection）清理过期或已删除对象的统计元数据。
pub trait SqlStore: Send + Sync {
    /// 返回当前事务/会话 start_ts，用作统计版本号。
    fn start_ts(&self) -> Result<u64, Error>;
    /// 执行 SQL 并返回结果行（写语句可返回空）。
    fn execute(&self, sql: &str) -> Result<Vec<Row>, Error>;

    /// 列出 version 落在 [min, max) 的表 ID，供 GC 扫描。
    fn gc_meta_ids(&self, minimum_version: u64, maximum_version: u64) -> Result<Vec<i64>, Error> {
        self.execute(&format!(
            "select table_id from mysql.stats_meta where version >= {minimum_version} and version < {maximum_version}"
        ))
        .map(|rows| rows.into_iter().map(|row| row.int(0)).collect())
    }

    /// 列出表下全部直方图身份：(是否索引, hist_id)。
    fn gc_histogram_identities(&self, table_id: i64) -> Result<Vec<(bool, i64)>, Error> {
        let mut identities = self
            .execute(&format!(
                "select is_index,hist_id from mysql.stats_histograms where table_id={table_id}"
            ))?
            .into_iter()
            .map(|row| (row.int(0) == 1, row.int(1)))
            .collect::<Vec<_>>();
        // DDL implementations may remove the histogram row eagerly while its
        // FM sketch still awaits statistics GC. Include those orphan identities
        // so the same schema-existence check can reclaim every payload.
        identities.extend(
            self.execute(&format!(
                "select is_index,hist_id from mysql.stats_fm_sketch where table_id={table_id}"
            ))?
            .into_iter()
            .map(|row| (row.int(0) == 1, row.int(1))),
        );
        identities.sort_unstable();
        identities.dedup();
        Ok(identities)
    }

    /// 删除表级统计：soft 仅清零直方图数值，hard 则删除直方图行及附属表。
    fn gc_delete_table_stats(&self, table_id: i64, soft: bool, version: u64) -> Result<(), Error> {
        self.execute(&format!("update mysql.stats_meta set version={version},last_stats_histograms_version={version} where table_id={table_id}"))?;
        // soft：保留 hist 行但清零；hard：删除 hist 及 buckets/top_n 等。
        if soft {
            self.execute(&format!("update mysql.stats_histograms set distinct_count=0,null_count=0,tot_col_size=0,modify_count=0,version={version},cm_sketch=null,stats_ver=0,flag=0,correlation=0,last_analyze_pos=null where table_id={table_id}"))?;
        } else {
            self.execute(&format!(
                "delete from mysql.stats_histograms where table_id={table_id}"
            ))?;
        }
        for table in [
            "stats_buckets",
            "stats_top_n",
            "stats_fm_sketch",
            "column_stats_usage",
            "analyze_options",
        ] {
            self.execute(&format!(
                "delete from mysql.{table} where table_id={table_id}"
            ))?;
        }
        self.execute(&format!(
            "delete from mysql.stats_table_locked where table_id={table_id}"
        ))?;
        Ok(())
    }

    /// 删除 `stats_meta` 中指定表的元数据行。
    fn gc_delete_meta(&self, table_id: i64) -> Result<(), Error> {
        self.execute(&format!(
            "delete from mysql.stats_meta where table_id={table_id}"
        ))?;
        Ok(())
    }

    /// 删除单个直方图及其 buckets/TopN/FMSketch，并刷新 meta 版本。
    fn gc_delete_histogram(
        &self,
        table_id: i64,
        histogram_id: i64,
        is_index: bool,
        version: u64,
    ) -> Result<(), Error> {
        self.execute(&format!("update mysql.stats_meta set version={version},last_stats_histograms_version={version} where table_id={table_id}"))?;
        for table in [
            "stats_histograms",
            "stats_top_n",
            "stats_buckets",
            "stats_fm_sketch",
        ] {
            self.execute(&format!(
                "delete from mysql.{table} where table_id={table_id} and hist_id={histogram_id} and is_index={}",
                i32::from(is_index)
            ))?;
        }
        if !is_index {
            self.execute(&format!(
                "delete from mysql.column_stats_usage where table_id={table_id} and column_id={histogram_id}"
            ))?;
        }
        Ok(())
    }

    /// 删除表的历史统计（stats_history / stats_meta_history）。
    fn gc_delete_history(&self, table_id: i64) -> Result<(), Error> {
        self.execute(&format!(
            "delete from mysql.stats_history where table_id={table_id}"
        ))?;
        self.execute(&format!(
            "delete from mysql.stats_meta_history where table_id={table_id}"
        ))?;
        Ok(())
    }

    /// 按保留窗口分批删除过期历史统计，避免长事务。
    fn gc_clear_expired_history(&self, retention_seconds: u64) -> Result<(), Error> {
        let rows = self.execute(&format!("select count(*) from mysql.stats_meta_history use index (idx_create_time) where create_time <= NOW() - INTERVAL {retention_seconds} SECOND"))?;
        let count = rows.first().map_or(0, |row| row.int(0));
        for _ in 0..crate::batch_count(count, 1000) {
            self.execute(&format!("delete from mysql.stats_meta_history use index (idx_create_time) where create_time <= NOW() - INTERVAL {retention_seconds} SECOND limit 1000"))?;
        }
        for _ in 0..crate::batch_count(count, 50) {
            self.execute(&format!("delete from mysql.stats_history use index (idx_create_time) where create_time <= NOW() - INTERVAL {retention_seconds} SECOND limit 50"))?;
        }
        Ok(())
    }

    /// 读取 `mysql.tidb` 中记录的 GC 进度时间戳变量。
    fn gc_timestamp(&self, variable_name: &str) -> Result<Option<String>, Error> {
        self.execute(&format!(
            "select high_priority variable_value from mysql.tidb where variable_name='{variable_name}'"
        ))
        .map(|rows| rows.first().map(|row| row.text(0).to_owned()))
    }

    /// 写入/更新 GC 进度时间戳（UPSERT）。
    fn set_gc_timestamp(&self, variable_name: &str, timestamp: u64) -> Result<(), Error> {
        self.execute(&format!("insert into mysql.tidb (variable_name,variable_value) values ('{variable_name}','{timestamp}') on duplicate key update variable_value='{timestamp}'"))?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 直方图单个桶：累计/重复次数、上下界与桶内 NDV。
pub struct Bucket {
    pub count: i64,
    pub repeat: i64,
    pub lower: Vec<u8>,
    pub upper: Vec<u8>,
    pub ndv: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 列或索引的直方图元数据及桶列表。
/// NDV（Number of Distinct Values）为近似不重复值个数。
pub struct Histogram {
    pub id: i64,
    pub ndv: i64,
    pub null_count: i64,
    pub last_update_version: u64,
    pub total_column_size: i64,
    pub correlation: f64,
    pub buckets: Vec<Bucket>,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// TopN 中的单个高频值及其出现次数。
pub struct TopNItem {
    pub encoded: Vec<u8>,
    pub count: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 单列（或索引）完整统计：直方图、草图、TopN 与版本。
pub struct ColumnStats {
    pub name: String,
    pub histogram: Histogram,
    pub cmsketch: Option<Vec<u8>>,
    pub top_n: Vec<TopNItem>,
    pub fm_sketch: Option<Vec<u8>>,
    pub stats_version: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 物理表级统计缓存：行数、修改计数、列/索引映射。
pub struct TableStats {
    pub physical_id: i64,
    pub count: i64,
    pub modify_count: i64,
    pub version: u64,
    pub stats_version: i64,
    pub columns: HashMap<String, ColumnStats>,
    pub indices: HashMap<String, ColumnStats>,
}

#[derive(Clone, Debug, Default)]
/// 一次 ANALYZE 产出的待持久化结果集合。
pub struct AnalyzeResults {
    pub table_id: i64,
    pub snapshot: u64,
    pub count: i64,
    pub base_count: i64,
    pub base_modify_count: i64,
    pub stats_version: i64,
    pub for_mv_or_global_index: bool,
    pub columns: Vec<(bool, ColumnStats)>,
}

/// 上层 Handle 依赖：提供 SqlStore、租约与历史元数据记录回调。
/// Lease（租约）控制统计缓存刷新与同步加载超时相关节奏。
pub trait StatsHandler: Send + Sync {
    fn store(&self) -> Arc<dyn SqlStore>;
    fn lease(&self) -> Duration;
    fn record_historical_stats_meta(&self, version: u64, source: &str, analyze: bool, id: i64);
}

/// 统计读写器：委托存储层函数，并缓存已加载的 TableStats。
pub struct StatsReadWriter {
    handler: Arc<dyn StatsHandler>,
    cached_tables: Mutex<HashMap<i64, TableStats>>,
}

/// 构造空缓存的 StatsReadWriter。
pub fn new_stats_read_writer(handler: Arc<dyn StatsHandler>) -> StatsReadWriter {
    StatsReadWriter {
        handler,
        cached_tables: Mutex::new(HashMap::new()),
    }
}

impl StatsReadWriter {
    /// 将全局统计各系统表中的 table_id 从 from 改到 to（分区/表 ID 变更）。
    pub fn change_global_stats_id(&self, from: i64, to: i64) -> Result<(), Error> {
        crate::change_global_stats_id(self.handler.store().as_ref(), from, to)
    }

    /// 刷新 meta 版本供 GC 识别，并可选记录历史元数据（schema_change）。
    pub fn update_stats_meta_version_for_gc(&self, physical_id: i64) -> Result<(), Error> {
        let version = crate::update_stats_meta_version_and_last_histogram_version(
            self.handler.store().as_ref(),
            physical_id,
        )?;
        if version != 0 {
            self.handler
                .record_historical_stats_meta(version, "schema_change", false, physical_id);
        }
        Ok(())
    }

    /// 保存 ANALYZE 结果；若耗时超过租约一半则再 bump 版本，最后记历史。
    pub fn save_analyze_result(
        &self,
        results: &AnalyzeResults,
        analyze_snapshot: bool,
        source: &str,
    ) -> Result<(), Error> {
        let started = Instant::now();
        let mut version = crate::save_analyze_result_to_storage(
            self.handler.store().as_ref(),
            results,
            analyze_snapshot,
        )?;
        // 写入过慢时重新推进 version，避免租约窗口内版本落后。
        if version != 0
            && self.handler.lease() > Duration::ZERO
            && started.elapsed() >= self.handler.lease() / 2
        {
            version = crate::update_stats_meta_version_and_last_histogram_version(
                self.handler.store().as_ref(),
                results.table_id,
            )
            .map_err(|_| {
                Error(
                    "failed to update stats meta version during analyze result save. The system may be too busy. Please retry the operation later"
                        .to_owned(),
                )
            })?;
        }
        if version != 0 {
            self.handler
                .record_historical_stats_meta(version, source, true, results.table_id);
        }
        Ok(())
    }

    /// 读取表的 count 与 modify_count。
    pub fn stats_meta_count_and_modify_count(&self, table_id: i64) -> Result<(i64, i64), Error> {
        let (count, modify, _) = crate::stats_meta_count_and_modify_count(
            self.handler.store().as_ref(),
            table_id,
            false,
        )?;
        Ok((count, modify))
    }

    /// 从存储加载表统计；命中缓存则作为增量加载基准，结果写回缓存。
    pub fn table_stats_from_storage(
        &self,
        table_id: i64,
        snapshot: u64,
    ) -> Result<TableStats, Error> {
        let cached = self
            .cached_tables
            .lock()
            .expect("stats cache mutex poisoned")
            .get(&table_id)
            .cloned();
        let table = crate::table_stats_from_storage(
            self.handler.store().as_ref(),
            table_id,
            snapshot,
            cached,
        )?;
        self.cached_tables
            .lock()
            .expect("stats cache mutex poisoned")
            .insert(table_id, table.clone());
        Ok(table)
    }
}

/// 将字节编码为 SQL 十六进制字面量 `x'..'`。
pub(crate) fn sql_bytes(value: &[u8]) -> String {
    let mut output = String::from("x'");
    for byte in value {
        output.push_str(&format!("{byte:02x}"));
    }
    output.push('\'');
    output
}
