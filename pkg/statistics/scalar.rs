// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 标量辅助：将直方图桶边界与查询值映射到可比较的 f64，供选择性估计使用。
//
// 优化器估算范围谓词落在某桶内的比例时，需把 Datum（含字节串）线性化为标量；
// 另提供小整数/时长区间的枚举展开，用于精确枚举少量离散值。

use crate::histogram::{Histogram, scalar};

fn calcDayNumber(mut year: i32, month: i32, day: i32) -> i64 {
    if year == 0 && month == 0 {
        return 0;
    }
    let mut result = 365_i64 * i64::from(year) + 31 * i64::from(month - 1) + i64::from(day);
    if month <= 2 {
        year -= 1;
    } else {
        result -= i64::from((month * 4 + 23) / 10);
    }
    result + i64::from(year / 4) - i64::from(((year / 100 + 1) * 3) / 4)
}

fn timeMicros(value: types::Time) -> i64 {
    (calcDayNumber(value.Year(), value.Month(), value.Day()) * 86_400
        + i64::from(value.Hour()) * 3_600
        + i64::from(value.Minute()) * 60
        + i64::from(value.Second()))
        * 1_000_000
        + i64::from(value.Microsecond())
}

fn timeDifferenceNanos(upper: types::Time, lower: types::Time) -> i64 {
    if upper.Type() == types::mysql::TypeTimestamp && lower.Type() == types::mysql::TypeTimestamp {
        upper
            .Sub(&*types::DefaultStmtNoWarningContext, lower)
            .Duration
    } else {
        timeMicros(upper)
            .wrapping_sub(timeMicros(lower))
            .wrapping_mul(1_000)
    }
}

/// 计算 `value` 在 `[lower, upper]` 区间上的归一化位置（0~1）；退化区间返回 0.5。
pub fn calcFraction(lower: f64, upper: f64, value: f64) -> f64 {
    if upper <= lower {
        return 0.5;
    }
    if value <= lower {
        return 0.0;
    }
    if value >= upper {
        return 1.0;
    }
    let result = (value - lower) / (upper - lower);
    if result.is_finite() && (0.0..=1.0).contains(&result) {
        result
    } else {
        0.5
    }
}

/// 将 Datum 转为 f64 标量；字符串/字节跳过公共前缀后取前 8 字节大端整数。
pub fn convertDatumToScalar(value: &types::Datum, common_prefix_length: usize) -> f64 {
    match value.Kind() {
        types::KindFloat32 => value.GetFloat32() as f64,
        types::KindFloat64 => value.GetFloat64(),
        types::KindInt64 => value.GetInt64() as f64,
        types::KindUint64 => value.GetUint64() as f64,
        types::KindMysqlDuration => value.GetMysqlDuration().Duration as f64,
        types::KindMysqlDecimal => value.GetMysqlDecimal().ToFloat64().unwrap_or(0.0),
        types::KindMysqlTime => {
            let value = value.GetMysqlTime();
            if value.Type() == types::mysql::TypeTimestamp {
                value
                    .Sub(&*types::DefaultStmtNoWarningContext, types::MinTimestamp())
                    .Duration as f64
            } else {
                let minimum = types::NewTime(
                    types::FromDate(1, 1, 1, 0, 0, 0, 0),
                    value.Type(),
                    types::DefaultFsp,
                );
                timeMicros(value)
                    .wrapping_sub(timeMicros(minimum))
                    .wrapping_mul(1_000) as f64
            }
        }
        types::KindString | types::KindBytes => {
            let bytes = value.GetBytes();
            bytes
                .get(common_prefix_length..)
                .map_or(0.0, convertBytesToScalar)
        }
        types::KindMinNotNull => -f64::MAX,
        types::KindMaxValue => f64::MAX,
        _ => 0.0,
    }
}

impl Histogram {
    /// 预计算每个桶上下界的标量及公共前缀长度，缓存到 `Scalars`。
    pub fn PreCalculateScalar(&mut self) {
        if self.Len() == 0 {
            return;
        }
        let kind = self.GetLower(0).Kind();
        if !matches!(
            kind,
            types::KindMysqlDecimal | types::KindMysqlTime | types::KindBytes | types::KindString
        ) {
            return;
        }
        self.Scalars = vec![scalar::default(); self.Len()];
        for index in 0..self.Len() {
            let lower = self.GetLower(index);
            let upper = self.GetUpper(index);
            let common_prefix = if matches!(kind, types::KindBytes | types::KindString) {
                commonPrefixLength(&[lower.GetBytes(), upper.GetBytes()])
            } else {
                0
            };
            self.Scalars[index] = scalar {
                lower: convertDatumToScalar(lower, common_prefix),
                upper: convertDatumToScalar(upper, common_prefix),
                commonPfxLen: common_prefix,
            };
        }
    }

    /// 计算查询值在指定桶内的分数位置，供直方图选择性插值。
    pub(crate) fn calcFraction(&self, index: usize, value: &types::Datum) -> f64 {
        let common_prefix = self
            .Scalars
            .get(index)
            .map_or(0, |value| value.commonPfxLen);
        let lower = self.GetLower(index);
        let upper = self.GetUpper(index);
        match value.Kind() {
            types::KindFloat32
            | types::KindFloat64
            | types::KindInt64
            | types::KindUint64
            | types::KindMysqlDuration
            | types::KindMysqlDecimal
            | types::KindMysqlTime
            | types::KindBytes
            | types::KindString => calcFraction(
                convertDatumToScalar(lower, common_prefix),
                convertDatumToScalar(upper, common_prefix),
                convertDatumToScalar(value, common_prefix),
            ),
            _ => 0.5,
        }
    }
}

/// 多组字节序列的公共前缀长度。
pub fn commonPrefixLength(values: &[Vec<u8>]) -> usize {
    let Some(first) = values.first() else {
        return 0;
    };
    let maximum = values.iter().map(Vec::len).min().unwrap_or(0);
    (0..maximum)
        .find(|index| values.iter().any(|value| value[*index] != first[*index]))
        .unwrap_or(maximum)
}

/// 取字节前至多 8 字节按大端解释为 u64，再转为 f64，用于字符串标量化。
pub fn convertBytesToScalar(value: &[u8]) -> f64 {
    let mut bytes = [0_u8; 8];
    let length = value.len().min(8);
    bytes[..length].copy_from_slice(&value[..length]);
    u64::from_be_bytes(bytes) as f64
}

/// 在两个 Datum 边界间计算某值的分数位置（自动处理字符串公共前缀）。
pub fn calcFraction4Datums(
    lower: &types::Datum,
    upper: &types::Datum,
    value: &types::Datum,
) -> f64 {
    let common_prefix = if matches!(value.Kind(), types::KindBytes | types::KindString) {
        commonPrefixLength(&[lower.GetBytes(), upper.GetBytes()])
    } else {
        0
    };
    calcFraction(
        convertDatumToScalar(lower, common_prefix),
        convertDatumToScalar(upper, common_prefix),
        convertDatumToScalar(value, common_prefix),
    )
}

/// 可枚举离散区间的最大步数（含端点后的取值个数上限相关阈值）。
pub const maxNumStep: i64 = 10;

/// 对齐 Go `time.Duration.Round`：按最近步长舍入，中点远离零，溢出时钳位。
fn roundDuration(value: i64, step: i64) -> i64 {
    if step <= 0 {
        return value;
    }
    let value = i128::from(value);
    let step = i128::from(step);
    let remainder = value % step;
    let rounded = if remainder.abs() * 2 < step {
        value - remainder
    } else if value >= 0 {
        value + step - remainder
    } else {
        value - step - remainder
    };
    rounded.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

/// 当上下界类型一致且区间足够小时，枚举区间内全部 Datum；过大或不支持的类型返回 None。
pub fn EnumRangeValues(
    lower: types::Datum,
    upper: types::Datum,
    lower_excluded: bool,
    upper_excluded: bool,
) -> Option<Vec<types::Datum>> {
    if lower.Kind() != upper.Kind() {
        return None;
    }
    let excluded = i64::from(lower_excluded) + i64::from(upper_excluded);
    match lower.Kind() {
        types::KindInt64 => {
            let lower_value = lower.GetInt64();
            let upper_value = upper.GetInt64();
            if lower_value <= 0
                && upper_value >= 0
                && (lower_value < -maxNumStep || upper_value > maxNumStep)
            {
                return None;
            }
            let distance = upper_value.wrapping_sub(lower_value);
            if distance >= maxNumStep + 1 {
                return None;
            }
            let count = distance.wrapping_add(1).wrapping_sub(excluded);
            if !(0..maxNumStep).contains(&count) {
                return None;
            }
            let start = lower_value.wrapping_add(i64::from(lower_excluded));
            Some(
                (0..count)
                    .map(|offset| types::NewIntDatum(start.wrapping_add(offset)))
                    .collect(),
            )
        }
        types::KindUint64 => {
            let lower_value = lower.GetUint64();
            let upper_value = upper.GetUint64();
            let distance = upper_value.wrapping_sub(lower_value);
            if distance >= (maxNumStep + 1) as u64 {
                return None;
            }
            let count = distance.wrapping_add(1).wrapping_sub(excluded as u64);
            if count >= maxNumStep as u64 {
                return None;
            }
            let start = lower_value.wrapping_add(u64::from(lower_excluded));
            Some(
                (0..count)
                    .map(|offset| types::NewUintDatum(start.wrapping_add(offset)))
                    .collect(),
            )
        }
        types::KindMysqlDuration => {
            // 按时长精度 Fsp 确定步长，枚举中间 Duration 取值。
            let lower_duration = lower.GetMysqlDuration();
            let upper_duration = upper.GetMysqlDuration();
            let fsp = lower_duration.Fsp.max(upper_duration.Fsp);
            let step = 10_i64.pow((types::MaxFsp - fsp) as u32) * 1_000;
            let lower_value = roundDuration(lower_duration.Duration, step);
            let distance = upper_duration.Duration.wrapping_sub(lower_value);
            let count = (distance / step).wrapping_add(1).wrapping_sub(excluded);
            if !(1..maxNumStep).contains(&count) {
                return None;
            }
            let start = lower_value.wrapping_add(i64::from(lower_excluded) * step);
            Some(
                (0..count)
                    .map(|offset| {
                        types::NewDurationDatum(types::Duration {
                            Duration: start.wrapping_add(offset.wrapping_mul(step)),
                            Fsp: fsp,
                        })
                    })
                    .collect(),
            )
        }
        types::KindMysqlTime => {
            let mut lower_time = lower.GetMysqlTime();
            let upper_time = upper.GetMysqlTime();
            if lower_time.Type() != upper_time.Type() {
                return None;
            }
            let fsp = lower_time.Fsp().max(upper_time.Fsp());
            let step = if lower_time.Type() == types::mysql::TypeDate {
                lower_time.SetCoreTime(types::FromDate(
                    lower_time.Year(),
                    lower_time.Month(),
                    lower_time.Day(),
                    0,
                    0,
                    0,
                    0,
                ));
                24 * 60 * 60 * 1_000_000_000_i64
            } else {
                lower_time = lower_time
                    .RoundFrac(&*types::DefaultStmtNoWarningContext, fsp)
                    .ok()?;
                10_i64.pow((types::MaxFsp - fsp) as u32) * 1_000
            };
            let count = (timeDifferenceNanos(upper_time, lower_time) / step)
                .wrapping_add(1)
                .wrapping_sub(excluded);
            if !(1..maxNumStep).contains(&count) {
                return None;
            }
            let start = if lower_excluded {
                lower_time
                    .Add(
                        &*types::DefaultStmtNoWarningContext,
                        types::Duration {
                            Duration: step,
                            Fsp: fsp,
                        },
                    )
                    .ok()?
            } else {
                lower_time
            };
            (0..count)
                .map(|offset| {
                    start
                        .Add(
                            &*types::DefaultStmtNoWarningContext,
                            types::Duration {
                                Duration: offset * step,
                                Fsp: fsp,
                            },
                        )
                        .ok()
                        .map(types::NewTimeDatum)
                })
                .collect()
        }
        _ => None,
    }
}
