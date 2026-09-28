// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// TiDB GC（Garbage Collection，垃圾回收）工具：安全点读写、snapshot 校验与 GC 启停。
//
// 对应 Go `util/gcutil`。GC safe point（安全点）是 TiKV 保证不再回收的最小时间戳；
// 读历史快照（snapshot）时若 TS 早于安全点会报 SnapshotTooOld。TSO（Timestamp Oracle）
// 时间戳高位为物理毫秒，低 18 位为逻辑计数。

#![allow(non_snake_case, non_upper_case_globals)]

use chrono::{DateTime, FixedOffset, Timelike, Utc};
use thiserror::Error;

/// 从 `mysql.tidb` 读取变量值的受限 SQL（HIGH_PRIORITY）。
pub const selectVariableValueSQL: &str =
    "SELECT HIGH_PRIORITY variable_value FROM mysql.tidb WHERE variable_name=%?";

/// `mysql.tidb` 中存储 TiKV GC 安全点的变量名。
const GC_SAFE_POINT_VARIABLE: &str = "tikv_gc_safe_point";
/// Go 风格安全点时间格式（用于错误消息展示）。
const GC_TIME_FORMAT: &str = "20060102-15:04:05.000 -0700";
/// 受限 SQL 内部事务来源标记：标识为 GC 内部请求。
const INTERNAL_TXN_GC: &str = "gc";
/// TSO 逻辑位宽度：物理毫秒左移该位数得到完整 TS。
const TSO_LOGICAL_BITS: u32 = 18;

/// Errors returned by the GC utility boundary.
///
/// Dependency errors stay as their original boxed errors, while TiDB's
/// snapshot-too-old error retains its stable MySQL error code.
///
/// GC 工具边界错误：依赖错误透传；SnapshotTooOld 保留稳定 MySQL 错误码。
#[derive(Debug, Error)]
pub enum GcUtilError {
    #[error(transparent)]
    Dependency(#[from] sessionctx::GoError),
    #[error("can not get 'tikv_gc_safe_point'")]
    MissingSafePoint,
    #[error("string \"{value}\" doesn't has a prefix that matches format \"{GC_TIME_FORMAT}\"")]
    InvalidSafePointTime { value: String },
    #[error("{message}")]
    SnapshotTooOld { code: u16, message: String },
}

impl GcUtilError {
    /// Returns the stable TiDB/MySQL error code when this error has one.
    /// 若有稳定 TiDB/MySQL 错误码则返回。
    pub fn code(&self) -> Option<u16> {
        match self {
            Self::SnapshotTooOld { code, .. } => Some(*code),
            _ => None,
        }
    }
}

/// Minimal global-variable contract used by the Go implementation.
///
/// Setters take `&self` because Go accessors are shared interface values and
/// concrete Rust session implementations normally provide interior locking.
///
/// 全局系统变量读写契约（对应 Go session 接口子集）。
pub trait GlobalVarAccessor: Send + Sync {
    fn get_global_sys_var(&self, name: &str) -> Result<String, sessionctx::GoError>;
    fn set_global_sys_var(&self, name: &str, value: &str) -> Result<(), sessionctx::GoError>;
}

/// Context passed to restricted SQL, retaining the Go internal-source marker.
/// 受限 SQL 上下文，保留 Go 的 internal source 标记。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RestrictedSqlContext {
    internal_source_type: Option<&'static str>,
}

impl RestrictedSqlContext {
    /// 设置内部来源类型（如 `"gc"`）。
    fn with_internal_source_type(mut self, source: &'static str) -> Self {
        self.internal_source_type = Some(source);
        self
    }

    /// 读取内部来源；未设置时返回空串。
    pub fn internal_source_type(&self) -> &str {
        self.internal_source_type.unwrap_or_default()
    }
}

/// One restricted-SQL row. `GetGCSafePoint` reads column zero just as Go does.
/// 受限 SQL 一行；`GetGCSafePoint` 与 Go 一样读第 0 列。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RestrictedRow {
    values: Vec<String>,
}

impl RestrictedRow {
    /// 由列值列表构造。
    pub fn new(values: Vec<String>) -> Self {
        Self { values }
    }

    /// 按列下标取字符串。
    pub fn get_string(&self, index: usize) -> Option<&str> {
        self.values.get(index).map(String::as_str)
    }
}

/// Restricted-SQL behavior required by this package.
/// 本包所需的受限 SQL 执行能力。
pub trait RestrictedSqlExecutor: Send + Sync {
    fn exec_restricted_sql(
        &self,
        ctx: RestrictedSqlContext,
        sql: &str,
        arguments: &[&str],
    ) -> Result<Vec<RestrictedRow>, sessionctx::GoError>;
}

/// Narrow, usable adapter over the two `sessionctx.Context` capabilities used
/// by the Go package. Full session implementations can implement this trait
/// without reproducing the unrelated transaction and planner method set.
///
/// 会话能力窄接口：仅全局变量 + 受限 SQL。
pub trait Context {
    fn global_vars_accessor(&self) -> &dyn GlobalVarAccessor;
    fn restricted_sql_executor(&self) -> &dyn RestrictedSqlExecutor;
}

/// CheckGCEnable checks whether the TiDB GC global variable is enabled.
/// 检查 TiDB GC 全局开关是否开启。
pub fn CheckGCEnable(ctx: &dyn Context) -> Result<bool, GcUtilError> {
    let value = ctx
        .global_vars_accessor()
        .get_global_sys_var(vardef::TiDBGCEnable)?;
    Ok(variable::TiDBOptOn(&value))
}

/// DisableGC disables the TiDB GC global variable.
/// 关闭 TiDB GC 全局开关。
pub fn DisableGC(ctx: &dyn Context) -> Result<(), GcUtilError> {
    ctx.global_vars_accessor()
        .set_global_sys_var(vardef::TiDBGCEnable, vardef::Off)?;
    Ok(())
}

/// EnableGC enables the TiDB GC global variable.
/// 开启 TiDB GC 全局开关。
pub fn EnableGC(ctx: &dyn Context) -> Result<(), GcUtilError> {
    ctx.global_vars_accessor()
        .set_global_sys_var(vardef::TiDBGCEnable, vardef::On)?;
    Ok(())
}

/// ValidateSnapshot checks that the snapshot timestamp is not older than the
/// GC safe point loaded through restricted SQL.
/// 校验 snapshot TS 不早于经受限 SQL 加载的 GC 安全点。
pub fn ValidateSnapshot(ctx: &dyn Context, snapshotTS: u64) -> Result<(), GcUtilError> {
    let safePointTS = GetGCSafePoint(ctx)?;
    ValidateSnapshotWithGCSafePoint(snapshotTS, safePointTS)
}

/// ValidateSnapshotWithGCSafePoint validates an already loaded safe point.
/// 用已加载的安全点校验 snapshot TS。
pub fn ValidateSnapshotWithGCSafePoint(
    snapshotTS: u64,
    safePointTS: u64,
) -> Result<(), GcUtilError> {
    if safePointTS > snapshotTS {
        let descriptor = &variable::error::ErrSnapshotTooOld;
        let safe_point = format_go_time(ts_convert_to_time(safePointTS));
        return Err(GcUtilError::SnapshotTooOld {
            code: descriptor.code,
            message: descriptor.format(&[&safe_point]),
        });
    }
    Ok(())
}

/// GetGCSafePoint loads and parses `tikv_gc_safe_point` from `mysql.tidb`.
/// 从 `mysql.tidb` 加载并解析 `tikv_gc_safe_point`，转为 TSO。
pub fn GetGCSafePoint(ctx: &dyn Context) -> Result<u64, GcUtilError> {
    let sql_context = RestrictedSqlContext::default().with_internal_source_type(INTERNAL_TXN_GC);
    let rows = ctx.restricted_sql_executor().exec_restricted_sql(
        sql_context,
        selectVariableValueSQL,
        &[GC_SAFE_POINT_VARIABLE],
    )?;
    if rows.len() != 1 {
        return Err(GcUtilError::MissingSafePoint);
    }

    let value = rows[0].get_string(0).ok_or(GcUtilError::MissingSafePoint)?;
    let safe_point = compatible_parse_gc_time(value)?;
    // 对齐 oracle.GoTimeToTS：UnixNano 的 i64 运算允许回绕，除以毫秒向零截断，
    // 随后仍以 i64 左移逻辑位，最后才转换为 u64。
    let unix_nanos = safe_point
        .timestamp()
        .wrapping_mul(1_000_000_000)
        .wrapping_add(i64::from(safe_point.timestamp_subsec_nanos()));
    let physical_millis = unix_nanos / 1_000_000;
    Ok(physical_millis.wrapping_shl(TSO_LOGICAL_BITS) as u64)
}

/// 兼容解析 GC 时间串：先整串，失败则去掉末段空格字段再试（client-go 旧时区缩写）。
fn compatible_parse_gc_time(value: &str) -> Result<DateTime<FixedOffset>, GcUtilError> {
    if let Some(parsed) = parse_gc_time_prefix(value) {
        return Ok(parsed);
    }

    // client-go compatibility: remove exactly the final space-separated field
    // and retry, accepting old values with a trailing timezone abbreviation.
    // client-go 兼容：去掉最后一个空格分段（旧时区缩写）后重试。
    if let Some((prefix, _)) = value.rsplit_once(' ') {
        if let Some(parsed) = parse_gc_time_prefix(prefix) {
            return Ok(parsed);
        }
    }

    Err(GcUtilError::InvalidSafePointTime {
        value: value.to_owned(),
    })
}

/// 按 `%Y%m%d-%H:%M:%S[.f] %z` 解析前缀时间。
fn parse_gc_time_prefix(value: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_str(value, "%Y%m%d-%H:%M:%S%.f %z")
        .or_else(|_| DateTime::parse_from_str(value, "%Y%m%d-%H:%M:%S %z"))
        .ok()
}

/// TSO → UTC：右移逻辑位得到物理毫秒时间戳。
fn ts_convert_to_time(ts: u64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis((ts >> TSO_LOGICAL_BITS) as i64)
        .expect("TSO physical milliseconds must be a valid UTC timestamp")
}

/// 格式化为 Go 风格时间串（用于 SnapshotTooOld 错误消息）。
fn format_go_time(value: DateTime<Utc>) -> String {
    let mut formatted = value.format("%Y-%m-%d %H:%M:%S").to_string();
    let nanos = value.nanosecond();
    if nanos != 0 {
        let fraction = format!("{nanos:09}");
        formatted.push('.');
        formatted.push_str(fraction.trim_end_matches('0'));
    }
    formatted.push_str(" +0000 UTC");
    formatted
}
