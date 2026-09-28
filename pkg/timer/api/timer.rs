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

// 定时器领域模型与调度策略。
//
// 定义定时器规格（TimerSpec）、记录（TimerRecord）、手动触发请求、
// 事件扩展字段，以及 INTERVAL / CRON 两类调度策略的下次触发时间计算。
// Watermark 表示上次成功调度水位；Location 将时间换算到指定时区。

use crate::error::{TimerError, TimerResult};
use chrono::{DateTime, FixedOffset, Local, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use std::ops::{Deref, DerefMut};
use std::str::FromStr;
use std::time::Duration;

#[path = "../../parser/duration/duration.rs"]
mod parser_duration;
use parser_duration::ParseDuration;

/// 带固定偏移的时间戳类型（对应 Go 的 time.Time）。
pub type Timestamp = DateTime<FixedOffset>;
/// 调度策略类型字符串别名（INTERVAL / CRON）。
pub type SchedPolicyType = str;

/// 按固定间隔触发的调度策略类型名。
pub const SchedEventInterval: &str = "INTERVAL";
/// 按 Cron 表达式触发的调度策略类型名。
pub const SchedEventCron: &str = "CRON";

/// 调度事件策略：根据水位（watermark）计算下一次事件时间。
pub trait SchedEventPolicy: Send + Sync {
    /// 返回 (下次事件时间, 是否成功算出)；无水位时行为由具体策略决定。
    fn NextEventTime(&self, watermark: Option<Timestamp>) -> (Option<Timestamp>, bool);
}

/// 固定间隔调度策略：在水位上叠加 interval。
pub struct SchedIntervalPolicy {
    /// 原始间隔表达式（如 `1h`）。
    pub expr: String,
    /// 解析后的标准库 Duration。
    pub interval: Duration,
}

/// 解析间隔表达式并构造 `SchedIntervalPolicy`；支持 failpoint 覆盖间隔。
pub fn NewSchedIntervalPolicy(expr: String) -> TimerResult<SchedIntervalPolicy> {
    let interval = ParseDuration(&expr).map_err(|err| {
        TimerError::message(format!("invalid schedule event expr '{}': {}", expr, err))
    })?;
    // failpoint：测试中可强制覆盖 TTL/定时任务间隔为指定纳秒。
    fail::fail_point!("overwrite-ttl-job-interval", |value| {
        let nanos = value
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_default();
        Ok(SchedIntervalPolicy {
            expr: expr.clone(),
            interval: Duration::from_nanos(nanos),
        })
    });
    Ok(SchedIntervalPolicy { expr, interval })
}

impl SchedEventPolicy for SchedIntervalPolicy {
    fn NextEventTime(&self, watermark: Option<Timestamp>) -> (Option<Timestamp>, bool) {
        // 无水位时无法推算下次触发点，返回 (None, true) 表示“尚无结果但策略有效”。
        let Some(watermark) = watermark else {
            return (None, true);
        };
        let Ok(interval) = chrono::Duration::from_std(self.interval) else {
            return (None, false);
        };
        (watermark.checked_add_signed(interval), true)
    }
}

/// Cron 调度策略，内部持有已解析的 cron Schedule。
pub struct CronPolicy {
    schedule: Schedule,
}

/// 将标准五段 Cron 表达式规范化后构造 `CronPolicy`。
pub fn NewCronPolicy(expr: String) -> TimerResult<CronPolicy> {
    // robfig/cron ParseStandard accepts five fields. The Rust cron crate also
    // models seconds and numbers Sunday as 1 instead of 0, so translate the
    // standard Go day-of-week field before prepending the zero-second field.
    // 对齐 Go robfig/cron：五段标准式 → 前置秒字段，并把周日编号从 0 平移到 1。
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() == 5 && cron_day_contains_out_of_range_value(fields[4]) {
        return Err(TimerError::message(format!(
            "invalid cron expr '{}': day-of-week value must be between 0 and 6",
            expr
        )));
    }
    let normalized = normalize_standard_cron(&expr);
    let schedule = Schedule::from_str(&normalized)
        .map_err(|err| TimerError::message(format!("invalid cron expr '{}': {}", expr, err)))?;
    Ok(CronPolicy { schedule })
}

fn cron_day_contains_out_of_range_value(field: &str) -> bool {
    field.split(',').any(|part| {
        let base = part.split_once('/').map_or(part, |(base, _)| base);
        base.split(|ch: char| !ch.is_ascii_digit())
            .filter(|token| !token.is_empty())
            .any(|token| token.parse::<u8>().is_ok_and(|day| day > 6))
    })
}

/// 将五段标准 Cron 转为 Rust cron crate 可用的六段（秒 + 其余），并平移周日编号。
fn normalize_standard_cron(expr: &str) -> String {
    let mut fields: Vec<String> = expr.split_whitespace().map(str::to_string).collect();
    if fields.len() != 5 {
        return expr.to_string();
    }
    fields[4] = shift_cron_day_of_week(&fields[4]);
    format!("0 {}", fields.join(" "))
}

/// 把 day-of-week 字段中的数字 0..=6 加一，以匹配 Rust cron 以 1 表示周日。
fn shift_cron_day_of_week(field: &str) -> String {
    field
        .split(',')
        .map(|part| {
            if let Some((base, step)) = part.split_once('/') {
                // The step is a distance, so it must not be shifted with the
                // weekday values (for example `*/2` remains every two days).
                format!("{}/{}", shift_cron_day_tokens(base), step)
            } else {
                shift_cron_day_tokens(part)
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Shift numeric weekday values and range endpoints from 0-based to 1-based.
fn shift_cron_day_tokens(field: &str) -> String {
    let mut result = String::with_capacity(field.len());
    let mut chars = field.char_indices().peekable();
    while let Some((start, ch)) = chars.next() {
        if !ch.is_ascii_digit() {
            result.push(ch);
            continue;
        }
        // 聚合连续数字 token，仅对 0..=6 做 +1 平移。
        let mut end = start + ch.len_utf8();
        while let Some((index, next)) = chars.peek().copied() {
            if !next.is_ascii_digit() {
                break;
            }
            chars.next();
            end = index + next.len_utf8();
        }
        let token = &field[start..end];
        match token.parse::<u8>() {
            Ok(day @ 0..=6) => result.push_str(&(day + 1).to_string()),
            _ => result.push_str(token),
        }
    }
    result
}

impl SchedEventPolicy for CronPolicy {
    fn NextEventTime(&self, watermark: Option<Timestamp>) -> (Option<Timestamp>, bool) {
        let Some(watermark) = watermark else {
            return (None, false);
        };
        let next = self.schedule.after(&watermark).next();
        let ok = next.is_some();
        (next, ok)
    }
}

/// 手动触发请求：携带请求 ID、时间、超时与处理状态。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ManualRequest {
    /// 手动请求唯一标识。
    pub ManualRequestID: String,
    /// 发起手动请求的时间。
    pub ManualRequestTime: Option<Timestamp>,
    /// 手动请求超时时长。
    pub ManualTimeout: Duration,
    /// 是否已处理完毕。
    pub ManualProcessed: bool,
    /// 处理完成后关联的事件 ID。
    pub ManualEventID: String,
}

impl ManualRequest {
    /// 是否仍在等待处理：有请求 ID 且尚未标记 processed。
    pub fn IsManualRequesting(&self) -> bool {
        !self.ManualRequestID.is_empty() && !self.ManualProcessed
    }

    /// 标记已处理并绑定触发产生的 eventID，返回新副本。
    pub fn SetProcessed(&self, eventID: String) -> ManualRequest {
        let mut result = self.clone();
        result.ManualProcessed = true;
        result.ManualEventID = eventID;
        result
    }
}

/// 调度事件附加信息：关联手动请求与事件水位。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EventExtra {
    /// 触发本事件的手动请求 ID。
    pub EventManualRequestID: String,
    /// 事件对应的水位时间。
    pub EventWatermark: Option<Timestamp>,
}

/// 定时器规格：命名空间、键、标签、时区、调度策略与 Hook 类等可配置字段。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TimerSpec {
    /// 命名空间，用于隔离不同业务的定时器。
    pub Namespace: String,
    /// 定时器键（业务唯一名）。
    pub Key: String,
    /// 业务标签列表。
    pub Tags: Vec<String>,
    /// 用户自定义二进制数据。
    pub Data: Vec<u8>,
    /// 时区名或数值偏移（如 `Asia/Shanghai`、`+0800`）。
    pub TimeZone: String,
    /// 调度策略类型（INTERVAL / CRON）。
    pub SchedPolicyType: String,
    /// 调度策略表达式。
    pub SchedPolicyExpr: String,
    /// Hook 类名，运行时据此查找回调。
    pub HookClass: String,
    /// 调度水位：上次成功调度的时间基准。
    pub Watermark: Option<Timestamp>,
    /// 是否启用；禁用时不下一次事件时间。
    pub Enable: bool,
}

impl TimerSpec {
    /// 浅拷贝包装，对应 Go 的 Clone 方法名。
    pub fn Clone(&self) -> TimerSpec {
        self.clone()
    }

    /// 校验必填字段、时区与调度策略配置是否合法。
    pub fn Validate(&self) -> TimerResult<()> {
        if self.Namespace.is_empty() {
            return Err(TimerError::message("field 'Namespace' should not be empty"));
        }
        if self.Key.is_empty() {
            return Err(TimerError::message("field 'Key' should not be empty"));
        }
        ValidateTimeZone(&self.TimeZone)?;
        if self.SchedPolicyType.is_empty() {
            return Err(TimerError::message(
                "field 'SchedPolicyType' should not be empty",
            ));
        }
        // 尝试创建策略以验证类型与表达式组合。
        self.CreateSchedEventPolicy().map_err(|err| {
            TimerError::message(format!("schedule event configuration is not valid: {err}"))
        })?;
        Ok(())
    }

    /// 按本规格的策略类型与表达式创建调度策略实例。
    pub fn CreateSchedEventPolicy(&self) -> TimerResult<Box<dyn SchedEventPolicy>> {
        CreateSchedEventPolicy(&self.SchedPolicyType, self.SchedPolicyExpr.clone())
    }
}

/// 按策略类型工厂创建 INTERVAL 或 CRON 策略；未知类型报错。
pub fn CreateSchedEventPolicy(
    policy_type: &str,
    expr: String,
) -> TimerResult<Box<dyn SchedEventPolicy>> {
    match policy_type {
        SchedEventInterval => Ok(Box::new(NewSchedIntervalPolicy(expr)?)),
        SchedEventCron => Ok(Box::new(NewCronPolicy(expr)?)),
        _ => Err(TimerError::message(format!(
            "invalid schedule event type: '{}'",
            policy_type
        ))),
    }
}

/// 调度事件状态字符串别名（IDLE / TRIGGER）。
pub type SchedEventStatus = str;
/// 空闲状态：未处于触发流程中。
pub const SchedEventIdle: &str = "IDLE";
/// 触发中状态：事件已开始执行。
pub const SchedEventTrigger: &str = "TRIGGER";

/// 定时器时区位置：命名时区（IANA）或固定偏移。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TimerLocation {
    /// 命名时区，如 Asia/Shanghai。
    Named(Tz),
    /// 固定 UTC 偏移。
    Fixed(FixedOffset),
}

/// 定时器持久化记录：规格 + 运行时事件字段与版本号。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TimerRecord {
    /// 嵌入的定时器规格。
    pub TimerSpec: TimerSpec,
    /// 全局唯一定时器 ID。
    pub ID: String,
    /// 当前手动触发请求状态。
    pub ManualRequest: ManualRequest,
    /// 当前事件状态（IDLE / TRIGGER）。
    pub EventStatus: String,
    /// 当前事件 ID。
    pub EventID: String,
    /// 当前事件携带的数据。
    pub EventData: Vec<u8>,
    /// 当前事件开始时间。
    pub EventStart: Option<Timestamp>,
    /// 事件扩展字段。
    pub EventExtra: EventExtra,
    /// 摘要二进制数据。
    pub SummaryData: Vec<u8>,
    /// 创建时间。
    pub CreateTime: Option<Timestamp>,
    /// 乐观并发版本号；更新时用于 CAS 校验。
    pub Version: u64,
    /// 解析后的时区位置缓存。
    pub Location: Option<TimerLocation>,
}

impl Deref for TimerRecord {
    type Target = TimerSpec;

    fn deref(&self) -> &Self::Target {
        &self.TimerSpec
    }
}

impl DerefMut for TimerRecord {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.TimerSpec
    }
}

impl TimerRecord {
    /// 计算下一次事件时间；禁用时返回 (None, false)。
    ///
    /// CRON + 命名时区时先换到该时区再求下次触发点，避免夏令时偏移误差。
    pub fn NextEventTime(&self) -> TimerResult<(Option<Timestamp>, bool)> {
        if !self.Enable {
            return Ok((None, false));
        }
        // 将水位换算到定时器 Location 所在时区。
        let watermark = self
            .Watermark
            .as_ref()
            .map(|value| in_location(value, self.Location.as_ref()));
        if self.SchedPolicyType == SchedEventCron {
            let policy = NewCronPolicy(self.SchedPolicyExpr.clone())?;
            // 命名时区路径：用带时区的时间喂给 cron Schedule。
            if let (Some(watermark), Some(TimerLocation::Named(tz))) =
                (watermark.as_ref(), self.Location.as_ref())
            {
                let zoned = watermark.with_timezone(tz);
                let next = policy
                    .schedule
                    .after(&zoned)
                    .next()
                    .map(|value| value.fixed_offset());
                let ok = next.is_some();
                return Ok((next, ok));
            }
            return Ok(policy.NextEventTime(watermark));
        }
        let policy = self.CreateSchedEventPolicy()?;
        Ok(policy.NextEventTime(watermark))
    }

    /// 浅拷贝包装，对应 Go 的 Clone 方法名。
    pub fn Clone(&self) -> TimerRecord {
        self.clone()
    }

    /// 校验嵌入的 TimerSpec。
    pub fn Validate(&self) -> TimerResult<()> {
        self.TimerSpec.Validate()
    }
}

/// 校验时区字符串：空串合法；否则须能被 `parse_location` 解析。
pub fn ValidateTimeZone(tz: &str) -> TimerResult<()> {
    if tz.is_empty() {
        return Ok(());
    }
    parse_location(tz).map(|_| ())
}

/// 解析时区：空串用本地偏移；否则尝试 IANA 名或 `±HHMM` 数值偏移。
pub fn parse_location(tz: &str) -> TimerResult<TimerLocation> {
    if tz.is_empty() {
        return Ok(TimerLocation::Fixed(*Local::now().offset()));
    }
    if tz.eq_ignore_ascii_case("SYSTEM") {
        return Ok(TimerLocation::Fixed(*Local::now().offset()));
    }
    if let Ok(named) = tz.parse::<Tz>() {
        return Ok(TimerLocation::Named(named));
    }
    if let Some(offset) = parse_numeric_offset(tz) {
        return Ok(TimerLocation::Fixed(offset));
    }
    Err(TimerError::message(format!(
        "Unknown or incorrect time zone: '{}'",
        tz
    )))
}

/// 解析 `+0800` / `+08:00` / `-6:00` 数值时区偏移。
fn parse_numeric_offset(value: &str) -> Option<FixedOffset> {
    let sign = *value.as_bytes().first()?;
    if !matches!(sign, b'+' | b'-') {
        return None;
    }
    let magnitude = &value[1..];
    let (hours, minutes): (i32, i32) = if let Some((hours, minutes)) = magnitude.split_once(':') {
        if hours.is_empty() || hours.len() > 2 || minutes.len() != 2 {
            return None;
        }
        (hours.parse().ok()?, minutes.parse().ok()?)
    } else {
        if magnitude.len() != 4 {
            return None;
        }
        (magnitude[..2].parse().ok()?, magnitude[2..].parse().ok()?)
    };
    if minutes > 59
        || (sign == b'+' && (hours > 14 || (hours == 14 && minutes > 0)))
        || (sign == b'-' && hours > 12)
    {
        return None;
    }
    let seconds = (hours * 60 + minutes) * 60;
    if sign == b'-' {
        FixedOffset::west_opt(seconds)
    } else {
        FixedOffset::east_opt(seconds)
    }
}

/// 当前 UTC 时间转为 FixedOffset 时间戳。
pub fn now_timestamp() -> Timestamp {
    Utc::now().fixed_offset()
}

/// 将时间戳转换到指定 Location；无 Location 时原样返回。
pub fn in_location(value: &Timestamp, location: Option<&TimerLocation>) -> Timestamp {
    match location {
        Some(TimerLocation::Named(tz)) => value.with_timezone(tz).fixed_offset(),
        Some(TimerLocation::Fixed(offset)) => value.with_timezone(offset),
        None => *value,
    }
}
