// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 配置类型模块：提供配置文件（TOML/JSON）中常用的两类基础类型及其序列化/反序列化辅助函数。
//
// - `ByteSize`：字节容量类型（如 "1GB"、"512MB"），用于表达缓存大小、内存配额等配置项。
// - `Duration`：时间长度类型（如 "10s"、"1h30m"），用于表达超时、间隔等配置项。
//
// 本模块源自 Go 版 TiDB 的机械迁移，因此保留了 Go 风格的
// `Type_MarshalJSON` / `Type_UnmarshalText` 等自由函数命名，
// 分别对应 Go 中类型实现的 `encoding/json` 与 `encoding.TextMarshaler` 接口方法。

use anyhow::{Context, Result, anyhow};

/// ByteSize is a retype uint64 for TOML and JSON.
/// ByteSize 是 u64 的类型别名，表示以字节为单位的容量，
/// 在 TOML 与 JSON 配置中以人类可读的字符串形式（如 "1GB"）表示。
pub type ByteSize = u64;

/// 将字节数格式化为紧凑的人类可读字符串（如 `1GB`、`512MB`）。
/// 去掉小数位为 0 的 ".0" 以及数值与单位之间的空格，与 Go 版输出格式保持一致。
fn format_byte_size(bytes: ByteSize) -> String {
    const UNITS: [&str; 9] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB", "ZiB", "YiB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        return format!("{bytes}B");
    }

    // docker/go-units uses %.4g: four significant decimal digits.
    let precision = if size >= 1000.0 {
        0
    } else if size >= 100.0 {
        1
    } else if size >= 10.0 {
        2
    } else {
        3
    };
    let mut number = format!("{size:.precision$}");
    if number.contains('.') {
        number = number
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned();
    }
    format!("{number}{}", UNITS[unit])
}

/// MarshalJSON returns the size as a JSON string.
/// 将字节容量序列化为 JSON 字符串（带引号），例如 `"1GB"`。
pub fn ByteSize_MarshalJSON(b: ByteSize) -> Result<Vec<u8>> {
    serde_json::to_vec(&format_byte_size(b)).context("marshal byte size as JSON")
}

/// MarshalText returns the size as a TOML string.
/// 将字节容量序列化为 TOML 文本（不带引号的裸字符串字节）。
pub fn ByteSize_MarshalText(b: ByteSize) -> Result<Vec<u8>> {
    Ok(format_byte_size(b).into_bytes())
}

/// 解析人类可读的容量字符串（如 "1GB"、"512MiB"）为字节数。
/// 输入必须是合法 UTF-8，否则返回错误。
fn parse_byte_size(text: &[u8]) -> Result<ByteSize> {
    let text = std::str::from_utf8(text).context("byte size is not valid UTF-8")?;
    let separator = text
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_ascii_digit() || *c == '.' || *c == ' ')
        .map(|(index, _)| index)
        .ok_or_else(|| anyhow!("invalid byte size {text:?}"))?;
    let (number, suffix) = if text.as_bytes()[separator] == b' ' {
        (&text[..separator], &text[separator + 1..])
    } else {
        (&text[..=separator], &text[separator + 1..])
    };
    let mut size: f64 = number
        .parse()
        .with_context(|| format!("invalid byte size {text:?}"))?;
    if size < 0.0 || !size.is_finite() {
        return Err(anyhow!("invalid byte size {text:?}"));
    }
    let suffix = suffix.to_ascii_lowercase();
    if !suffix.is_empty() && suffix != "b" {
        let first = suffix.as_bytes()[0];
        let exponent = match first {
            b'k' => 1,
            b'm' => 2,
            b'g' => 3,
            b't' => 4,
            b'p' => 5,
            _ => return Err(anyhow!("invalid byte size {text:?}")),
        };
        let valid_tail = suffix.len() == 1
            || (suffix.len() == 2 && suffix.as_bytes()[1] == b'b')
            || (suffix.len() == 3 && &suffix[1..] == "ib");
        if !valid_tail {
            return Err(anyhow!("invalid byte size {text:?}"));
        }
        size *= 1024_u64.pow(exponent) as f64;
    }
    if size > i64::MAX as f64 {
        return Err(anyhow!("byte size {text:?} overflows int64"));
    }
    Ok((size as i64) as u64)
}

/// UnmarshalJSON parses a JSON string into the byte size.
/// 从 JSON 字符串反序列化字节容量：先解除 JSON 引号，再解析容量文本，写回 `b`。
pub fn ByteSize_UnmarshalJSON(b: &mut ByteSize, text: &[u8]) -> Result<()> {
    let text: String = serde_json::from_slice(text).context("unquote byte size JSON string")?;
    let parsed = parse_byte_size(text.as_bytes())?;
    *b = parsed;
    Ok(())
}

/// UnmarshalText parses a TOML string into the byte size.
/// 从 TOML 文本反序列化字节容量，解析结果写回 `b`。
pub fn ByteSize_UnmarshalText(b: &mut ByteSize, text: &[u8]) -> Result<()> {
    let parsed = parse_byte_size(text)?;
    *b = parsed;
    Ok(())
}

/// Duration is a wrapper of Go's nanosecond-based time.Duration for TOML and JSON.
/// Duration 是对 Go `time.Duration`（以纳秒计数的 i64）的包装，
/// 用于 TOML/JSON 配置的序列化与反序列化。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Duration {
    /// 时间长度，单位为纳秒（对应 Go 的 time.Duration 内部表示）。
    pub Duration: i64,
}

/// 解析 Go 风格的时长字符串（如 "10s"、"1h30m"、"-1.5ms"）为纳秒数。
///
/// 与 Go `time.ParseDuration` 语义保持一致：
/// - 特殊值 "0" 直接返回 0（无需单位）；
/// - 支持前导正负号；
/// - 兼容微秒的两种 Unicode 写法（µ/μ），统一转成 "u" 后交给 humantime 解析；
/// - 负值溢出时允许恰好等于 i64::MIN 的边界情况。
fn parse_duration(text: &[u8]) -> Result<i64> {
    let text = std::str::from_utf8(text).context("duration is not valid UTF-8")?;
    let (negative, mut rest) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    if rest == "0" {
        return Ok(0);
    }
    if rest.is_empty() {
        return Err(anyhow!("invalid duration {text:?}"));
    }

    let mut total = 0_u128;
    while !rest.is_empty() {
        let integer_len = rest.bytes().take_while(u8::is_ascii_digit).count();
        let integer = &rest[..integer_len];
        rest = &rest[integer_len..];
        let mut fraction = "";
        if let Some(after_dot) = rest.strip_prefix('.') {
            let fraction_len = after_dot.bytes().take_while(u8::is_ascii_digit).count();
            fraction = &after_dot[..fraction_len];
            rest = &after_dot[fraction_len..];
        }
        if integer.is_empty() && fraction.is_empty() {
            return Err(anyhow!("invalid duration {text:?}"));
        }
        let unit_end = rest
            .char_indices()
            .find(|(_, c)| *c == '.' || c.is_ascii_digit())
            .map_or(rest.len(), |(index, _)| index);
        if unit_end == 0 {
            return Err(anyhow!("missing unit in duration {text:?}"));
        }
        let unit_text = &rest[..unit_end];
        rest = &rest[unit_end..];
        let unit: u128 = match unit_text {
            "ns" => 1,
            "us" | "µs" | "μs" => 1_000,
            "ms" => 1_000_000,
            "s" => 1_000_000_000,
            "m" => 60_000_000_000,
            "h" => 3_600_000_000_000,
            _ => return Err(anyhow!("unknown unit {unit_text:?} in duration {text:?}")),
        };
        let whole = if integer.is_empty() {
            0
        } else {
            integer
                .parse::<u128>()
                .with_context(|| format!("invalid duration {text:?}"))?
        };
        let mut value = whole
            .checked_mul(unit)
            .ok_or_else(|| anyhow!("duration {text:?} overflows i64 nanoseconds"))?;
        if !fraction.is_empty() {
            // Match time.leadingFraction: retain digits only until its uint64
            // accumulator would overflow, while still consuming the suffix.
            let mut fraction_value = 0_u64;
            let mut scale = 1_f64;
            let mut overflow = false;
            for digit in fraction.bytes() {
                if overflow {
                    continue;
                }
                if fraction_value > (i64::MAX as u64) / 10 {
                    overflow = true;
                    continue;
                }
                let next = fraction_value * 10 + u64::from(digit - b'0');
                if next > i64::MAX as u64 + 1 {
                    overflow = true;
                    continue;
                }
                fraction_value = next;
                scale *= 10.0;
            }
            value = value
                .checked_add((fraction_value as f64 * unit as f64 / scale) as u128)
                .ok_or_else(|| anyhow!("duration {text:?} overflows i64 nanoseconds"))?;
        }
        total = total
            .checked_add(value)
            .filter(|value| *value <= i64::MAX as u128 + 1)
            .ok_or_else(|| anyhow!("duration {text:?} overflows i64 nanoseconds"))?;
    }

    if negative {
        if total == i64::MAX as u128 + 1 {
            Ok(i64::MIN)
        } else {
            Ok(-(total as i64))
        }
    } else {
        i64::try_from(total).with_context(|| format!("duration {text:?} overflows i64 nanoseconds"))
    }
}

/// 将余数作为小数部分追加到输出串：按 `width` 位补零后去掉末尾多余的 0。
/// 例如 remainder=500、width=3 时追加 ".5"；remainder 为 0 时不追加。
fn append_fraction(output: &mut String, remainder: u64, width: usize) {
    if remainder == 0 {
        return;
    }
    let fraction = format!("{remainder:0width$}");
    output.push('.');
    output.push_str(fraction.trim_end_matches('0'));
}

/// 将纳秒数格式化为 Go `time.Duration.String()` 风格的字符串。
///
/// 规则与 Go 保持一致：
/// - 小于 1µs 用 "ns"，小于 1ms 用 "µs"，小于 1s 用 "ms"，各带小数部分；
/// - 大于等于 1s 按 "XhYmZs" 组合输出，秒可带小数；
/// - 0 输出 "0s"，负值加前导 '-'。
fn format_duration(duration: i64) -> String {
    let negative = duration < 0;
    let mut nanos = duration.unsigned_abs();
    let mut output = String::new();
    if negative {
        output.push('-');
    }
    if nanos == 0 {
        output.push_str("0s");
        return output;
    }

    const MICROSECOND: u64 = 1_000;
    const MILLISECOND: u64 = 1_000_000;
    const SECOND: u64 = 1_000_000_000;
    const MINUTE: u64 = 60 * SECOND;
    const HOUR: u64 = 60 * MINUTE;

    // 按数量级选择输出单位：ns → µs → ms → h/m/s。
    if nanos < MICROSECOND {
        output.push_str(&format!("{nanos}ns"));
    } else if nanos < MILLISECOND {
        output.push_str(&(nanos / MICROSECOND).to_string());
        append_fraction(&mut output, nanos % MICROSECOND, 3);
        output.push_str("µs");
    } else if nanos < SECOND {
        output.push_str(&(nanos / MILLISECOND).to_string());
        append_fraction(&mut output, nanos % MILLISECOND, 6);
        output.push_str("ms");
    } else {
        let hours = nanos / HOUR;
        if hours != 0 {
            output.push_str(&format!("{hours}h"));
            nanos %= HOUR;
        }
        let minutes = nanos / MINUTE;
        // 只要有小时部分，即便分钟为 0 也要输出（如 "1h0m5s"）。
        if hours != 0 || minutes != 0 {
            output.push_str(&format!("{minutes}m"));
            nanos %= MINUTE;
        }
        output.push_str(&(nanos / SECOND).to_string());
        append_fraction(&mut output, nanos % SECOND, 9);
        output.push('s');
    }
    output
}

/// MarshalJSON returns the duration as a JSON string.
/// 将时长序列化为 JSON 字符串（带引号），例如 `"10s"`。
pub fn Duration_MarshalJSON(d: &Duration) -> Result<Vec<u8>> {
    serde_json::to_vec(&format_duration(d.Duration)).context("marshal duration as JSON")
}

/// UnmarshalJSON parses a JSON string into the duration.
/// 从 JSON 字符串反序列化时长：先解除 JSON 引号，再解析时长文本，写回 `d`。
pub fn Duration_UnmarshalJSON(d: &mut Duration, text: &[u8]) -> Result<()> {
    let text: String = serde_json::from_slice(text).context("unquote duration JSON string")?;
    let parsed = parse_duration(text.as_bytes())?;
    d.Duration = parsed;
    Ok(())
}

/// UnmarshalText parses a TOML string into the duration.
/// 从 TOML 文本反序列化时长，解析结果写回 `d`。
pub fn Duration_UnmarshalText(d: &mut Duration, text: &[u8]) -> Result<()> {
    match parse_duration(text) {
        Ok(parsed) => {
            d.Duration = parsed;
            Ok(())
        }
        Err(error) => {
            // Go assigns time.ParseDuration's zero result before returning its error.
            d.Duration = 0;
            Err(error)
        }
    }
}

/// MarshalText returns the duration as a TOML string.
/// 将时长序列化为 TOML 文本（不带引号的裸字符串字节）。
pub fn Duration_MarshalText(d: Duration) -> Result<Vec<u8>> {
    Ok(format_duration(d.Duration).into_bytes())
}
