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

// FSP（Fractional Seconds Precision，小数秒精度）相关常量与解析。
//
// MySQL 时间类型可声明 0..=6 位小数秒；本模块负责范围校验、
// 小数串解析为微秒，以及按精度右侧补零。

/// 未指定 / 最大 / 最小 / 默认小数秒精度常量。
// UnspecifiedFsp/MaxFsp/MinFsp/DefaultFsp 保留 Go 中小数秒精度的常量取值。
pub const UnspecifiedFsp: i32 = -1;
pub const MaxFsp: i32 = 6;
pub const MinFsp: i32 = 0;
pub const DefaultFsp: i32 = 0;

/// 校验并规范化 FSP：未指定→默认；过小报错；过大截到 MaxFsp。
// CheckFsp 对应 Go 的范围检查：未指定时落到默认值，低于最小值报错，高于最大值截到 MaxFsp。
pub fn CheckFsp<T: Into<i64>>(fsp: T) -> (i32, Option<errors::SharedError>) {
    let fsp = fsp.into();
    // 未指定：落到默认精度，不报错
    if fsp == i64::from(UnspecifiedFsp) {
        return (DefaultFsp, None);
    }
    if fsp < i64::from(MinFsp) {
        return (
            DefaultFsp,
            Some(errors::Errorf("Invalid fsp %d", &[fsp.into()])),
        );
    } else if fsp > i64::from(MaxFsp) {
        // 超过最大值时静默截断到 MaxFsp（与 Go 一致）
        return (MaxFsp, None);
    }
    (fsp as i32, None)
}

/// 将小数秒字符串解析为微秒；返回 (微秒, 是否进位到整秒, 错误)。
// ParseFrac 对应 Go 的 ParseFrac，返回微秒值、是否因为舍入产生进位，以及解析错误。
pub fn ParseFrac(s: &str, fsp: i32) -> (i32, bool, Option<errors::SharedError>) {
    if s.is_empty() {
        return (0, false, None);
    }

    let (fsp, err) = CheckFsp(fsp);
    if err.is_some() {
        return (0, false, err);
    }
    // 小数位不足 fsp：右侧按 MaxFsp 补齐到微秒，无需舍入
    if fsp as usize >= s.len() {
        let tmp = match s.parse::<i64>() {
            Ok(value) => value,
            Err(err) => {
                return (
                    0,
                    false,
                    Some(errors::New(format!("strconv.ParseInt: {err}"))),
                );
            }
        };
        let v = (tmp as f64 * 10_f64.powi(MaxFsp - s.len() as i32)) as i32;
        return (v, false, None);
    }

    // fsp 小于字符串长度时，Go 取 fsp+1 位做四舍五入；这里保留同一进位判断。
    let prefix_bytes = &s.as_bytes()[..fsp as usize + 1];
    let prefix = match std::str::from_utf8(prefix_bytes) {
        Ok(prefix) => prefix,
        Err(err) => {
            return (
                0,
                false,
                Some(errors::New(format!("strconv.ParseInt: {err}"))),
            );
        }
    };
    let mut tmp = match prefix.parse::<i64>() {
        Ok(value) => value,
        Err(err) => {
            return (
                0,
                false,
                Some(errors::New(format!("strconv.ParseInt: {err}"))),
            );
        }
    };
    // 多取一位做四舍五入：+5 再整除 10
    tmp = (tmp + 5) / 10;

    // 舍入后进位到整数部分（如 999 round 2 → 100）
    if (tmp as f64) >= 10_f64.powi(fsp) {
        return (0, true, None);
    }

    // 最终统一补成 6 位微秒，例如 1236 round 3 -> 124000。
    let v = (tmp as f64 * 10_f64.powi(MaxFsp - fsp)) as i32;
    (v, false, None)
}

/// 按 fsp 在小数串右侧补 0；负号不计入有效数字长度。
// alignFrac 对应 Go 的私有函数：按 fsp 右侧补 0，负号不计入有效长度。
pub(crate) fn alignFrac(s: &str, fsp: i32) -> String {
    let mut sl = s.len() as i32;
    if sl > 0 && s.as_bytes()[0] == b'-' {
        sl -= 1;
    }
    if sl < fsp {
        return format!("{}{}", s, "0".repeat((fsp - sl) as usize));
    }
    s.to_string()
}

/// 测试出口：暴露包内私有 `alignFrac`。
pub fn AlignFracForTest(s: &str, fsp: i32) -> String {
    alignFrac(s, fsp)
}
