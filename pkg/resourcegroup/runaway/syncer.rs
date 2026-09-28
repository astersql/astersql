// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// Runaway 系统表同步器：增量扫描 watch / watch_done。
//
// `Syncer` 维护两个 `SystemTableReader` 游标：
// - `new_watch_reader`：按 `start_time` 扫描 `mysql.tidb_runaway_watch`；
// - `deletion_watch_reader`：按 `done_time` 扫描 `mysql.tidb_runaway_watch_done`。
//
// 扫描窗口为 `[check_point, upper_bound)`，满批时推进到最后一行键时间，
// 否则回退到 `upper_bound - overlap`，用重叠窗口避免边界漏读。

use std::sync::Arc;

use crate::record::{
    QuarantineRecord, RUNAWAY_WATCH_DONE_FULL_TABLE_NAME, RUNAWAY_WATCH_FULL_TABLE_NAME, SqlValue,
};
use crate::{ExecutorRef, Result, RunawayAction, RunawayWatchType, Timestamp, nowMicros};

/// 同步周期（微秒），默认 1 秒。
pub const WATCH_SYNC_INTERVAL_MICROS: i64 = 1_000_000;
/// 扫描重叠窗口（微秒），约为 3 个同步周期。
pub const WATCH_SYNC_OVERLAP_MICROS: i64 = 3 * WATCH_SYNC_INTERVAL_MICROS;
/// 单次扫描行数上限。
pub const WATCH_SYNC_BATCH_LIMIT: usize = 2048;

/// 一行查询结果，元素为按列序排列的 `SqlValue`。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SqlRow(pub Vec<SqlValue>);
impl SqlRow {
    /// 按列下标读取整数。
    fn int(&self, index: usize) -> Option<i64> {
        match self.0.get(index)? {
            SqlValue::Int(v) => Some(*v),
            SqlValue::UInt(v) => i64::try_from(*v).ok(),
            _ => None,
        }
    }
    /// 按列下标读取文本。
    fn text(&self, index: usize) -> Option<String> {
        match self.0.get(index)? {
            SqlValue::Text(v) => Some(v.clone()),
            _ => None,
        }
    }
    /// 按列下标读取时间戳。
    fn time(&self, index: usize) -> Option<Timestamp> {
        match self.0.get(index)? {
            SqlValue::Time(v) => Some(*v),
            _ => None,
        }
    }
    /// 可空时间：NULL 映射为 0（表示不过期）。
    fn nullable_time(&self, index: usize) -> Option<Timestamp> {
        match self.0.get(index)? {
            SqlValue::Null => Some(0),
            SqlValue::Time(v) => Some(*v),
            _ => None,
        }
    }
}

/// quarantine 记录各字段在结果行中的列下标。
#[derive(Clone, Copy)]
pub struct QuarantineColumns {
    id: usize,
    resource_group_name: usize,
    start_time: usize,
    end_time: usize,
    watch: usize,
    watch_text: usize,
    source: usize,
    action: usize,
    switch_group_name: usize,
    exceed_cause: usize,
}
/// `tidb_runaway_watch` 表的列布局。
pub const WATCH_RECORD_COLUMNS: QuarantineColumns = QuarantineColumns {
    id: 0,
    resource_group_name: 1,
    start_time: 2,
    end_time: 3,
    watch: 4,
    watch_text: 5,
    source: 6,
    action: 7,
    switch_group_name: 8,
    exceed_cause: 9,
};
/// `tidb_runaway_watch_done` 表的列布局（多一列 done 主键，原 ID 从 1 起）。
pub const WATCH_DONE_RECORD_COLUMNS: QuarantineColumns = QuarantineColumns {
    id: 1,
    resource_group_name: 2,
    start_time: 3,
    end_time: 4,
    watch: 5,
    watch_text: 6,
    source: 7,
    action: 8,
    switch_group_name: 9,
    exceed_cause: 10,
};

/// 探测系统表是否存在的目录接口。
pub trait SystemTableCatalog: Send + Sync {
    /// `schema.table` 是否存在。
    fn TableExists(&self, schema: &str, table: &str) -> bool;
}
/// 假定所有系统表均存在的默认目录。
#[derive(Default)]
pub struct AllSystemTables;
impl SystemTableCatalog for AllSystemTables {
    fn TableExists(&self, _schema: &str, _table: &str) -> bool {
        true
    }
}

/// 单表增量扫描读取引擎：缓存 SQL 模板与游标状态。
pub struct SystemTableReader {
    /// 全表名。
    pub table_name: String,
    /// 用作扫描键的列名（如 `start_time` / `done_time`）。
    pub key_col: String,
    /// 扫描键在结果行中的列下标。
    pub key_col_idx: usize,
    columns: QuarantineColumns,
    /// 窗口下界（含）。
    pub check_point: Timestamp,
    /// 窗口上界（不含）。
    pub upper_bound: Timestamp,
    select_by_id_sql: String,
    select_by_group_sql: String,
    select_window_sql: String,
    /// 本轮扫描最后一行有效键时间。
    last_scan_key_time: Timestamp,
}
impl SystemTableReader {
    /// 构造读取引擎并预生成三类 SELECT。
    pub fn new(table: &str, key: &str, key_col_idx: usize, columns: QuarantineColumns) -> Self {
        Self {
            table_name: table.into(),
            key_col: key.into(),
            key_col_idx,
            columns,
            check_point: 0,
            upper_bound: 0,
            select_by_id_sql: format!("select * from {table} where id = %?"),
            select_by_group_sql: format!("select * from {table} where resource_group_name = %?"),
            select_window_sql: format!(
                "select * from {table} where {key} >= %? and {key} < %? order by {key} limit %?"
            ),
            last_scan_key_time: 0,
        }
    }
    /// 按主键点查。
    pub fn genSelectByIDStmt(&self, id: i64) -> (String, Vec<SqlValue>) {
        (self.select_by_id_sql.clone(), vec![SqlValue::Int(id)])
    }
    /// 按资源组名查询。
    pub fn genSelectByGroupStmt(&self, group: &str) -> (String, Vec<SqlValue>) {
        (
            self.select_by_group_sql.clone(),
            vec![SqlValue::Text(group.into())],
        )
    }
    /// 按当前游标窗口扫描。
    pub fn genSelectStmt(&self) -> (String, Vec<SqlValue>) {
        (
            self.select_window_sql.clone(),
            vec![
                SqlValue::Time(self.check_point),
                SqlValue::Time(self.upper_bound),
                SqlValue::Int(WATCH_SYNC_BATCH_LIMIT as i64),
            ],
        )
    }
}

/// 同时驱动 watch 新增与 done 删除两条扫描路径的同步器。
pub struct Syncer {
    /// 扫描新增监视。
    pub new_watch_reader: SystemTableReader,
    /// 扫描已完成（待从本地移除）的监视。
    pub deletion_watch_reader: SystemTableReader,
    executor: ExecutorRef,
    catalog: Arc<dyn SystemTableCatalog>,
    /// 最近一次同步时间戳。
    pub last_sync_time: Timestamp,
}
impl Syncer {
    /// 绑定执行器与系统表目录，初始化两个 reader。
    pub fn new(executor: ExecutorRef, catalog: Arc<dyn SystemTableCatalog>) -> Self {
        Self {
            new_watch_reader: SystemTableReader::new(
                RUNAWAY_WATCH_FULL_TABLE_NAME,
                "start_time",
                2,
                WATCH_RECORD_COLUMNS,
            ),
            deletion_watch_reader: SystemTableReader::new(
                RUNAWAY_WATCH_DONE_FULL_TABLE_NAME,
                "done_time",
                11,
                WATCH_DONE_RECORD_COLUMNS,
            ),
            executor,
            catalog,
            last_sync_time: 0,
        }
    }
    /// watch 表是否存在。
    pub fn checkWatchTableExist(&self) -> bool {
        self.catalog.TableExists("mysql", "tidb_runaway_watch")
    }
    /// watch_done 表是否存在。
    pub fn checkWatchDoneTableExist(&self) -> bool {
        self.catalog.TableExists("mysql", "tidb_runaway_watch_done")
    }
    /// 按 ID 点查监视记录（不推进扫描游标）。
    pub fn getWatchRecordByID(&self, id: i64) -> Result<Vec<QuarantineRecord>> {
        let stmt = self.new_watch_reader.genSelectByIDStmt(id);
        self.readQuarantineRecords(&self.new_watch_reader, stmt)
    }
    /// 按资源组点查监视记录（不推进扫描游标）。
    pub fn getWatchRecordByGroup(&self, group: &str) -> Result<Vec<QuarantineRecord>> {
        let stmt = self.new_watch_reader.genSelectByGroupStmt(group);
        self.readQuarantineRecords(&self.new_watch_reader, stmt)
    }
    /// 增量拉取新增 watch。
    pub fn getNewWatchRecords(&mut self) -> Result<Vec<QuarantineRecord>> {
        Self::scan(&self.executor, &mut self.new_watch_reader)
    }
    /// 增量拉取 watch_done。
    pub fn getNewWatchDoneRecords(&mut self) -> Result<Vec<QuarantineRecord>> {
        Self::scan(&self.executor, &mut self.deletion_watch_reader)
    }
    /// 执行窗口扫描并按批大小推进 checkpoint。
    fn scan(
        executor: &ExecutorRef,
        reader: &mut SystemTableReader,
    ) -> Result<Vec<QuarantineRecord>> {
        reader.upper_bound = nowMicros();
        let (sql, params) = reader.genSelectStmt();
        let rows = executor.Execute(&sql, &params)?;
        let mut records = Vec::with_capacity(rows.len());
        for row in rows {
            let key_time = row.time(reader.key_col_idx);
            if let Some(record) = decodeQuarantineRecord(&row, reader.columns) {
                if let Some(key_time) = key_time {
                    reader.last_scan_key_time = key_time;
                }
                records.push(record);
            }
        }
        // 满批：推进到最后有效键时间；若键未前进则回退到 overlap，防活锁。
        if records.len() >= WATCH_SYNC_BATCH_LIMIT {
            reader.check_point = if reader.last_scan_key_time > reader.check_point {
                reader.last_scan_key_time
            } else {
                reader.upper_bound - WATCH_SYNC_OVERLAP_MICROS
            };
        } else if !records.is_empty() {
            // 部分批次：用上界减 overlap，覆盖边界附近新写入。
            reader.check_point = reader.upper_bound - WATCH_SYNC_OVERLAP_MICROS;
        }
        Ok(records)
    }
    /// 执行点查类 SELECT 并解码，不修改游标。
    fn readQuarantineRecords(
        &self,
        reader: &SystemTableReader,
        stmt: (String, Vec<SqlValue>),
    ) -> Result<Vec<QuarantineRecord>> {
        Ok(self
            .executor
            .Execute(&stmt.0, &stmt.1)?
            .iter()
            .filter_map(|row| decodeQuarantineRecord(row, reader.columns))
            .collect())
    }
}

/// 将结果行解码为 `QuarantineRecord`；列类型不符时返回 None。
pub fn decodeQuarantineRecord(row: &SqlRow, cols: QuarantineColumns) -> Option<QuarantineRecord> {
    Some(QuarantineRecord {
        ID: row.int(cols.id)?,
        ResourceGroupName: row.text(cols.resource_group_name)?,
        StartTime: row.time(cols.start_time)?,
        EndTime: row.nullable_time(cols.end_time)?,
        Watch: match row.int(cols.watch)? {
            1 => RunawayWatchType::Exact,
            2 => RunawayWatchType::Similar,
            3 => RunawayWatchType::Plan,
            _ => RunawayWatchType::None,
        },
        WatchText: row.text(cols.watch_text)?,
        Source: row.text(cols.source)?,
        ExceedCause: row.text(cols.exceed_cause)?,
        Action: match row.int(cols.action)? {
            1 => RunawayAction::DryRun,
            2 => RunawayAction::CoolDown,
            3 => RunawayAction::Kill,
            4 => RunawayAction::SwitchGroup,
            _ => RunawayAction::NoneAction,
        },
        SwitchGroupName: row.text(cols.switch_group_name)?,
    })
}
