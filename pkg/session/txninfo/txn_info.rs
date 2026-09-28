// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 会话事务运行时信息（TxnInfo）与相关 Prometheus 指标。
//
// 描述事务当前状态（空闲/执行/等锁/提交/回滚）、SQL digest、阻塞时间等，
// 并提供 `ToDatum` 以便映射到信息模式表列。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Datelike, Duration as ChronoDuration, Local, Timelike};
use prometheus::{Counter, Histogram};
pub use types::datum::Datum;
use types::datum::{
    Enum, MaxFsp, NewFloat64Datum, NewMysqlEnumDatum, NewStringDatum, NewTime, NewTimeDatum,
    NewUintDatum,
};
use types::time::FromDate;

/// 事务运行状态枚举的底层整型别名。
pub type TxnRunningState = i32;
/// 空闲：尚未执行用户 SQL 或语句间隙。
pub const TxnIdle: TxnRunningState = 0;
/// 正在执行 SQL。
pub const TxnRunning: TxnRunningState = 1;
/// 正在获取悲观锁（等待锁）。
pub const TxnLockAcquiring: TxnRunningState = 2;
/// 正在提交（含两阶段提交过程）。
pub const TxnCommitting: TxnRunningState = 3;
/// 正在回滚。
pub const TxnRollingBack: TxnRunningState = 4;
/// 状态种类数量，用于指标数组长度校验。
pub const TxnStateCounter: TxnRunningState = 5;

// 以下常量为 INFORMATION_SCHEMA 风格列名，供 ToDatum 按名取值。
/// 事务 ID 列（使用 StartTS）。
pub const IDStr: &str = "ID";
/// 事务开始时间列。
pub const StartTimeStr: &str = "START_TIME";
/// 当前语句 SQL digest 列。
pub const CurrentSQLDigestStr: &str = "CURRENT_SQL_DIGEST";
/// 当前语句 digest 文本列（本文件未接 getter）。
pub const CurrentSQLDigestTextStr: &str = "CURRENT_SQL_DIGEST_TEXT";
/// 事务状态列。
pub const StateStr: &str = "STATE";
/// 开始等待锁的时间列。
pub const WaitingStartTimeStr: &str = "WAITING_START_TIME";
/// 内存写缓冲键数量列。
pub const MemBufferKeysStr: &str = "MEM_BUFFER_KEYS";
/// 内存写缓冲字节数列（本文件未接 getter，返回 NULL）。
pub const MemBufferBytesStr: &str = "MEM_BUFFER_BYTES";
/// 会话连接 ID 列。
pub const SessionIDStr: &str = "SESSION_ID";
/// 用户名列。
pub const UserStr: &str = "USER";
/// 当前库名列。
pub const DBStr: &str = "DB";
/// 事务内全部 SQL digest 的 JSON 列。
pub const AllSQLDigestsStr: &str = "ALL_SQL_DIGESTS";
/// 相关表 ID 列表列。
pub const RelatedTableIDsStr: &str = "RELATED_TABLE_IDS";
/// 已等待锁时长（秒）列。
pub const WaitingTimeStr: &str = "WAITING_TIME";

/// 状态枚举到展示名的映射（与 Go 一致）。
pub static TxnRunningStateStrs: [&str; 5] = [
    "Idle",
    "Running",
    "LockWaiting",
    "Committing",
    "RollingBack",
];

fn metricStateLabel(state: TxnRunningState) -> &'static str {
    [
        "idle",
        "executing_sql",
        "acquiring_lock",
        "committing",
        "rolling_back",
    ][state as usize]
}

/// 触发指标注册（与 Go init 副作用对齐）。
pub fn InitMetricsVars() {
    let _ = metrics::TxnStatusEnteringCounterVec();
    let _ = metrics::TxnDurationHistogramVec();
}

/// 按状态与是否持锁选取时长直方图。
pub fn TxnDurationHistogram(state: TxnRunningState, hasLock: bool) -> Histogram {
    metrics::TxnDurationHistogramVec().with_label_values(&[
        metricStateLabel(state),
        if hasLock { "true" } else { "false" },
    ])
}

/// 按状态选取“进入该状态”计数器。
pub fn TxnStatusEnteringCounter(state: TxnRunningState) -> Counter {
    metrics::TxnStatusEnteringCounterVec().with_label_values(&[metricStateLabel(state)])
}

/// 单笔事务的运行时诊断信息。
pub struct TxnInfo {
    /// 开始时间戳（TSO）；高位为物理时间。
    pub StartTS: u64,
    /// 当前正在执行语句的 SQL digest。
    pub CurrentSQLDigest: String,
    /// 本事务已执行语句 digest 序列。
    pub AllSQLDigests: Vec<String>,
    /// 当前运行状态。
    pub State: TxnRunningState,
    /// 最近一次状态变更时间。
    pub LastStateChangeTime: SystemTime,
    /// 开始阻塞（等锁）的时间；Valid 表示是否在等待。
    pub BlockStartTime: BlockStartTime,
    /// 内存写缓冲中的键条数。
    pub EntriesCount: u64,
    /// 关联会话进程信息（连接、用户、库、相关表）。
    pub ProcessInfo: Option<ProcessInfo>,
}

impl Default for TxnInfo {
    fn default() -> Self {
        Self {
            StartTS: 0,
            CurrentSQLDigest: String::new(),
            AllSQLDigests: Vec::new(),
            State: TxnIdle,
            LastStateChangeTime: UNIX_EPOCH,
            BlockStartTime: BlockStartTime::default(),
            EntriesCount: 0,
            ProcessInfo: None,
        }
    }
}

/// 阻塞开始时间包装：Valid 为 false 表示当前未阻塞。
pub struct BlockStartTime {
    pub Valid: bool,
    pub Time: SystemTime,
}

impl Default for BlockStartTime {
    fn default() -> Self {
        Self {
            Valid: false,
            Time: UNIX_EPOCH,
        }
    }
}

#[derive(Default)]
/// 与事务关联的会话侧进程信息。
pub struct ProcessInfo {
    /// 连接 ID。
    pub ConnectionID: u64,
    /// 用户名。
    pub Username: String,
    /// 当前数据库。
    pub CurrentDB: String,
    /// 相关表 ID 集合（用 HashMap 模拟集合）。
    pub RelatedTableIDs: HashMap<i64, ()>,
}

/// 列名到取值函数的映射类型。
type ColumnValueGetter = fn(&TxnInfo) -> Datum;
static COLUMN_VALUE_GETTER_MAP: OnceLock<HashMap<&'static str, ColumnValueGetter>> =
    OnceLock::new();

/// 返回空 Datum（SQL NULL）。
fn nullDatum() -> Datum {
    Datum::default()
}

/// 将 SystemTime 转为 MySQL TIMESTAMP Datum。
fn mysqlTimestamp(time: SystemTime) -> Datum {
    let local: DateTime<Local> = time.into();
    let rounded = local + ChronoDuration::nanoseconds(500);
    let value = NewTime(
        FromDate(
            rounded.year(),
            rounded.month() as i32,
            rounded.day() as i32,
            rounded.hour() as i32,
            rounded.minute() as i32,
            rounded.second() as i32,
            rounded.nanosecond() as i32 / 1_000,
        ),
        parser_mysql::r#type::TypeTimestamp,
        MaxFsp,
    );
    NewTimeDatum(value)
}

/// 从 StartTS 还原开始时间。
fn startTime(info: &TxnInfo) -> Datum {
    let time = UNIX_EPOCH
        .checked_add(std::time::Duration::from_millis(info.StartTS >> 18))
        .unwrap_or(UNIX_EPOCH);
    mysqlTimestamp(time)
}

/// 当前 digest；空串映射为 NULL。
fn currentSQLDigest(info: &TxnInfo) -> Datum {
    if info.CurrentSQLDigest.is_empty() {
        nullDatum()
    } else {
        NewStringDatum(info.CurrentSQLDigest.clone())
    }
}

/// 状态枚举 Datum（Name + 1-based Value）。
fn stateDatum(info: &TxnInfo) -> Datum {
    let index = info.State as usize;
    NewMysqlEnumDatum(Enum {
        Name: TxnRunningStateStrs[index].to_owned(),
        Value: index as u64 + 1,
    })
}

/// 等待开始时间；无效则 NULL。
fn waitingStartTime(info: &TxnInfo) -> Datum {
    if info.BlockStartTime.Valid {
        mysqlTimestamp(info.BlockStartTime.Time)
    } else {
        nullDatum()
    }
}

/// 取出进程信息引用。
fn processInfo(info: &TxnInfo) -> Option<&ProcessInfo> {
    info.ProcessInfo.as_ref()
}

/// 全部 digest 序列化为 JSON 字符串。
fn allSQLDigests(info: &TxnInfo) -> Datum {
    match serde_json::to_string(&info.AllSQLDigests) {
        Ok(value) => NewStringDatum(value),
        Err(_) => nullDatum(),
    }
}

/// 相关表 ID 逗号拼接。
fn relatedTableIDs(info: &TxnInfo) -> Datum {
    let value = processInfo(info)
        .map(|process| {
            process
                .RelatedTableIDs
                .keys()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    NewStringDatum(value)
}

/// 自阻塞开始至今的秒数；未阻塞则 NULL。
fn waitingTime(info: &TxnInfo) -> Datum {
    if !info.BlockStartTime.Valid {
        return nullDatum();
    }
    let seconds = match SystemTime::now().duration_since(info.BlockStartTime.Time) {
        Ok(duration) => duration.as_secs_f64(),
        Err(error) => -error.duration().as_secs_f64(),
    };
    NewFloat64Datum(seconds)
}

/// 懒构建列名到 getter 的静态映射表。
pub fn columnValueGetterMap() -> &'static HashMap<&'static str, ColumnValueGetter> {
    COLUMN_VALUE_GETTER_MAP.get_or_init(|| {
        HashMap::from([
            (
                IDStr,
                (|info: &TxnInfo| NewUintDatum(info.StartTS)) as ColumnValueGetter,
            ),
            (StartTimeStr, startTime as ColumnValueGetter),
            (CurrentSQLDigestStr, currentSQLDigest as ColumnValueGetter),
            (StateStr, stateDatum as ColumnValueGetter),
            (WaitingStartTimeStr, waitingStartTime as ColumnValueGetter),
            (
                MemBufferKeysStr,
                (|info: &TxnInfo| NewUintDatum(info.EntriesCount)) as ColumnValueGetter,
            ),
            (
                SessionIDStr,
                (|info: &TxnInfo| {
                    NewUintDatum(
                        processInfo(info)
                            .map(|value| value.ConnectionID)
                            .unwrap_or(0),
                    )
                }) as ColumnValueGetter,
            ),
            (
                UserStr,
                (|info: &TxnInfo| {
                    NewStringDatum(
                        processInfo(info)
                            .map(|value| value.Username.clone())
                            .unwrap_or_default(),
                    )
                }) as ColumnValueGetter,
            ),
            (
                DBStr,
                (|info: &TxnInfo| {
                    NewStringDatum(
                        processInfo(info)
                            .map(|value| value.CurrentDB.clone())
                            .unwrap_or_default(),
                    )
                }) as ColumnValueGetter,
            ),
            (AllSQLDigestsStr, allSQLDigests as ColumnValueGetter),
            (RelatedTableIDsStr, relatedTableIDs as ColumnValueGetter),
            (WaitingTimeStr, waitingTime as ColumnValueGetter),
        ])
    })
}

impl TxnInfo {
    /// 按列名取值；未知列返回 NULL。
    pub fn ToDatum(&self, column: &str) -> Datum {
        columnValueGetterMap()
            .get(column)
            .map(|getter| getter(self))
            .unwrap_or_else(nullDatum)
    }
}
