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
// Copyright 2026 AsterSQL.

// 谓词列统计使用时间的 SQL 读写实现。
//
// 通过受限 SQL 执行器访问系统表 `mysql.column_stats_usage`，加载/保存列的
// 最近使用与分析时间，查询谓词列，并在 InfoSchema（最新表结构元数据）
// 基础上清理已删除列的残留记录。时间戳经会话时区与 UTC 互转。

use chrono::{NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use meta_model::TableItemID;
use stats_types::ColStatsTimeInfo;
use std::collections::HashMap;
use std::fmt;

/// 加载全部列使用时间（时间列已转为 UTC）。
pub const LOAD_ALL_SQL: &str = "SELECT table_id, column_id, CONVERT_TZ(last_used_at, @@TIME_ZONE, '+00:00'), CONVERT_TZ(last_analyzed_at, @@TIME_ZONE, '+00:00') FROM mysql.column_stats_usage";
/// 按表 ID 加载列使用时间。
pub const LOAD_TABLE_SQL: &str = "SELECT table_id, column_id, CONVERT_TZ(last_used_at, @@TIME_ZONE, '+00:00'), CONVERT_TZ(last_analyzed_at, @@TIME_ZONE, '+00:00') FROM mysql.column_stats_usage WHERE table_id = %?";
/// 查询表上 last_used_at 非空的谓词列。
pub const PREDICATE_COLUMNS_SQL: &str = "SELECT column_id, CONVERT_TZ(last_used_at, @@TIME_ZONE, '+00:00') FROM mysql.column_stats_usage WHERE table_id = %? AND last_used_at IS NOT NULL";
/// 删除表中已不在当前列集合内的使用记录。
pub const CLEANUP_DROPPED_COLUMNS_SQL: &str =
    "DELETE FROM mysql.column_stats_usage WHERE table_id = %? AND column_id NOT IN (%?)";
/// 以 REPLACE 写入单行列使用时间（入参为 UTC，存库转会话时区）。
pub const REPLACE_USAGE_SQL: &str = "REPLACE INTO mysql.column_stats_usage (table_id, column_id, last_used_at, last_analyzed_at) VALUES (%?, %?, CONVERT_TZ(%?, '+00:00', @@TIME_ZONE), CONVERT_TZ(%?, '+00:00', @@TIME_ZONE))";

/// 谓词列 SQL 路径上的错误类型。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PredicateColumnError(String);

impl PredicateColumnError {
    /// 由消息构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for PredicateColumnError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PredicateColumnError {}

/// 受限 SQL 绑定参数：整型、字符串列表、时间戳或 NULL。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SqlArg {
    I64(i64),
    StringList(Vec<String>),
    String(String),
}

/// Typed result of the restricted SQL query after SQL has normalized both
/// timestamp columns to UTC. Nullable IDs are retained so the Go skip rule can
/// be enforced in this package instead of hidden in an executor adapter.
/// 受限查询行：时间列已规范为 UTC；可空 ID 保留以便本包执行 Go 的跳过规则。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColumnStatsUsageRecord {
    pub table_id: Option<i64>,
    pub column_id: Option<i64>,
    pub last_used_at_utc: Option<NaiveDateTime>,
    pub last_analyzed_at_utc: Option<NaiveDateTime>,
}

/// 谓词列查询结果行：列 ID 与已转 UTC 的最近使用时间。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PredicateColumnRecord {
    pub column_id: i64,
    pub converted_last_used_at_is_null: bool,
}

/// Narrow adapter over TiDB's restricted SQL executor. The canonical Rust
/// `statistics/handle/util::ExecRows` remains an uncompiled draft, so callers
/// must supply a real implementation; this crate never reports a write as
/// successful without invoking this boundary.
/// 受限 SQL 执行器窄适配：查询使用记录 / 谓词列，以及执行写语句。
pub trait PredicateColumnExecutor: Send + Sync {
    fn query_usage(
        &self,
        sql: &str,
        args: &[SqlArg],
    ) -> Result<Vec<ColumnStatsUsageRecord>, PredicateColumnError>;

    fn query_predicate_columns(
        &self,
        sql: &str,
        args: &[SqlArg],
    ) -> Result<Vec<PredicateColumnRecord>, PredicateColumnError>;

    fn execute(&self, sql: &str, args: &[SqlArg]) -> Result<(), PredicateColumnError>;
}

/// Backing executor that routes every statement through the canonical
/// statistics `ExecRows` helper, including its timeout hook and current-session
/// restricted-SQL option.
pub struct ExecRowsPredicateColumnExecutor<'a> {
    context: &'a dyn stats_types::SessionContext,
}

impl<'a> ExecRowsPredicateColumnExecutor<'a> {
    pub fn new(context: &'a dyn stats_types::SessionContext) -> Self {
        Self { context }
    }

    fn arguments(args: &[SqlArg]) -> Vec<stats_types::StatsSqlValue> {
        args.iter()
            .map(|argument| match argument {
                SqlArg::I64(value) => stats_types::StatsSqlValue::Integer(*value),
                SqlArg::StringList(values) => {
                    stats_types::StatsSqlValue::StringList(values.clone())
                }
                SqlArg::String(value) => stats_types::StatsSqlValue::String(value.clone()),
            })
            .collect()
    }

    fn rows(
        &self,
        sql: &str,
        args: &[SqlArg],
    ) -> Result<Vec<stats_types::StatsRow>, PredicateColumnError> {
        let arguments = Self::arguments(args);
        stats_types::ExecRows(self.context, sql, &arguments)
            .map(|(rows, _)| rows)
            .map_err(|error| PredicateColumnError::new(error.to_string()))
    }
}

fn rowValue<'a>(
    row: &'a stats_types::StatsRow,
    index: usize,
) -> Result<&'a stats_types::StatsSqlValue, PredicateColumnError> {
    row.values
        .get(index)
        .ok_or_else(|| PredicateColumnError::new(format!("SQL row is missing column {index}")))
}

fn nullableI64(
    row: &stats_types::StatsRow,
    index: usize,
) -> Result<Option<i64>, PredicateColumnError> {
    match rowValue(row, index)? {
        stats_types::StatsSqlValue::Null => Ok(None),
        stats_types::StatsSqlValue::Integer(value) => Ok(Some(*value)),
        stats_types::StatsSqlValue::Unsigned(value) => i64::try_from(*value)
            .map(Some)
            .map_err(|_| PredicateColumnError::new(format!("column {index} does not fit i64"))),
        value => Err(PredicateColumnError::new(format!(
            "column {index} is not an integer: {value:?}"
        ))),
    }
}

fn nullableTime(
    row: &stats_types::StatsRow,
    index: usize,
) -> Result<Option<NaiveDateTime>, PredicateColumnError> {
    let value = match rowValue(row, index)? {
        stats_types::StatsSqlValue::Null => return Ok(None),
        stats_types::StatsSqlValue::String(value) => value.as_str(),
        stats_types::StatsSqlValue::Bytes(value) => std::str::from_utf8(value)
            .map_err(|error| PredicateColumnError::new(error.to_string()))?,
        value => {
            return Err(PredicateColumnError::new(format!(
                "column {index} is not a timestamp: {value:?}"
            )));
        }
    };
    NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
        .map(Some)
        .map_err(|error| PredicateColumnError::new(error.to_string()))
}

impl PredicateColumnExecutor for ExecRowsPredicateColumnExecutor<'_> {
    fn query_usage(
        &self,
        sql: &str,
        args: &[SqlArg],
    ) -> Result<Vec<ColumnStatsUsageRecord>, PredicateColumnError> {
        self.rows(sql, args)?
            .into_iter()
            .map(|row| {
                let table_id = nullableI64(&row, 0)?;
                let column_id = nullableI64(&row, 1)?;
                let (last_used_at_utc, last_analyzed_at_utc) =
                    if table_id.is_none() || column_id.is_none() {
                        (None, None)
                    } else {
                        (nullableTime(&row, 2)?, nullableTime(&row, 3)?)
                    };
                Ok(ColumnStatsUsageRecord {
                    table_id,
                    column_id,
                    last_used_at_utc,
                    last_analyzed_at_utc,
                })
            })
            .collect()
    }

    fn query_predicate_columns(
        &self,
        sql: &str,
        args: &[SqlArg],
    ) -> Result<Vec<PredicateColumnRecord>, PredicateColumnError> {
        self.rows(sql, args)?
            .into_iter()
            .map(|row| {
                Ok(PredicateColumnRecord {
                    column_id: nullableI64(&row, 0)?.unwrap_or_default(),
                    converted_last_used_at_is_null: matches!(
                        rowValue(&row, 1)?,
                        stats_types::StatsSqlValue::Null
                    ),
                })
            })
            .collect()
    }

    fn execute(&self, sql: &str, args: &[SqlArg]) -> Result<(), PredicateColumnError> {
        self.rows(sql, args).map(|_| ())
    }
}

/// Narrow read-only view of the latest InfoSchema required for dropped-column
/// cleanup. `None` preserves Go's safe no-op behavior when a table disappears.
/// 最新 InfoSchema 只读视图：返回表当前列 ID；`None` 表示表已消失（清理跳过）。
pub trait PredicateColumnInfoSchema: Send + Sync {
    fn table_column_ids(&self, table_id: i64) -> Option<Vec<i64>>;
}

pub struct ExecRowsPredicateColumnInfoSchema<'a>(&'a dyn stats_types::InfoSchema);

impl PredicateColumnInfoSchema for ExecRowsPredicateColumnInfoSchema<'_> {
    fn table_column_ids(&self, table_id: i64) -> Option<Vec<i64>> {
        self.0.TableByID(table_id).map(|table| {
            table
                .Meta()
                .columns
                .iter()
                .map(|column| column.id)
                .collect()
        })
    }
}

/// Session boundary joining its restricted SQL executor and latest InfoSchema.
/// 会话边界：同时提供执行器与最新 InfoSchema。
pub trait PredicateColumnSession: Send + Sync {
    fn executor(&self) -> &dyn PredicateColumnExecutor;
    fn latest_info_schema(&self) -> &dyn PredicateColumnInfoSchema;
}

/// Production session adapter joining statistics `ExecRows` with the latest
/// canonical InfoSchema.
pub struct ExecRowsPredicateColumnSession<'a> {
    executor: ExecRowsPredicateColumnExecutor<'a>,
    info_schema: ExecRowsPredicateColumnInfoSchema<'a>,
}

impl<'a> ExecRowsPredicateColumnSession<'a> {
    pub fn new(
        context: &'a dyn stats_types::SessionContext,
        info_schema: &'a dyn stats_types::InfoSchema,
    ) -> Self {
        Self {
            executor: ExecRowsPredicateColumnExecutor::new(context),
            info_schema: ExecRowsPredicateColumnInfoSchema(info_schema),
        }
    }
}

impl PredicateColumnSession for ExecRowsPredicateColumnSession<'_> {
    fn executor(&self) -> &dyn PredicateColumnExecutor {
        &self.executor
    }

    fn latest_info_schema(&self) -> &dyn PredicateColumnInfoSchema {
        &self.info_schema
    }
}

/// 将 UTC 朴素时间按给定时区本地化并构造 MySQL TIMESTAMP。
fn localizedTime(value: NaiveDateTime, location: Tz) -> stats_types::Time {
    let utc = Utc.from_utc_datetime(&value);
    let localized = utc.with_timezone(&location);
    stats_types::NewTime(
        stats_types::FromGoTime(localized),
        stats_types::mysql::TypeTimestamp,
        stats_types::DefaultFsp,
    )
}

/// 执行查询并将行转为 `TableItemID → ColStatsTimeInfo`；跳过 ID 为空的行。
fn loadColumnStatsUsage(
    session: &dyn PredicateColumnSession,
    location: Tz,
    query: &str,
    args: &[SqlArg],
) -> Result<HashMap<TableItemID, ColStatsTimeInfo>, PredicateColumnError> {
    let rows = session.executor().query_usage(query, args)?;
    let mut result = HashMap::with_capacity(rows.len());
    for row in rows {
        let (Some(table_id), Some(column_id)) = (row.table_id, row.column_id) else {
            continue;
        };
        result.insert(
            TableItemID {
                TableID: table_id,
                ID: column_id,
                IsIndex: false,
                IsSyncLoadFailed: false,
            },
            ColStatsTimeInfo {
                LastUsedAt: row
                    .last_used_at_utc
                    .map(|value| localizedTime(value, location)),
                LastAnalyzedAt: row
                    .last_analyzed_at_utc
                    .map(|value| localizedTime(value, location)),
            },
        );
    }
    Ok(result)
}

/// 加载系统表中全部列的使用/分析时间。
pub fn LoadColumnStatsUsage(
    session: &dyn PredicateColumnSession,
    location: Tz,
) -> Result<HashMap<TableItemID, ColStatsTimeInfo>, PredicateColumnError> {
    loadColumnStatsUsage(session, location, LOAD_ALL_SQL, &[])
}

/// 按表 ID 加载列使用/分析时间。
pub fn LoadColumnStatsUsageForTable(
    session: &dyn PredicateColumnSession,
    location: Tz,
    tableID: i64,
) -> Result<HashMap<TableItemID, ColStatsTimeInfo>, PredicateColumnError> {
    loadColumnStatsUsage(session, location, LOAD_TABLE_SQL, &[SqlArg::I64(tableID)])
}

/// 根据最新 schema 删除表上已丢弃列的使用记录；表不存在则空操作。
fn cleanupDroppedColumnStatsUsage(
    session: &dyn PredicateColumnSession,
    tableID: i64,
) -> Result<(), PredicateColumnError> {
    let Some(column_ids) = session.latest_info_schema().table_column_ids(tableID) else {
        return Ok(());
    };
    session.executor().execute(
        CLEANUP_DROPPED_COLUMNS_SQL,
        &[
            SqlArg::I64(tableID),
            SqlArg::StringList(column_ids.into_iter().map(|id| id.to_string()).collect()),
        ],
    )
}

/// 先清理已删列，再返回表上仍有 last_used_at 的谓词列 ID。
pub fn GetPredicateColumns(
    session: &dyn PredicateColumnSession,
    tableID: i64,
) -> Result<Vec<i64>, PredicateColumnError> {
    cleanupDroppedColumnStatsUsage(session, tableID)?;
    let rows = session
        .executor()
        .query_predicate_columns(PREDICATE_COLUMNS_SQL, &[SqlArg::I64(tableID)])?;
    Ok(rows
        .into_iter()
        .filter(|row| !row.converted_last_used_at_is_null)
        .map(|row| row.column_id)
        .collect())
}

/// Go passes the formatted timestamp, or the literal string "NULL" when absent.
fn timeArgument(value: Option<stats_types::Time>) -> SqlArg {
    SqlArg::String(
        value
            .map(stats_types::Time::String)
            .unwrap_or_else(|| "NULL".to_owned()),
    )
}

/// 逐行 REPLACE 写入表的列使用时间；任一行失败则返回错误。
pub fn SaveColumnStatsUsageForTable(
    session: &dyn PredicateColumnSession,
    values: &HashMap<TableItemID, ColStatsTimeInfo>,
) -> Result<(), PredicateColumnError> {
    for (id, usage) in values {
        session.executor().execute(
            REPLACE_USAGE_SQL,
            &[
                SqlArg::I64(id.TableID),
                SqlArg::I64(id.ID),
                timeArgument(usage.LastUsedAt),
                timeArgument(usage.LastAnalyzedAt),
            ],
        )?;
    }
    Ok(())
}
