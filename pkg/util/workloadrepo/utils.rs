// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 工作负载仓库分区命名与保留期等工具函数。
//
// 对应 Go `pkg/util/workloadrepo/utils.go`。按日期生成 MySQL RANGE 分区
// （`PARTITION BY RANGE(TO_DAYS(...))`）定义、解析分区名，并校验保留天数与
// 仓库目标（dest）配置。

use crate::worker::worker as Worker;
use crate::*;
use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone};

/// 在 CREATE TABLE 语句末尾追加按日 RANGE 分区子句。
pub fn generatePartitionDef(
    output: &mut String,
    column: &str,
    now: DateTime<Local>,
) -> Result<(), String> {
    output.push_str(&format!(" PARTITION BY RANGE( TO_DAYS({column}) ) ("));
    // generatePartitionRanges 返回 true 表示没有新分区可加，视为失败。
    if generatePartitionRanges(output, &[], now)? {
        return Err("could not generate partition ranges".into());
    }
    output.push(')');
    Ok(())
}

/// 将日期格式化为分区名，形如 `p20260727`。
pub fn generatePartitionName(time: DateTime<Local>) -> String {
    format!("p{}", time.format("%Y%m%d"))
}

/// 解析 `pYYYYMMDD` 分区名为本地零点时间。
pub fn parsePartitionName(part: &str) -> Result<DateTime<Local>, String> {
    let mut rest = part;
    let mut numbers = Vec::with_capacity(3);
    for (layout, width) in [("p", 1), ("2006", 4), ("01", 2), ("02", 2)] {
        let valid = rest.get(..width).filter(|value| {
            if layout == "p" {
                *value == "p"
            } else {
                value.bytes().all(|b| b.is_ascii_digit())
            }
        });
        let Some(value) = valid else {
            return Err(format!(
                "parsing time {part:?} as \"p20060102\": cannot parse {rest:?} as {layout:?}"
            ));
        };
        if layout != "p" {
            let number = value.parse::<u32>().unwrap();
            if layout == "01" && !(1..=12).contains(&number) {
                return Err(format!("parsing time {part:?}: month out of range"));
            }
            numbers.push(number);
        }
        rest = &rest[width..];
    }
    if !rest.is_empty() {
        return Err(format!("parsing time {part:?}: extra text: {rest:?}"));
    }
    let date = NaiveDate::from_ymd_opt(numbers[0] as i32, numbers[1], numbers[2])
        .ok_or_else(|| format!("parsing time {part:?}: day out of range"))?;
    Ok(local_midnight(date))
}

/// Match Go time.Date's two UTC-offset lookups, including missing/repeated midnight.
fn local_midnight(date: NaiveDate) -> DateTime<Local> {
    let unix = date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();
    let initial = Local
        .timestamp_opt(unix, 0)
        .unwrap()
        .offset()
        .local_minus_utc();
    let offset = Local
        .timestamp_opt(unix - i64::from(initial), 0)
        .unwrap()
        .offset()
        .local_minus_utc();
    Local.timestamp_opt(unix - i64::from(offset), 0).unwrap()
}

/// 向 `output` 追加相对 `now` 未来 1～2 天的缺失分区定义。
///
/// 返回 `true` 表示所需分区均已存在（未写入任何新定义）。
pub fn generatePartitionRanges(
    output: &mut String,
    existing: &[String],
    now: DateTime<Local>,
) -> Result<bool, String> {
    let mut lastPart = local_midnight(now.date_naive());
    // 以已有最晚分区为基准，避免重复创建更早的分区。
    if let Some(name) = existing.last() {
        let date = parsePartitionName(name)?;
        if date > lastPart {
            lastPart = date;
        }
    }
    let mut allExisted = true;
    for days in [1, 2] {
        let date = now
            .date_naive()
            .checked_add_signed(Duration::days(days))
            .unwrap();
        let time = local_midnight(date);
        if time > lastPart {
            if !allExisted {
                output.push_str(", ");
            }
            output.push_str(&format!(
                "PARTITION {} VALUES LESS THAN (TO_DAYS('{}'))",
                generatePartitionName(time),
                time.format("%Y-%m-%d")
            ));
            allExisted = false;
        }
    }
    Ok(allExisted)
}

impl Worker {
    /// 与 Go hook 一致：解析机器整数后转为 int32；范围由 SQL sysvar 层校验。
    pub fn setRetentionDays(&self, value: &str) -> Result<(), String> {
        let value = value.parse::<isize>().map_err(|_| {
            format!("Variable '{repositoryRetentionDays}' can't be set to the value of '{value}'")
        })?;
        self.state.lock().unwrap().retentionDays = value as i32;
        Ok(())
    }
}

/// 校验仓库目标配置：仅允许空串或 `"table"`（大小写不敏感）。
pub fn validateDest(orig: &str) -> Result<String, String> {
    let value = orig
        .chars()
        .map(|c| c.to_lowercase().next().unwrap())
        .collect::<String>();
    if value.is_empty() || value == "table" {
        Ok(value)
    } else {
        Err(format!(
            "Variable '{}' can't be set to the value of '{}': valid values are '' and 'table'",
            repositoryDest.chars().take(64).collect::<String>(),
            value.chars().take(200).collect::<String>()
        ))
    }
}
