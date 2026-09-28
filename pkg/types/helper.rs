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

// 数值舍入、截断与字符串转整数等类型辅助函数。
//
// 对齐 Go `types` 包中 MySQL ROUND（银行家舍入）、flen/decimal 限幅，
// 以及 best-effort 的 `strToInt` 溢出/截断语义。

/// 银行家舍入（round half to even），对齐 MySQL ROUND 默认行为。
// RoundFloat 对应 Go 的 math.RoundToEven，用于模拟 MySQL ROUND 的默认 bankers rounding。
pub fn RoundFloat(f: f64) -> f64 {
    f.round_ties_even()
}

/// 将 `f` 舍入到 `dec` 位小数：先乘 10^dec，round-to-even，再除回。
// Round 对应 Go 的 Round：先移动小数点，再 round-to-even，最后移回。
pub fn Round(f: f64, dec: i32) -> f64 {
    let shift = 10_f64.powi(dec);
    let tmp = f * shift;
    // 移位后溢出为 Inf 时保留原值，避免错误归零
    if tmp.is_infinite() {
        return f;
    }
    let result = RoundFloat(tmp) / shift;
    if result.is_nan() {
        return 0.0;
    }
    result
}

/// 按 `dec` 截断（向零）小数位；Inf/NaN/`shift==0` 走边界分支。
// Truncate 对应 Go 的 Truncate：按 dec 截断小数位，保留 Inf/NaN/shift==0 的边界分支。
pub fn Truncate(f: f64, dec: i32) -> f64 {
    let shift = 10_f64.powi(dec);
    let tmp = f * shift;

    // dec 极大时 shift/tmp 可能为 Inf 或 NaN；这些边界直接返回原值。
    if tmp.is_infinite() || tmp.is_nan() {
        return f;
    }
    if shift == 0.0 {
        if f.is_nan() {
            return f;
        }
        return 0.0;
    }
    tmp.trunc() / shift
}

/// 由显示宽度 flen 与小数位 decimal 计算该类型可表示的最大正浮点值。
// GetMaxFloat 对应 Go 的最大浮点值计算：flen-decimal 为整数位数，再减掉最小小数单位。
pub fn GetMaxFloat(flen: i32, decimal: i32) -> f64 {
    let intPartLen = flen - decimal;
    let mut f = 10_f64.powi(intPartLen);
    // 减去最小小数单位，得到形如 999.99 的上界
    f -= 10_f64.powi(-decimal);
    f
}

/// 先按 decimal 舍入，再按 flen/decimal 限幅；超界返回 ErrOverflow。
// TruncateFloat 对应 Go 的截断并按 flen/decimal 限幅逻辑；返回值保留 Go 的 (float64, error) 形状。
pub fn TruncateFloat(mut f: f64, flen: i32, decimal: i32) -> (f64, Option<errors::SharedError>) {
    if f.is_nan() {
        // Go 对 NaN 返回 0 并带 ErrOverflow。
        return (0.0, Some(ErrOverflow.GenWithStackByArgs(&["DOUBLE".into(), "".into()])));
    }

    let maxF = GetMaxFloat(flen, decimal);
    if !f.is_infinite() {
        f = Round(f, decimal);
    }

    let mut err: Option<errors::SharedError> = None;
    if f > maxF {
        f = maxF;
        err = Some(ErrOverflow.GenWithStackByArgs(&["DOUBLE".into(), "".into()]));
    } else if f < -maxF {
        f = -maxF;
        err = Some(ErrOverflow.GenWithStackByArgs(&["DOUBLE".into(), "".into()]));
    }

    (f, errors::Trace(err))
}

/// 截断后再格式化为十进制字符串（对齐 Go FormatFloat）。
// TruncateFloatToString 对应 Go 的 FormatFloat(f, 'f', -1, 64)。
pub fn TruncateFloatToString(f: f64, dec: i32) -> String {
    let f = Truncate(f, dec);
    format!("{}", f)
}

/// 判断是否为空格或制表符。
fn isSpace(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

/// 判断是否为 ASCII 数字字符。
fn isDigit(c: u8) -> bool {
    c >= b'0' && c <= b'9'
}

/// ASCII 可打印非字母数字的标点判断（对齐 Go）。
// isPunctuation 对应 Go 的 ASCII 可打印非字母数字标点判断。
fn isPunctuation(c: u8) -> bool {
    (c >= 0x21 && c <= 0x2F) || (c >= 0x3A && c <= 0x40) || (c >= 0x5B && c <= 0x60) || (c >= 0x7B && c <= 0x7E)
}

/// u64 最大值，用于溢出检测分母。
const maxUint: u64 = u64::MAX;
/// 再乘 10 会溢出 u64 的阈值。
const uintCutOff: u64 = maxUint / 10 + 1;
/// 再加一位会超出 i64::MAX 的无符号阈值（亦等于 |i64::MIN|）。
const intCutOff: u64 = i64::MAX as u64 + 1;

/// best-effort 字符串转 i64：截断/溢出时尽量返回边界值并附带错误。
// strToInt 对应 Go 的 best-effort 字符串转整数；遇到截断和溢出时保留错误但尽量返回边界值。
pub(crate) fn strToInt(str_: &str) -> (i64, Option<errors::SharedError>) {
    let str_ = str_.trim();
    if str_.is_empty() {
        return (0, Some(ErrTruncated.GenWithStackByArgs(&[])));
    }

    let bytes = str_.as_bytes();
    let mut negative = false;
    let mut i = 0usize;
    if bytes[i] == b'-' {
        negative = true;
        i += 1;
    } else if bytes[i] == b'+' {
        i += 1;
    }

    let mut err: Option<errors::SharedError> = None;
    let mut hasNum = false;
    let mut r: u64 = 0;
    while i < bytes.len() {
        // 非数字：标记截断并停止解析（保留已解析前缀）
        if !char::from(bytes[i]).is_ascii_digit() {
            err = Some(ErrTruncated.GenWithStackByArgs(&[]));
            break;
        }
        hasNum = true;
        if r >= uintCutOff {
            r = 0;
            err = Some(ErrBadNumber.GenWithStackByArgs(&[]));
            break;
        }
        r *= 10;

        let Some(r1) = r.checked_add(u64::from(bytes[i] - b'0')) else {
            r = 0;
            err = Some(ErrBadNumber.GenWithStackByArgs(&[]));
            break;
        };
        r = r1;
        i += 1;
    }

    if !hasNum {
        err = Some(ErrTruncated.GenWithStackByArgs(&[]));
    }
    // 正数超出 i64::MAX → 钳到 MAX 并报 BadNumber
    if !negative && r >= intCutOff {
        return (i64::MAX, Some(ErrBadNumber.GenWithStackByArgs(&[])));
    }
    // 负数绝对值大于 |i64::MIN| → 钳到 MIN
    if negative && r > intCutOff {
        return (i64::MIN, Some(ErrBadNumber.GenWithStackByArgs(&[])));
    }

    let signed = if negative {
        if r == intCutOff { i64::MIN } else { -(r as i64) }
    } else {
        r as i64
    };
    (signed, err)
}

/// 测试出口：暴露包内私有 `strToInt`。
pub fn StrToIntForTest(value: &str) -> (i64, Option<errors::SharedError>) {
    strToInt(value)
}

/// DECIMAL 显示长度/小数位换算为 precision（扣掉小数点与符号位）。
// DecimalLength2Precision 对应 Go 的 length/scale 到 precision 换算。
pub fn DecimalLength2Precision(mut length: i32, scale: i32, hasUnsignedFlag: bool) -> i32 {
    if scale > 0 {
        length -= 1;
    }
    if hasUnsignedFlag || length > 0 {
        length -= 1;
    }
    length
}

/// precision/scale 换回显示长度，方向与 DecimalLength2Precision 相反。
// Precision2LengthNoTruncation 对应 Go 的 precision/scale 到显示长度换算，方向与上一个函数相反。
pub fn Precision2LengthNoTruncation(mut length: i32, scale: i32, hasUnsignedFlag: bool) -> i32 {
    if scale > 0 {
        length += 1;
    }
    if hasUnsignedFlag || length > 0 {
        length += 1;
    }
    length
}
