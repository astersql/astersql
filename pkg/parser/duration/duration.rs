// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 时长字符串解析模块。
//
// 将形如 `1d`、`1.5h`、`1h100m` 的文本解析为 `std::time::Duration`。
// 支持单位：`d`（天）、`h`（小时）、`m`（分钟）；允许小数；转换时按纳秒
// 整数截断，对齐 Go `time.Duration` 语义。用于配置项与超时类参数解析。

// 本实现保留 pkg/parser/duration/duration.go 的声明与解析顺序。
// 支持 d、h、m 单位的时长文本解析。
// strconv、time、unicode 与 errors 分别映射为 Rust 字符扫描、Duration 和字符串错误。
use std::time::Duration;

// readFloat 对应 Go 的同名辅助函数：读取字符串开头连续的十进制数字和小数点，并返回剩余单位文本。
// Go 按 rune 遍历却按字节位置切片；合法数字是 ASCII，这里用 char_indices 保留该字节边界语义。
/// 读取字符串开头的浮点数字面量，返回数值与剩余单位文本。
fn read_float(s: &str) -> Result<(f64, &str), String> {
    let mut end = None;
    for (pos, ch) in s.char_indices() {
        // Go uses unicode.IsDigit. Rust's numeric predicate is slightly broader,
        // but every non-ASCII numeric character is rejected by f64 parsing below,
        // preserving the public parse result while retaining the UTF-8 token.
        if !ch.is_numeric() && ch != '.' {
            end = Some(pos);
            break;
        }
    }

    // 与 Go 一致：只有遇到非数字字符、且前面确实有数字文本时才尝试解析。
    if let Some(pos) = end.filter(|pos| *pos > 0) {
        let numbers = &s[..pos];
        let value = numbers
            .parse::<f64>()
            .map_err(|err| format!("invalid float {numbers:?}: {err}"))?;
        // Go's strconv.ParseFloat returns ErrRange when a well-formed value
        // overflows to infinity. Rust accepts the same token as `f64::INFINITY`,
        // so reject it explicitly before the integer duration conversion.
        if !value.is_finite() {
            return Err(format!("invalid float {numbers:?}: value out of range"));
        }
        return Ok((value, &s[pos..]));
    }

    Err("fail to read an integer".to_owned())
}

// ParseDuration 对应 Go 的导出函数：依次累加 d、h、m 片段，结果使用 std::time::Duration 表示。
// 原实现允许小数；转换为 Duration 时按 Go time.Duration 的纳秒整数截断语义构造结果。
/// 解析时长文本为 `Duration`；空片段循环结束后返回累计纳秒。
#[allow(non_snake_case)]
pub fn ParseDuration(mut s: &str) -> Result<Duration, String> {
    let mut duration_nanos = 0_u64;

    // 特判裸 "0"，避免把它当作缺少单位的数字片段。
    if s == "0" {
        return Ok(Duration::ZERO);
    }

    while !s.is_empty() {
        let (value, rest) = read_float(s)?;
        let Some(unit) = rest.as_bytes().first().copied() else {
            // Go 随后访问 s[0]；这里显式返回错误，记录输入末尾缺少单位这一非法情况。
            return Err("duration unit is missing".to_owned());
        };

        // 将单位换算为纳秒系数后与数值相乘，饱和累加。
        let unit_nanos = match unit {
            b'd' => 24.0 * 60.0 * 60.0 * 1_000_000_000.0,
            b'h' => 60.0 * 60.0 * 1_000_000_000.0,
            b'm' => 60.0 * 1_000_000_000.0,
            _ => return Err(format!("unknown unit {}", unit as char)),
        };
        duration_nanos = duration_nanos.saturating_add((value * unit_nanos) as u64);

        // 单位始终是单字节 ASCII，跳过它后继续解析下一段。
        s = &rest[1..];
    }

    Ok(Duration::from_nanos(duration_nanos))
}
