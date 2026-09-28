// Copyright 2026 AsterSQL.

// 统计锁（lock stats）子 crate 根。
//
// 锁定表/分区后，自动 ANALYZE 与增量统计更新会跳过它们；本模块提供会话执行接口、
// 锁表元数据、SQL 值类型，并导出加锁、查询与解锁实现。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals
)]
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
/// 统计锁相关操作的错误包装，消息与 Go 侧 `StatsError` 字符串对齐。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatsError(pub String);
impl fmt::Display for StatsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for StatsError {}
/// 待加锁/解锁的表描述：全名及分区 ID → 分区名映射。
#[derive(Clone, Debug, Default)]
pub struct StatsLockTable {
    pub FullName: String,
    pub PartitionInfo: HashMap<i64, String>,
}

/// 受限 SQL 参数/结果单元格的简化值类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SqlValue {
    Int(i64),
    UInt(u64),
    Text(String),
}

impl SqlValue {
    /// 尽量转为 `i64`：整数直接取，文本则解析失败时回退为 0。
    pub fn int(&self) -> i64 {
        match self {
            Self::Int(value) => *value,
            Self::UInt(value) => *value as i64,
            Self::Text(value) => value.parse().unwrap_or_default(),
        }
    }
}

/// 一行受限 SQL 结果。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SqlRow(pub Vec<SqlValue>);

/// 在统计系统表上执行受限 SQL 的执行器抽象（对应 Go 的 session 封装）。
pub trait RestrictedSQLExecutor {
    fn ExecRestrictedSQL(
        &mut self,
        sql: &str,
        arguments: &[SqlValue],
    ) -> Result<Vec<SqlRow>, StatsError>;
    /// 当前会话起始时间戳（StartTS），用于写 `stats_meta.version`。
    fn StartTS(&self) -> u64;

    fn LockedTableIds(&mut self) -> Result<Vec<i64>, StatsError>;
    fn InsertStatsLock(&mut self, table_id: i64) -> Result<(), StatsError>;
    fn UpdateStatsMetaVersion(&mut self, table_id: i64) -> Result<(), StatsError>;
    fn LockedStatsDelta(&mut self, table_id: i64) -> Result<(i64, i64), StatsError>;
    fn ApplyStatsDelta(
        &mut self,
        table_id: i64,
        count: i64,
        modify_count: i64,
    ) -> Result<(), StatsError>;
    fn DeleteStatsLock(&mut self, table_id: i64) -> Result<(), StatsError>;
}

/// 统计会话池：可选择是否把回调内全部 SQL 包在同一 KV 事务中。
pub trait StatsSession: Send + Sync {
    /// Runs one restricted-statistics session. `wrap_transaction` gives all SQL
    /// statements one KV transaction and commits only when the callback succeeds.
    /// 运行一次受限统计会话；`wrap_transaction` 为真时把回调内 SQL 纳入同一 KV 事务，
    /// 仅在回调成功时提交。
    fn WithSession(
        &self,
        wrap_transaction: bool,
        callback: &mut dyn FnMut(&mut dyn RestrictedSQLExecutor) -> Result<(), StatsError>,
    ) -> Result<(), StatsError>;
}
/// 共享的会话池引用。
pub type SessionRef = Arc<dyn StatsSession>;
mod lock_stats;
mod query_lock;
mod unlock_stats;
pub use lock_stats::*;
pub use query_lock::*;
pub use unlock_stats::{
    DeleteLockSQL, RemoveLockedPartitions, RemoveLockedTables, selectDeltaSQL, updateDeltaSQL,
};

#[cfg(test)]
mod lock_stats_test;
#[cfg(test)]
mod main_test;
#[cfg(test)]
mod query_lock_test;
#[cfg(test)]
mod unlock_stats_test;
