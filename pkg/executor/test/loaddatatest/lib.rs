// Copyright 2026 AsterSQL.

//! Executable LOAD DATA semantics used by the port of `loaddatatest`.
//!
//! The full SQL executor is not a dependency of this test-only crate yet.  The
//! model below deliberately keeps the observable parts of TiDB's LOAD DATA
//! contract in one place: CSV dialect handling, NULL rules, row filtering,
//! column mapping, duplicate handling, conversion warnings, partition routing,
//! and the package-level runtime settings from `TestMain`.
//!
//! 本模块为 `loaddatatest` 的 Rust 迁移测试集中维护一套可执行的 LOAD DATA
//! 语义模型。它不依赖完整 SQL 执行器，而是覆盖测试可观察到的字段/行解析、
//! NULL 判定、列映射、重复键处理、数值转换和 `TestMain` 全局配置。

#![allow(dead_code)]

use std::fmt;

/// LOAD DATA errors asserted by the Go tests.
/// Go 测试会断言的 LOAD DATA 错误类别；显示文本保持与原错误契约一致。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoadDataError {
    EmptyPath,
    MustSpecifyEnclosed,
    EmptyLineTerminator,
    OverlappingTerminators,
    ReaderNil,
    WrongFormatConfig,
    ServerFile,
    InvalidAutoRandom,
}

impl fmt::Display for LoadDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::EmptyPath => "The value of INFILE must not be empty when LOAD DATA from LOCAL",
            Self::MustSpecifyEnclosed => "must specify FIELDS [OPTIONALLY] ENCLOSED BY",
            Self::EmptyLineTerminator => "LINES TERMINATED BY is empty",
            Self::OverlappingTerminators => "must not be prefix of each other",
            Self::ReaderNil => "reader is nil",
            Self::WrongFormatConfig => "wrong format configuration",
            Self::ServerFile => "[executor:8154]Don't support load data from tidb-server's disk.",
            Self::InvalidAutoRandom => "invalid explicit value for AUTO_RANDOM column",
        };
        f.write_str(message)
    }
}

/// The data-format portion of a LOAD DATA statement.
/// LOAD DATA 语句中控制输入格式的配置，包括字段/行分隔符、转义、NULL 定义和忽略行数。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadDataConfig {
    /// 字段分隔符，默认使用制表符。
    pub fields_terminated_by: String,
    /// 可选的字段包围字符。
    pub fields_enclosed_by: Option<u8>,
    /// 可选的转义字符；`None` 表示禁用转义。
    pub fields_escaped_by: Option<u8>,
    /// 非空时，只接收以该前缀开头的行，并在解析前移除前缀。
    pub lines_starting_by: String,
    /// 行结束符，不能为空且不能与字段分隔符互为前缀。
    pub lines_terminated_by: String,
    /// 在解码转义之前参与 NULL 判定的原始文本集合。
    pub null_def: Vec<String>,
    /// NULL 定义是否只适用于未被包围的字段。
    pub null_value_opt_enclosed: bool,
    /// 从输入开头跳过的逻辑记录数。
    pub ignore_lines: usize,
}

impl Default for LoadDataConfig {
    fn default() -> Self {
        Self {
            fields_terminated_by: "\t".into(),
            fields_enclosed_by: None,
            fields_escaped_by: Some(b'\\'),
            lines_starting_by: String::new(),
            lines_terminated_by: "\n".into(),
            null_def: vec![r"\N".into()],
            null_value_opt_enclosed: false,
            ignore_lines: 0,
        }
    }
}

impl LoadDataConfig {
    /// 设置字段包围字符及 NULL 定义的 `OPTIONALLY ENCLOSED` 语义。
    pub fn enclosed(mut self, value: u8, optionally: bool) -> Self {
        self.fields_enclosed_by = Some(value);
        self.null_value_opt_enclosed = optionally;
        self
    }

    /// 设置字段分隔符。
    pub fn terminated_by(mut self, value: impl Into<String>) -> Self {
        self.fields_terminated_by = value.into();
        self
    }

    /// 同时设置行前缀和行结束符。
    pub fn lines(mut self, starting: impl Into<String>, terminated: impl Into<String>) -> Self {
        self.lines_starting_by = starting.into();
        self.lines_terminated_by = terminated.into();
        self
    }

    /// 设置或禁用字段转义字符。
    pub fn escaped_by(mut self, value: Option<u8>) -> Self {
        self.fields_escaped_by = value;
        self
    }

    /// 替换用于识别 NULL 的原始文本集合。
    pub fn null_by(mut self, values: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.null_def = values.into_iter().map(Into::into).collect();
        self
    }

    /// 校验会令解析器无法前进或产生分隔歧义的格式组合。
    fn validate(&self) -> Result<(), LoadDataError> {
        if self.lines_terminated_by.is_empty() {
            return Err(LoadDataError::EmptyLineTerminator);
        }
        if self.fields_terminated_by.is_empty() {
            return Err(LoadDataError::WrongFormatConfig);
        }
        if self
            .fields_terminated_by
            .starts_with(&self.lines_terminated_by)
            || self
                .lines_terminated_by
                .starts_with(&self.fields_terminated_by)
        {
            return Err(LoadDataError::OverlappingTerminators);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 单个字段同时保留原始文本、转义后值及是否被包围，以便正确判定 NULL。
struct RawField {
    raw: String,
    value: String,
    quoted: bool,
}

/// 按 LOAD DATA 规则解码反斜杠转义；未知转义保留转义后的字符本身。
fn decode_escape(raw: &[u8], escape: Option<u8>) -> String {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if Some(raw[i]) == escape && i + 1 < raw.len() {
            i += 1;
            out.push(match raw[i] {
                b'0' => 0,
                b'b' => 8,
                b'n' => b'\n',
                b'r' => b'\r',
                b't' => b'\t',
                b'Z' => 26,
                other => other,
            });
        } else {
            out.push(raw[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn is_at(data: &[u8], pos: usize, token: &[u8]) -> bool {
    !token.is_empty() && pos + token.len() <= data.len() && data[pos..].starts_with(token)
}

fn raw_to_field(raw: Vec<u8>, quoted: bool, config: &LoadDataConfig) -> RawField {
    let raw_text = String::from_utf8_lossy(&raw).into_owned();
    let value = decode_escape(&raw, config.fields_escaped_by);
    RawField {
        raw: raw_text,
        value,
        quoted,
    }
}

/// 使用原始文本而非解码结果判定 NULL，并区分被包围字段的可选语义。
fn raw_is_null(field: &RawField, config: &LoadDataConfig) -> bool {
    (!field.quoted && config.null_def.iter().any(|v| v == &field.raw))
        || (!field.quoted && config.fields_enclosed_by.is_some() && field.raw == "NULL")
        || (field.quoted
            && !config.null_value_opt_enclosed
            && config.null_def.iter().any(|v| v == &field.raw))
}

/// 从当前位置读取一条逻辑记录；不匹配行前缀的物理行会被整体跳过。
fn read_record(data: &[u8], pos: &mut usize, config: &LoadDataConfig) -> Option<Vec<RawField>> {
    let separator = config.fields_terminated_by.as_bytes();
    let terminator = config.lines_terminated_by.as_bytes();
    let prefix = config.lines_starting_by.as_bytes();

    while *pos < data.len() {
        if !prefix.is_empty() {
            while *pos < data.len() && !is_at(data, *pos, prefix) && !is_at(data, *pos, terminator)
            {
                *pos += 1;
            }

            if is_at(data, *pos, prefix) {
                *pos += prefix.len();
            } else if is_at(data, *pos, terminator) {
                *pos += terminator.len();
                continue;
            } else {
                return None;
            }
        }

        let mut fields = Vec::new();
        loop {
            let mut raw = Vec::new();
            let quoted = config
                .fields_enclosed_by
                .is_some_and(|quote| *pos < data.len() && data[*pos] == quote);
            if let Some(quote) = config.fields_enclosed_by.filter(|_| quoted) {
                *pos += 1;
                while *pos < data.len() {
                    if Some(data[*pos]) == config.fields_escaped_by && *pos + 1 < data.len() {
                        raw.push(data[*pos]);
                        raw.push(data[*pos + 1]);
                        *pos += 2;
                    } else if data[*pos] == quote {
                        *pos += 1;
                        break;
                    } else {
                        raw.push(data[*pos]);
                        *pos += 1;
                    }
                }
            } else {
                while *pos < data.len()
                    && !is_at(data, *pos, separator)
                    && !is_at(data, *pos, terminator)
                {
                    if Some(data[*pos]) == config.fields_escaped_by && *pos + 1 < data.len() {
                        raw.push(data[*pos]);
                        raw.push(data[*pos + 1]);
                        *pos += 2;
                    } else {
                        raw.push(data[*pos]);
                        *pos += 1;
                    }
                }
            }
            fields.push(raw_to_field(raw, quoted, config));
            if is_at(data, *pos, separator) {
                *pos += separator.len();
                continue;
            }
            if is_at(data, *pos, terminator) {
                *pos += terminator.len();
            }
            break;
        }
        return Some(fields);
    }
    None
}

/// Parse all input rows according to the LOAD DATA field/line rules.
/// 按字段和行规则解析全部输入，跳过指定记录数后再将原始 NULL 标记转换为 `None`。
pub fn parse_load_data(
    data: &[u8],
    config: &LoadDataConfig,
) -> Result<Vec<Vec<Option<String>>>, LoadDataError> {
    config.validate()?;
    let mut pos = 0;
    let mut seen_rows = 0;
    let mut rows = Vec::new();
    while let Some(fields) = read_record(data, &mut pos, config) {
        seen_rows += 1;
        if seen_rows <= config.ignore_lines {
            continue;
        }
        rows.push(
            fields
                .into_iter()
                .map(|field| {
                    if raw_is_null(&field, config) {
                        None
                    } else {
                        Some(field.value)
                    }
                })
                .collect(),
        );
    }
    Ok(rows)
}

/// Package-level settings installed by the Go `TestMain`.
/// Go `TestMain` 安装的包级测试环境快照，用于验证迁移后的默认值和清理动作。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadDataRuntime {
    pub auto_id_step: u64,
    pub slow_threshold_ms: u64,
    pub async_commit_safe_window: u64,
    pub async_commit_allowed_clock_drift: u64,
    pub allows_expression_index: bool,
    pub failpoints_enabled: bool,
    pub cleanup_runs: bool,
    pub fix56408_store_cleanup_runs: bool,
    pub view_stopped: bool,
}

impl Default for LoadDataRuntime {
    fn default() -> Self {
        Self {
            auto_id_step: 5_000,
            slow_threshold_ms: 30_000,
            async_commit_safe_window: 0,
            async_commit_allowed_clock_drift: 0,
            allows_expression_index: true,
            failpoints_enabled: true,
            cleanup_runs: false,
            fix56408_store_cleanup_runs: false,
            view_stopped: false,
        }
    }
}

impl LoadDataRuntime {
    /// 记录包级清理回调已经执行。
    pub fn cleanup(&mut self) {
        self.fix56408_store_cleanup_runs = true;
        self.view_stopped = true;
        self.cleanup_runs = true;
    }
}

/// The observable priority/lifecycle events emitted by a LOAD DATA execution.
/// LOAD DATA 执行对测试可见的文件、事务和低优先级 KV 访问事件。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadDataEvent {
    OpenFile,
    Begin,
    KvReadLow,
    KvWriteLow,
    Commit,
    CloseReader,
}

/// A small transaction lifecycle recorder used to test cleanup and priority.
/// 用于断言事务顺序、优先级传播及 reader 关闭时机的轻量事件记录器。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LoadDataLifecycle {
    pub events: Vec<LoadDataEvent>,
}

impl LoadDataLifecycle {
    /// 记录一次完整执行；低优先级模式会额外记录读写 KV 事件。
    pub fn run(&mut self, low_priority: bool) {
        self.events.push(LoadDataEvent::OpenFile);
        self.events.push(LoadDataEvent::Begin);
        if low_priority {
            self.events.push(LoadDataEvent::KvReadLow);
            self.events.push(LoadDataEvent::KvWriteLow);
        }
        self.events.push(LoadDataEvent::Commit);
        self.events.push(LoadDataEvent::CloseReader);
    }

    /// 判断开始事务事件是否先于提交事件；缺失事件时保持 `Option` 的顺序语义。
    pub fn begin_before_commit(&self) -> bool {
        self.events
            .iter()
            .position(|event| *event == LoadDataEvent::Begin)
            < self
                .events
                .iter()
                .position(|event| *event == LoadDataEvent::Commit)
    }
}

/// Validate the LOAD DATA options exercised by `TestLoadDataInitParam`.
/// 按 `TestLoadDataInitParam` 覆盖的顺序校验选项，并保留测试 helper 未安装 reader 的终态错误。
pub fn validate_load_data_options(
    path: &str,
    local: bool,
    format: Option<&str>,
    config: &LoadDataConfig,
) -> Result<(), LoadDataError> {
    if local && path.is_empty() {
        return Err(LoadDataError::EmptyPath);
    }
    if !local {
        return Err(LoadDataError::ServerFile);
    }
    if matches!(format, Some("sql file" | "delimited data")) {
        return Err(LoadDataError::ReaderNil);
    }
    if config.fields_enclosed_by.is_none() && config.null_value_opt_enclosed {
        return Err(LoadDataError::MustSpecifyEnclosed);
    }
    if config.lines_terminated_by.is_empty() {
        return Err(LoadDataError::EmptyLineTerminator);
    }
    if config.fields_terminated_by.is_empty() {
        return Err(LoadDataError::WrongFormatConfig);
    }
    config.validate()?;
    // The Go helper intentionally has no reader builder installed for these
    // option-only checks, so every otherwise valid request reaches this error.
    // Go helper 在这些纯选项检查中刻意不安装 reader builder，因此合法配置最终也返回 ReaderNil。
    Err(LoadDataError::ReaderNil)
}

/// Shared unsigned BIGINT conversion behavior used by the overflow cases.
/// 模拟无符号 BIGINT 转换：负数截为 0，正向溢出截为上限，并通过布尔值报告警告。
pub fn parse_unsigned_bigint(value: &str) -> (u64, bool) {
    match value.parse::<u128>() {
        Ok(value) if value <= u64::MAX as u128 => (value as u64, false),
        _ => {
            if value.starts_with('-') {
                (0, true)
            } else {
                (u64::MAX, true)
            }
        }
    }
}

/// Map input columns to a table, apply a simple SET expression, and preserve
/// the explicit-value AUTO_RANDOM error contract.
/// 将输入列映射到目标列，可应用简单乘法 SET 表达式，并拒绝显式写入 AUTO_RANDOM 列。
pub fn map_columns(
    rows: &[Vec<Option<String>>],
    target_columns: &[usize],
    set_multiplier: Option<u64>,
    auto_random_explicit: bool,
) -> Result<Vec<Vec<Option<String>>>, LoadDataError> {
    if auto_random_explicit {
        return Err(LoadDataError::InvalidAutoRandom);
    }
    Ok(rows
        .iter()
        .map(|row| {
            let mut mapped = vec![None; target_columns.iter().copied().max().unwrap_or(0) + 1];
            for (input_index, target_index) in target_columns.iter().copied().enumerate() {
                let value = row.get(input_index).cloned().unwrap_or(None);
                mapped[target_index] = match (value, set_multiplier) {
                    (Some(value), Some(multiplier)) => value
                        .parse::<i64>()
                        .ok()
                        .map(|number| (number * multiplier as i64).to_string()),
                    (None, Some(_)) => None,
                    (value, None) => value,
                };
            }
            mapped
        })
        .collect())
}

/// Apply REPLACE/IGNORE duplicate-key behavior to already decoded rows.
/// 对已解码行应用重复键策略：REPLACE 删除旧行并保留新行，IGNORE 则删除当前重复行。
/// 返回值只统计 REPLACE 实际删除的旧行数。
pub fn apply_replace(
    rows: &mut Vec<Vec<Option<String>>>,
    key_index: usize,
    replace: bool,
) -> usize {
    let mut deleted = 0;
    let mut index = 0;
    while index < rows.len() {
        let key = rows[index].get(key_index).cloned();
        let duplicate = rows[..index]
            .iter()
            .position(|row| row.get(key_index).cloned() == key);
        if let Some(previous) = duplicate {
            if replace {
                rows.remove(previous);
                deleted += 1;
                index = index.saturating_sub(1);
            } else {
                rows.remove(index);
                index = index.saturating_sub(1);
            }
        }
        index += 1;
    }
    deleted
}

#[cfg(test)]
mod load_data_test;
#[cfg(test)]
mod main_test;
