// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// #nosec G404

//! 本模块负责把列约束转成日期、时间、时间戳和年份字符串。
//! 生成顺序始终是“优先直方图，其次显式边界，最后默认范围”。
//! 这样做可以最大程度复用统计信息，同时保留 Go 的兜底策略。
//! 日期类函数返回的都是已经格式化好的字符串，而不是时间对象。
//! 这与 importer 下游直接拼接 SQL 的工作方式一致。
//! 缺少最小值时，日期和时间戳会退回当前年份附近的随机结果。
//! 只有给出最小值但不给最大值时，Go 会使用一年窗口近似采样。
//! 时间类型的默认兜底只关注一天中的时分秒，不借用当前日期。
//! YEAR 的兜底范围最保守，只在最近若干年间回退。
//! 所有格式常量都保留 Go 命名，便于 parity test 直接引用。
//! 解析失败时模块不会主动终止流程，而是记录警告后回落到零值时间。
//! 这些注释的重点是说明取样来源和边界，而不是随机算法实现细节。
//! 本次改动不会改变随机种子、范围闭区间语义或输出格式。
//! 如果未来接入更多统计类型，应继续保持“统计优先”的决策顺序。
//! 这样用户在导入已有分布数据时才能获得最接近 Go 的结果形状。

use crate::data::{randInt, randInt64, randString};
use crate::parser::column;
use crate::stubs::{
    self, CivilTime, YEAR_FORMAT, date_format_go, datetime_format_go, time_format_go,
};

pub use crate::data::{alphabet, letterIdxBits, letterIdxMask, letterIdxMax};

// `yearFormat` 保留 Go 对照中的常量名与默认语义。
// 调用方依赖它维持跨语言配置、格式或类型判断的一致性。
pub const yearFormat: &str = YEAR_FORMAT;
// `dateFormat` 明确复用 Go 的日期字符串布局。
// 这里保留字面量常量，便于和下游 SQL 文本拼接逻辑直接对齐。
pub const dateFormat: &str = "2006-01-02";
// `timeFormat` 保留 Go 对照中的常量名与默认语义。
// 调用方依赖它维持跨语言配置、格式或类型判断的一致性。
pub const timeFormat: &str = "15:04:05";
// `dateTimeFormat` 把日期和时间的输出契约绑定在同一布局上。
// 这样 histogram 路径与边界采样路径都能产出完全一致的文本格式。
pub const dateTimeFormat: &str = "2006-01-02 15:04:05";

fn go_zero_time() -> CivilTime {
    CivilTime {
        year: 1,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
    }
}

/// `randDate` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn randDate(col: &column) -> String {
    // 有直方图时直接按统计桶采样，优先保留原始数据分布。
    if let Some(hist) = col.hist.as_ref() {
        return hist.randDate("DAY", "%Y-%m-%d", dateFormat);
    }

    let minv = col.min.clone();
    let maxv = col.max.clone();
    if minv.is_empty() {
        let year = CivilTime::now().year;
        let month = randInt(1, 12);
        let day = randInt(1, 28);
        return format!("{:04}-{:02}-{:02}", year, month, day);
    }

    let minTime = match stubs::parse_date(&minv) {
        Ok(t) => t,
        Err(err) => {
            stubs::log_warn(format!("parse min date failed: {err}"));
            go_zero_time()
        }
    };
    if maxv.is_empty() {
        let t = minTime.add_days(randInt(0, 365));
        return format!("{:04}-{:02}-{:02}", t.year, t.month, t.day);
    }

    let maxTime = match stubs::parse_date(&maxv) {
        Ok(t) => t,
        Err(err) => {
            stubs::log_warn(format!("parse max date failed: {err}"));
            go_zero_time()
        }
    };
    let days = (stubs::timestamp_diff("DAY", &minTime, &maxTime)) as i32;
    let t = minTime.add_days(randInt(0, days));
    format!("{:04}-{:02}-{:02}", t.year, t.month, t.day)
}

/// `randTime` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn randTime(col: &column) -> String {
    // 时间类型同样优先走直方图，以免丢失高峰时段分布。
    if let Some(hist) = col.hist.as_ref() {
        return hist.randDate("SECOND", "%H:%i:%s", timeFormat);
    }
    let minv = col.min.clone();
    let maxv = col.max.clone();
    // 任一边界缺失都退化为全天随机时间，这与 Go 的保守兜底一致。
    if minv.is_empty() || maxv.is_empty() {
        let hour = randInt(0, 23);
        let minute = randInt(0, 59);
        let sec = randInt(0, 59);
        return format!("{:02}:{:02}:{:02}", hour, minute, sec);
    }

    let minTime = match stubs::parse_time_of_day(&minv) {
        Ok(t) => t,
        Err(err) => {
            stubs::log_warn(format!("parse min time failed: {err}"));
            go_zero_time()
        }
    };
    let maxTime = match stubs::parse_time_of_day(&maxv) {
        Ok(t) => t,
        Err(err) => {
            stubs::log_warn(format!("parse max time failed: {err}"));
            go_zero_time()
        }
    };
    let seconds = stubs::timestamp_diff("SECOND", &minTime, &maxTime) as i32;
    let t = minTime.add_seconds(randInt(0, seconds) as i64);
    format!("{:02}:{:02}:{:02}", t.hour, t.minute, t.second)
}

/// `randTimestamp` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn randTimestamp(col: &column) -> String {
    // 时间戳优先复用 histogram，避免随机落点破坏已有秒级分布。
    if let Some(hist) = col.hist.as_ref() {
        return hist.randDate("SECOND", "%Y-%m-%d %H:%i:%s", dateTimeFormat);
    }
    let minv = col.min.clone();
    let maxv = col.max.clone();
    if minv.is_empty() {
        let year = CivilTime::now().year;
        let month = randInt(1, 12);
        let day = randInt(1, 28);
        let hour = randInt(0, 23);
        let minute = randInt(0, 59);
        let sec = randInt(0, 59);
        return format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            year, month, day, hour, minute, sec
        );
    }

    let minTime = match stubs::parse_datetime(&minv) {
        Ok(t) => t,
        Err(err) => {
            stubs::log_warn(format!("parse min timestamp failed: {err}"));
            go_zero_time()
        }
    };
    if maxv.is_empty() {
        let t = minTime.add_days(randInt(0, 365));
        return format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            t.year, t.month, t.day, t.hour, t.minute, t.second
        );
    }

    let maxTime = match stubs::parse_datetime(&maxv) {
        Ok(t) => t,
        Err(err) => {
            stubs::log_warn(format!("parse max timestamp failed: {err}"));
            go_zero_time()
        }
    };
    let seconds = stubs::timestamp_diff("SECOND", &minTime, &maxTime);
    let t = minTime.add_seconds(randInt64(0, seconds));
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

/// `randYear` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn randYear(col: &column) -> String {
    // YEAR 桶最粗，因此只要统计信息存在就优先采用统计采样。
    if let Some(hist) = col.hist.as_ref() {
        return hist.randDate("YEAR", "%Y", yearFormat);
    }
    let minv = col.min.clone();
    let maxv = col.max.clone();
    if minv.is_empty() || maxv.is_empty() {
        return format!("{:04}", CivilTime::now().year - randInt(0, 10));
    }

    let minTime = match stubs::parse_year(&minv) {
        Ok(t) => t,
        Err(err) => {
            stubs::log_warn(format!("parse min year failed: {err}"));
            go_zero_time()
        }
    };
    let maxTime = match stubs::parse_year(&maxv) {
        Ok(t) => t,
        Err(err) => {
            stubs::log_warn(format!("parse max year failed: {err}"));
            go_zero_time()
        }
    };
    let seconds = stubs::timestamp_diff("SECOND", &minTime, &maxTime);
    let t = minTime.add_seconds(randInt64(0, seconds));
    format!("{:04}", t.year)
}

// Silence unused imports that document Go format constants.
#[allow(dead_code)]
// `_fmt_consts` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn _fmt_consts() {
    let _ = (
        date_format_go(),
        time_format_go(),
        datetime_format_go(),
        randInt64(0, 1),
        randString(1),
    );
}
