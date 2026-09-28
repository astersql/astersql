// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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
// 整数与 Duration 四则运算的溢出检查，对齐 MySQL BIGINT / BIGINT UNSIGNED 语义。
//
// 溢出（overflow）指结果超出目标类型可表示范围；函数以 `OverflowError` 返回目标类型名与表达式文本。

#[derive(Clone, Debug, Eq, PartialEq)]
/// 运算溢出错误：记录目标 SQL 类型名与触发溢出的表达式字符串。
pub struct OverflowError {
    /// 目标类型，如 `"BIGINT"` 或 `"BIGINT UNSIGNED"`。
    pub target_type: &'static str,
    /// 触发溢出的运算表达式文本，用于错误消息。
    pub expression: String,
}

impl OverflowError {
    /// 构造溢出错误。
    fn new(target_type: &'static str, expression: String) -> Self {
        Self {
            target_type,
            expression,
        }
    }
}

/// 格式化为 MySQL 风格：`{type} value is out of range in '{expr}'`。
impl std::fmt::Display for OverflowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} value is out of range in '{}'",
            self.target_type, self.expression
        )
    }
}

/// 标记为标准 Error。
impl std::error::Error for OverflowError {}

/// Duration 对应 Go `time.Duration` 的底层 int64 纳秒表示。
// Duration 对应 Go time.Duration 的底层 int64 纳秒表示。
pub type Duration = i64;

/// 无符号 64 位加法；越界返回 BIGINT UNSIGNED 溢出。
// AddUint64 对应 Go 的 AddUint64：无符号加法先检查 MaxUint64-a 是否还能容纳 b。
pub fn AddUint64(a: u64, b: u64) -> Result<u64, OverflowError> {
    a.checked_add(b)
        .ok_or_else(|| OverflowError::new("BIGINT UNSIGNED", format!("({}, {})", a, b)))
}

/// 有符号 64 位加法；越界返回 BIGINT 溢出。
// AddInt64 对应 Go 的 AddInt64：按同号相加分别检查正溢出和负溢出。
pub fn AddInt64(a: i64, b: i64) -> Result<i64, OverflowError> {
    a.checked_add(b)
        .ok_or_else(|| OverflowError::new("BIGINT", format!("({}, {})", a, b)))
}

/// Duration 相加，复用 AddInt64 边界检查。
// AddDuration 对应 Go 的 AddDuration：time.Duration 底层按 int64 纳秒检查。
pub fn AddDuration(a: Duration, b: Duration) -> Result<Duration, OverflowError> {
    AddInt64(a, b)
}

/// Duration 相减，复用 SubInt64（含 0-MinInt64 特例）。
// SubDuration 对应 Go 的 SubDuration：减法需要额外处理 0 - MinInt64 这一不可取负值。
pub fn SubDuration(a: Duration, b: Duration) -> Result<Duration, OverflowError> {
    SubInt64(a, b)
}

/// uint64 与 int64 混合加法；负加数等价为无符号减法。
// AddInteger 对应 Go 的 AddInteger：uint64 与 int64 相加，负数分支等价为无符号减法。
pub fn AddInteger(a: u64, b: i64) -> Result<u64, OverflowError> {
    if b >= 0 {
        return AddUint64(a, b as u64);
    }

    let magnitude = b.unsigned_abs();
    if magnitude > a {
        return Err(OverflowError::new(
            "BIGINT UNSIGNED",
            format!("({}, {})", a, b),
        ));
    }
    Ok(a - magnitude)
}

/// 无符号 64 位减法；被减数小于减数时溢出。
// SubUint64 对应 Go 的 SubUint64：无符号减法只需检查被减数是否小于减数。
pub fn SubUint64(a: u64, b: u64) -> Result<u64, OverflowError> {
    a.checked_sub(b)
        .ok_or_else(|| OverflowError::new("BIGINT UNSIGNED", format!("({}, {})", a, b)))
}

/// 有符号 64 位减法；越界返回 BIGINT 溢出。
// SubInt64 对应 Go 的 SubInt64：符号相反时检查减法是否跨过 int64 边界。
pub fn SubInt64(a: i64, b: i64) -> Result<i64, OverflowError> {
    a.checked_sub(b)
        .ok_or_else(|| OverflowError::new("BIGINT", format!("({}, {})", a, b)))
}

/// uint64 减 int64；减数为负时改为无符号加法。
// SubUintWithInt 对应 Go 的 SubUintWithInt：减去负数时复用无符号加法检查。
pub fn SubUintWithInt(a: u64, b: i64) -> Result<u64, OverflowError> {
    if b < 0 {
        return AddUint64(a, b.unsigned_abs());
    }
    SubUint64(a, b as u64)
}

/// int64 减 uint64，结果按无符号解释；负被减数或不足则溢出。
// SubIntWithUint 对应 Go 的 SubIntWithUint：负 int64 或小于 uint64 减数都会溢出到 unsigned 语义外。
pub fn SubIntWithUint(a: i64, b: u64) -> Result<u64, OverflowError> {
    if a < 0 || (a as u64) < b {
        return Err(OverflowError::new(
            "BIGINT UNSIGNED",
            format!("({}, {})", a, b),
        ));
    }
    Ok(a as u64 - b)
}

/// 无符号 64 位乘法；用 Max/b 预判是否越界。
// MulUint64 对应 Go 的 MulUint64：先用 MaxUint64/b 判定乘法是否会越界。
pub fn MulUint64(a: u64, b: u64) -> Result<u64, OverflowError> {
    if b > 0 && a > u64::MAX / b {
        return Err(OverflowError::new(
            "BIGINT UNSIGNED",
            format!("({}, {})", a, b),
        ));
    }
    Ok(a * b)
}

/// 有符号 64 位乘法；拆符号后复用无符号乘再还原。
// MulInt64 对应 Go 的 MulInt64：把符号拆开后复用无符号乘法，再按结果符号检查 int64 上界。
pub fn MulInt64(a: i64, b: i64) -> Result<i64, OverflowError> {
    if a == 0 || b == 0 {
        return Ok(0);
    }

    let mut negative = false;
    let res = if a > 0 && b > 0 {
        MulUint64(a as u64, b as u64)?
    } else if a < 0 && b < 0 {
        MulUint64(a.unsigned_abs(), b.unsigned_abs())?
    } else if a < 0 && b > 0 {
        negative = true;
        MulUint64(a.unsigned_abs(), b as u64)?
    } else {
        negative = true;
        MulUint64(a as u64, b.unsigned_abs())?
    };

    if negative {
        // 负结果允许绝对值到 MaxInt64+1，对应 math.MinInt64。
        if res > i64::MAX as u64 + 1 {
            return Err(OverflowError::new("BIGINT", format!("({}, {})", a, b)));
        }
        return Ok((res as i64).wrapping_neg());
    }

    if res > i64::MAX as u64 {
        return Err(OverflowError::new("BIGINT", format!("({}, {})", a, b)));
    }

    Ok(res as i64)
}

/// uint64 乘 int64；负乘数在无符号结果语义下直接溢出。
// MulInteger 对应 Go 的 MulInteger：uint64 乘 int64；负乘数在 unsigned 结果语义下直接溢出。
pub fn MulInteger(a: u64, b: i64) -> Result<u64, OverflowError> {
    if a == 0 || b == 0 {
        return Ok(0);
    }

    if b < 0 {
        return Err(OverflowError::new(
            "BIGINT UNSIGNED",
            format!("({}, {})", a, b),
        ));
    }

    MulUint64(a, b as u64)
}

/// 有符号除法；仅额外拦截 MinInt64/-1，除零交给调用方。
// DivInt64 对应 Go 的 DivInt64：这里只检查 MinInt64 / -1，除零 panic 语义保留给调用侧。
pub fn DivInt64(a: i64, b: i64) -> Result<i64, OverflowError> {
    if a == i64::MIN && b == -1 {
        return Err(OverflowError::new("BIGINT", format!("({}, {})", a, b)));
    }

    Ok(a / b)
}

/// uint64 除以 int64；负除数在结果无法表示为无符号时溢出。
// DivUintWithInt 对应 Go 的 DivUintWithInt：负除数只在结果无法表示为 BIGINT UNSIGNED 时返回溢出。
pub fn DivUintWithInt(a: u64, b: i64) -> Result<u64, OverflowError> {
    if b < 0 {
        if a != 0 && b.unsigned_abs() <= a {
            return Err(OverflowError::new(
                "BIGINT UNSIGNED",
                format!("({}, {})", a, b),
            ));
        }

        return Ok(0);
    }

    Ok(a / b as u64)
}

/// int64 除以 uint64；负被除数按无符号结果语义可能溢出。
// DivIntWithUint 对应 Go 的 DivIntWithUint：负被除数转换成 unsigned 结果时按溢出处理。
pub fn DivIntWithUint(a: i64, b: u64) -> Result<u64, OverflowError> {
    if a < 0 {
        if a.unsigned_abs() >= b {
            return Err(OverflowError::new(
                "BIGINT UNSIGNED",
                format!("({}, {})", a, b),
            ));
        }

        return Ok(0);
    }

    Ok(a as u64 / b)
}
