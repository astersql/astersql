// Copyright 2016 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

// 系统变量工具函数：开关字符串转换、校验辅助、TS/内存限额解析等。
//
// 对应 Go `varsutil.go`。包含字符集/排序规则检查、只读与隔离级别校验、
// SnapshotTS / TxnReadTS / ReadStaleness（陈旧读）互斥设置，以及表达式索引允许函数列表。

use std::collections::{HashMap, HashSet};

use chrono::{LocalResult, NaiveDateTime, TimeZone};
use parser_charset::charset;

use crate::vardef;
use crate::{
    Context, SessionVars, VariableError, VariableErrorKind, call_disable_ddl_hook,
    call_disable_stats_owner_hook, call_enable_ddl_hook, call_enable_stats_owner_hook,
};

/// 一年的秒数（近似，非闰年）。
pub const secondsPerYear: i64 = 60 * 60 * 24 * 365;

/// 将布尔转为 `ON`/`OFF` 字符串。
pub fn BoolToOnOff(value: bool) -> String {
    if value {
        vardef::On.to_owned()
    } else {
        vardef::Off.to_owned()
    }
}

/// 将 1/非1 的 i32 转为 ON/OFF。
fn int32ToBoolStr(value: i32) -> String {
    BoolToOnOff(value == 1)
}

/// 校验排序规则名：小写、含下划线、仅字母数字与下划线。
pub fn checkCollation(
    _vars: &mut SessionVars,
    normalized: &str,
    _original: &str,
    _scope: vardef::ScopeFlag,
) -> Result<String, VariableError> {
    charset::GetCollationByName(normalized)
        .map(|collation| collation.Name)
        .map_err(|error| VariableError::new(VariableErrorKind::WrongValue, error.to_string()))
}

/// 校验 utf8mb4 默认排序规则，仅允许 bin / general_ci。
pub fn checkDefaultCollationForUTF8MB4(
    vars: &mut SessionVars,
    normalized: &str,
    original: &str,
    scope: vardef::ScopeFlag,
) -> Result<String, VariableError> {
    let collation = checkCollation(vars, normalized, original, scope)?;
    if matches!(
        collation.as_str(),
        "utf8mb4_bin" | "utf8mb4_general_ci" | "utf8mb4_0900_ai_ci"
    ) {
        Ok(collation)
    } else {
        Err(VariableError::new(
            VariableErrorKind::WrongValue,
            format!("Invalid default utf8mb4 collation '{collation}'"),
        ))
    }
}

/// 校验字符集名；空值视为非法 NULL。
pub fn checkCharacterSet(value: &str, name: &str) -> Result<String, VariableError> {
    if value.is_empty() {
        return Err(VariableError::wrong_value(name, "NULL"));
    }
    charset::GetCharsetInfo(value)
        .map(|info| info.Name)
        .map_err(|_| VariableError::wrong_value(name, value))
}

/// 开启 READ ONLY / OFFLINE MODE 时检查 noop 函数开关是否允许。
pub fn checkReadOnly(
    vars: &mut SessionVars,
    normalized: &str,
    original: &str,
    scope: vardef::ScopeFlag,
    offline_mode: bool,
) -> Result<String, VariableError> {
    // 未开启只读时直接通过
    if !TiDBOptOn(normalized) {
        return Ok(normalized.to_owned());
    }
    let feature = if offline_mode {
        "OFFLINE MODE"
    } else {
        "READ ONLY"
    };
    let error = VariableError::new(
        VariableErrorKind::InvalidValue,
        format!(
            "function {feature} has only noop implementation; enable tidb_enable_noop_functions"
        ),
    );
    if scope == vardef::ScopeSession {
        if vars.NoopFuncsMode == OffInt {
            return Err(error);
        }
        if vars.NoopFuncsMode == WarnInt {
            vars.StmtCtx.append_warning(error);
        }
    } else if scope == vardef::ScopeGlobal {
        let value = vars
            .GlobalVarsAccessor
            .get_global_sys_var(vardef::TiDBEnableNoopFuncs)
            .map_err(|_| VariableError::unknown(vardef::TiDBEnableNoopFuncs))?;
        if value == vardef::Off {
            return Err(error);
        }
        if value == vardef::Warn {
            vars.StmtCtx.append_warning(error);
        }
    } else {
        return Ok(original.to_owned());
    }
    Ok(normalized.to_owned())
}

/// 校验隔离级别；SERIALIZABLE / READ-UNCOMMITTED 默认不支持，可跳过检查并警告。
pub fn checkIsolationLevel(
    vars: &mut SessionVars,
    normalized: &str,
    _original: &str,
    _scope: vardef::ScopeFlag,
) -> Result<String, VariableError> {
    // 这两个隔离级别默认不支持，除非跳过检查
    if matches!(normalized, "SERIALIZABLE" | "READ-UNCOMMITTED") {
        let skip_check = vars
            .system(vardef::TiDBSkipIsolationLevelCheck)
            .is_some_and(TiDBOptOn);
        let error = VariableError::new(
            VariableErrorKind::UnsupportedIsolationLevel,
            format!("Unsupported isolation level '{normalized}'"),
        );
        if !skip_check {
            return Err(error);
        }
        vars.StmtCtx.append_warning(error);
    }
    Ok(normalized.to_owned())
}

/// 从 mysql.tidb 表读取配置，并将 true/false 规范为 ON/OFF。
pub fn getTiDBTableValue(
    vars: &SessionVars,
    name: &str,
    default_value: &str,
) -> Result<String, VariableError> {
    match vars.GlobalVarsAccessor.get_tidb_table_value(name) {
        Ok(value) => Ok(trueFalseToOnOff(&value)),
        Err(_) => Ok(default_value.to_owned()),
    }
}

/// 写入 mysql.tidb 表，并将 ON/OFF 转为 true/false 存储。
pub fn setTiDBTableValue(
    vars: &mut SessionVars,
    name: &str,
    value: &str,
    comment: &str,
) -> Result<(), VariableError> {
    vars.GlobalVarsAccessor
        .set_tidb_table_value(name, &OnOffToTrueFalse(value), comment)
}

/// `true`/`false`（忽略大小写）转为 `ON`/`OFF`。
pub fn trueFalseToOnOff(value: &str) -> String {
    if value.eq_ignore_ascii_case("true") {
        vardef::On.to_owned()
    } else if value.eq_ignore_ascii_case("false") {
        vardef::Off.to_owned()
    } else {
        value.to_owned()
    }
}

/// `ON`/`OFF` 转为 `true`/`false`。
pub fn OnOffToTrueFalse(value: &str) -> String {
    if value.eq_ignore_ascii_case(vardef::On) {
        "true".to_owned()
    } else if value.eq_ignore_ascii_case(vardef::Off) {
        "false".to_owned()
    } else {
        value.to_owned()
    }
}

/// init_chunk_size 上界。
pub const initChunkSizeUpperBound: i32 = 32;
/// max_chunk_size 下界。
pub const maxChunkSizeLowerBound: i32 = 32;

/// 追加弃用警告，提示改用替代变量。
pub fn appendDeprecationWarning(vars: &mut SessionVars, name: &str, replacement: &str) {
    vars.StmtCtx.append_warning(VariableError::new(
        VariableErrorKind::InvalidValue,
        format!("'{name}' is deprecated; use {replacement} instead"),
    ));
}

/// 判断字符串是否表示开启（ON 或 "1"）。
pub fn TiDBOptOn(value: &str) -> bool {
    value.eq_ignore_ascii_case(vardef::On) || value == "1"
}

/// OFF 对应的整型枚举值。
pub const OffInt: i32 = 0;
/// ON 对应的整型枚举值。
pub const OnInt: i32 = 1;
/// WARN 对应的整型枚举值。
pub const WarnInt: i32 = 2;

/// 将 ON/OFF/WARN 映射为整型三态。
pub fn TiDBOptOnOffWarn(value: &str) -> i32 {
    match value {
        vardef::Warn => WarnInt,
        vardef::On => OnInt,
        _ => OffInt,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 事务断言级别：关闭 / 快速 / 严格。
pub enum AssertionLevel {
    AssertionLevelOff,
    AssertionLevelFast,
    AssertionLevelStrict,
}

/// 解析断言级别字符串。
pub fn tidbOptAssertionLevel(value: &str) -> AssertionLevel {
    match value {
        vardef::AssertionStrictStr => AssertionLevel::AssertionLevelStrict,
        vardef::AssertionFastStr => AssertionLevel::AssertionLevelFast,
        _ => AssertionLevel::AssertionLevelOff,
    }
}

/// 解析正整数；失败或非正则回退默认值。
pub fn tidbOptPositiveInt32(value: &str, default_value: i32) -> i32 {
    value
        .parse::<i32>()
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(default_value)
}

/// 解析 i32，失败则用默认值。
pub fn TidbOptInt(value: &str, default_value: i32) -> i32 {
    value.parse().unwrap_or(default_value)
}

/// 解析 i64，失败则用默认值。
pub fn TidbOptInt64(value: &str, default_value: i64) -> i64 {
    value.parse().unwrap_or(default_value)
}

/// 解析 u64，失败则用默认值。
pub fn TidbOptUint64(value: &str, default_value: u64) -> u64 {
    value.parse().unwrap_or(default_value)
}

/// 解析 f64，失败则用默认值。
pub fn tidbOptFloat64(value: &str, default_value: f64) -> f64 {
    value.parse().unwrap_or(default_value)
}

/// 解析内存限额：支持百分比或字节单位，过小则警告并抬升到 512MB。
pub fn parseMemoryLimit(
    vars: &mut SessionVars,
    normalized: &str,
    original: &str,
) -> Result<(u64, String), VariableError> {
    // 优先用会话已知总内存；否则探测系统内存
    let total = if !vars.memory_total_available {
        0
    } else if vars.memory_total != 0 {
        vars.memory_total
    } else {
        sysinfo::System::new_all().total_memory()
    };
    let (mut byte_size, mut normalized_string) = if total != 0 {
        let (percentage, value) = parsePercentage(normalized);
        if percentage != 0 {
            (total.saturating_mul(percentage) / 100, value)
        } else {
            parseByteSize(normalized)
        }
    } else {
        parseByteSize(normalized)
    };
    if normalized_string.is_empty() {
        return Err(VariableError::new(
            VariableErrorKind::TruncatedWrongValue,
            format!("invalid memory limit '{original}'"),
        ));
    }
    // 过小限额警告并强制至少 512MB
    if byte_size > 0 && byte_size < (512_u64 << 20) {
        vars.StmtCtx.append_warning(VariableError::truncated(
            vardef::TiDBServerMemoryLimit,
            original,
        ));
        byte_size = 512_u64 << 20;
        normalized_string = "512MB".to_owned();
    }
    Ok((byte_size, normalized_string))
}

/// 解析 `N%` 百分比；0 或 >=100 视为非法。
pub fn parsePercentage(value: &str) -> (u64, String) {
    let Some(number) = value.strip_suffix('%') else {
        return (0, String::new());
    };
    let Ok(percentage) = number.parse::<u64>() else {
        return (0, String::new());
    };
    if percentage == 0 || percentage >= 100 {
        (0, String::new())
    } else {
        (percentage, format!("{percentage}%"))
    }
}

/// 解析带 KB/MB/GB/TB（及 KiB 等）后缀的字节大小。
pub fn parseByteSize(value: &str) -> (u64, String) {
    for (suffix, shift) in [
        ("KiB", 10),
        ("KB", 10),
        ("MiB", 20),
        ("MB", 20),
        ("GiB", 30),
        ("GB", 30),
        ("TiB", 40),
        ("TB", 40),
    ] {
        if let Some(number) = value.strip_suffix(suffix)
            && let Ok(number) = number.parse::<u64>()
        {
            return (number.wrapping_shl(shift), value.to_owned());
        }
    }
    value
        .parse::<u64>()
        .map(|number| (number, number.to_string()))
        .unwrap_or((0, String::new()))
}

/// 设置 SnapshotTS；与 ReadStaleness 互斥，并清零 TxnReadTS。
pub fn setSnapshotTS(vars: &mut SessionVars, value: &str) -> Result<(), VariableError> {
    if value.is_empty() {
        vars.SnapshotTS = 0;
        vars.SnapshotInfoschema = None;
        return Ok(());
    }
    // Snapshot 与陈旧读互斥
    if vars.ReadStaleness != 0 {
        return Err(VariableError::new(
            VariableErrorKind::InvalidValue,
            "tidb_read_staleness should be clear before setting tidb_snapshot",
        ));
    }
    let timestamp = parseTSFromNumberOrTime(vars, value);
    vars.SnapshotTS = timestamp.as_ref().copied().unwrap_or(0);
    vars.TxnReadTS = 0;
    timestamp.map(|_| ())
}

/// 从数字或 `YYYY-MM-DD HH:MM:SS` 解析 TSO（毫秒左移 18 位）。
pub fn parseTSFromNumberOrTime(vars: &SessionVars, value: &str) -> Result<u64, VariableError> {
    if let Ok(timestamp) = value.parse::<u64>() {
        return Ok(timestamp);
    }
    parseTSFromTime(vars, value)
}

fn parseTSFromTime(vars: &SessionVars, value: &str) -> Result<u64, VariableError> {
    let parsed = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
        .map_err(|error| VariableError::new(VariableErrorKind::WrongType, error.to_string()))?;
    let timestamp = match vars.location().from_local_datetime(&parsed) {
        LocalResult::Single(timestamp) => timestamp,
        LocalResult::Ambiguous(first, _) => first,
        LocalResult::None => {
            return Err(VariableError::new(
                VariableErrorKind::WrongValue,
                format!("invalid timestamp '{value}'"),
            ));
        }
    };
    let millis = timestamp.timestamp_millis();
    if millis < 0 {
        return Err(VariableError::wrong_value("timestamp", value));
    }
    Ok((millis as u64) << 18)
}

/// 设置事务读 TS，并清除 SnapshotTS / SnapshotInfoschema。
pub fn setTxnReadTS(vars: &mut SessionVars, value: &str) -> Result<(), VariableError> {
    if value.is_empty() {
        vars.TxnReadTS = 0;
        return Ok(());
    }
    vars.TxnReadTS = parseTSFromTime(vars, value)?;
    vars.SnapshotTS = 0;
    vars.SnapshotInfoschema = None;
    Ok(())
}

/// 设置陈旧读秒数（存为纳秒）；与 SnapshotTS 互斥。
pub fn setReadStaleness(vars: &mut SessionVars, value: &str) -> Result<(), VariableError> {
    if value.is_empty() || value == "0" {
        vars.ReadStaleness = 0;
        return Ok(());
    }
    // 陈旧读与 Snapshot 互斥
    if vars.SnapshotTS != 0 {
        return Err(VariableError::new(
            VariableErrorKind::InvalidValue,
            "tidb_snapshot should be clear before setting tidb_read_staleness",
        ));
    }
    let seconds = value
        .parse::<i32>()
        .map_err(|error| VariableError::new(VariableErrorKind::WrongType, error.to_string()))?;
    vars.ReadStaleness = i64::from(seconds) * 1_000_000_000;
    Ok(())
}

/// 通过钩子启用或禁用 DDL Owner。
pub fn switchDDL(enabled: bool) -> Result<(), VariableError> {
    if enabled {
        call_enable_ddl_hook()
    } else {
        call_disable_ddl_hook()
    }
}

/// 通过钩子启用或禁用统计信息 Owner。
pub fn switchStats(enabled: bool) -> Result<(), VariableError> {
    if enabled {
        call_enable_stats_owner_hook()
    } else {
        call_disable_stats_owner_hook()
    }
}

/// 表达式索引（Expression Index）已 GA 允许的函数名列表。
pub static GAFunction4ExpressionIndex: &[&str] = &[
    "lower",
    "upper",
    "md5",
    "reverse",
    "vitess_hash",
    "tidb_shard",
    "json_type",
    "json_extract",
    "json_unquote",
    "json_array",
    "json_object",
    "json_set",
    "json_insert",
    "json_replace",
    "json_remove",
    "json_contains",
    "json_contains_path",
    "json_valid",
    "json_array_append",
    "json_array_insert",
    "json_merge_patch",
    "json_merge_preserve",
    "json_pretty",
    "json_quote",
    "json_schema_valid",
    "json_search",
    "json_storage_size",
    "json_depth",
    "json_keys",
    "json_length",
];

/// 返回排序后的表达式索引允许函数名（逗号分隔）。
pub fn collectAllowFuncName4ExpressionIndex() -> String {
    let mut functions = GAFunction4ExpressionIndex.to_vec();
    functions.sort_unstable();
    functions.join(", ")
}

/// 更新密码最小长度：写全局变量并同步原子配置。
pub fn updatePasswordValidationLength(
    vars: &mut SessionVars,
    length: i32,
) -> Result<(), VariableError> {
    vars.GlobalVarsAccessor.set_global_sys_var_only(
        &Context,
        vardef::ValidatePasswordLength,
        &length.to_string(),
        false,
    )?;
    vardef::PasswordValidationLength.Store(length);
    Ok(())
}

/// ANALYZE 可跳过的列类型白名单。
pub static analyzeSkipAllowedTypes: &[&str] = &[
    "json",
    "text",
    "mediumtext",
    "longtext",
    "blob",
    "mediumblob",
    "longblob",
];

/// 校验并归一化 analyze-skip 列类型列表。
pub fn ValidAnalyzeSkipColumnTypes(value: &str) -> Result<String, VariableError> {
    if value.is_empty() {
        return Ok(String::new());
    }
    let mut normalized = Vec::new();
    let lower = value.to_ascii_lowercase();
    for item in lower.split(',') {
        let column_type = item.trim();
        if !analyzeSkipAllowedTypes.contains(&column_type) {
            return Err(VariableError::wrong_value(
                vardef::TiDBAnalyzeSkipColumnTypes,
                value,
            ));
        }
        normalized.push(column_type.to_owned());
    }
    Ok(normalized.join(","))
}

/// 解析 analyze-skip 列类型为集合（忽略非法项）。
pub fn ParseAnalyzeSkipColumnTypes(value: &str) -> HashSet<String> {
    value
        .to_ascii_lowercase()
        .split(',')
        .filter(|value| analyzeSkipAllowedTypes.contains(value))
        .map(str::to_owned)
        .collect()
}

/// Schema 缓存大小下界（64MB）。
pub const SchemaCacheSizeLowerBound: u64 = 64_u64 << 20;
/// Schema 缓存大小下界的字符串表示。
pub const SchemaCacheSizeLowerBoundStr: &str = "64MB";

/// 解析 Schema 缓存大小：过小抬到下界，过大裁到 i64::MAX。
pub fn parseSchemaCacheSize(
    vars: &mut SessionVars,
    normalized: &str,
    original: &str,
) -> Result<(u64, String), VariableError> {
    let (mut byte_size, mut normalized_string) = parseByteSize(normalized);
    if normalized_string.is_empty() {
        return Err(VariableError::new(
            VariableErrorKind::TruncatedWrongValue,
            format!("invalid schema cache size '{original}'"),
        ));
    }
    if byte_size > 0 && byte_size < SchemaCacheSizeLowerBound {
        vars.StmtCtx.append_warning(VariableError::truncated(
            vardef::TiDBSchemaCacheSize,
            original,
        ));
        byte_size = SchemaCacheSizeLowerBound;
        normalized_string = SchemaCacheSizeLowerBoundStr.to_owned();
    }
    if byte_size > i64::MAX as u64 {
        vars.StmtCtx.append_warning(VariableError::truncated(
            vardef::TiDBSchemaCacheSize,
            original,
        ));
        byte_size = i64::MAX as u64;
        normalized_string = i64::MAX.to_string();
    }
    Ok((byte_size, normalized_string))
}

/// 将表达式索引允许函数列表转为查找用 HashMap。
pub fn expression_index_function_map() -> HashMap<&'static str, ()> {
    GAFunction4ExpressionIndex
        .iter()
        .copied()
        .map(|name| (name, ()))
        .collect()
}
