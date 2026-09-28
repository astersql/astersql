// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 唯一值生成器 `Datum`：按步长产出不重复的整数、浮点、字符串与时间类型值。
//
// 用于导入工具在唯一索引/主键列上生成确定性递增数据，避免随机碰撞。
// 内部用 `Mutex` 保护状态；时间序列以 Unix 秒为基准，经 civil 日换算输出日期字符串。

use crate::rand::{ALPHABET, civil_from_days, days_from_civil};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// `Datum` 内部可变状态：整型序列与时间序列。
struct DatumState {
    time_seconds: Option<i64>,
    int_value: i64,
    min_int_value: i64,
    max_int_value: i64,
    step: i64,
    initialized: bool,
    use_range: bool,
}

/// 列级唯一数据生成器；同一 `Datum` 可被多线程共享（内部加锁）。
pub struct Datum {
    state: Mutex<DatumState>,
}

impl Default for Datum {
    fn default() -> Self {
        Self::new()
    }
}

impl Datum {
    /// 创建未初始化的 `Datum`（整型从 -1 起步，等待 `set_init_int64_value`）。
    pub fn new() -> Self {
        Self {
            state: Mutex::new(DatumState {
                time_seconds: None,
                int_value: -1,
                min_int_value: 0,
                max_int_value: 0,
                step: 1,
                initialized: false,
                use_range: false,
            }),
        }
    }

    /// 首次初始化整型步长与可选区间；重复调用无效。
    pub fn set_init_int64_value(&self, step: i64, minimum: i64, maximum: i64) {
        let mut state = self.state.lock().unwrap();
        if state.initialized {
            return;
        }
        state.step = step;
        // minimum == -1 表示沿用默认起点，不覆盖当前 int_value。
        if minimum != -1 {
            state.min_int_value = minimum;
            state.int_value = minimum;
        }
        if minimum < maximum {
            state.max_int_value = maximum;
            state.use_range = true;
        }
        state.initialized = true;
    }

    /// 取当前整型唯一值并按 step 前进；若已达区间上界则返回当前值且不再递增。
    pub fn unique_i64(&self) -> i64 {
        let mut state = self.state.lock().unwrap();
        let value = state.int_value;
        if state.use_range && state.int_value.wrapping_add(state.step) > state.max_int_value {
            return value;
        }
        state.int_value = state.int_value.wrapping_add(state.step);
        value
    }

    /// 整型唯一值转 `f64`（用于 FLOAT/DOUBLE/DECIMAL 列）。
    pub fn unique_f64(&self) -> f64 {
        self.unique_i64() as f64
    }

    /// 将递增整数按字母表编码为定长唯一字符串（base62 风格）。
    pub fn unique_string(&self, maximum_length: usize) -> String {
        let mut state = self.state.lock().unwrap();
        state.int_value = state.int_value.saturating_add(1);
        let mut value = state.int_value;
        drop(state);
        let mut output = Vec::new();
        // 从低位到高位取余映射到 ALPHABET，再 reverse 得到高位在前。
        for _ in 0..maximum_length {
            let index = value.rem_euclid(ALPHABET.len() as i64) as usize;
            output.push(ALPHABET[index]);
            value /= ALPHABET.len() as i64;
            if value == 0 {
                break;
            }
        }
        output.reverse();
        String::from_utf8(output).unwrap()
    }

    /// 推进时间序列：首次取当前 Unix 秒，之后按 `step * multiplier` 累加。
    fn advance_seconds(&self, multiplier: i64) -> i64 {
        let mut state = self.state.lock().unwrap();
        let now = || {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64
        };
        let value = state.time_seconds.map_or_else(now, |value| {
            value.saturating_add(state.step.saturating_mul(multiplier))
        });
        state.time_seconds = Some(value);
        value
    }

    /// 唯一 TIME：`HH:MM:SS`（按秒步进，模一天）。
    pub fn unique_time(&self) -> String {
        let seconds = self.advance_seconds(1).rem_euclid(86_400);
        format!(
            "{:02}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    }

    /// 唯一 DATE：按天步进后经 civil 日换算为 `YYYY-MM-DD`。
    pub fn unique_date(&self) -> String {
        let seconds = self.advance_seconds(86_400);
        let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
        format!("{year:04}-{month:02}-{day:02}")
    }

    /// 唯一 DATETIME/TIMESTAMP：按秒步进，输出日期与时钟。
    pub fn unique_timestamp(&self) -> String {
        let seconds = self.advance_seconds(1);
        let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
        let clock = seconds.rem_euclid(86_400);
        format!(
            "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
            clock / 3600,
            clock / 60 % 60,
            clock % 60
        )
    }

    /// 唯一 YEAR：在保留月日时钟的前提下按 step 递增年份。
    pub fn unique_year(&self) -> String {
        let mut state = self.state.lock().unwrap();
        let (year, month, day, clock) = if let Some(seconds) = state.time_seconds {
            let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
            (
                year.saturating_add(state.step as i32),
                month,
                day,
                seconds.rem_euclid(86_400),
            )
        } else {
            let seconds = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
            (year, month, day, seconds.rem_euclid(86_400))
        };
        // 写回合成后的 Unix 秒，供后续时间序列继续使用。
        state.time_seconds = Some(days_from_civil(year, month, day) * 86_400 + clock);
        format!("{year:04}")
    }
}
