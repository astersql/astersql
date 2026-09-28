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
// MySQL DECIMAL/NUMERIC 的精确十进制实现（对齐 Go `types.MyDecimal`）。
//
// 数值按每 word 存 9 位十进制数字（`digitsPerWord`），固定 `maxWordBufLen` 个 word；
// 支持解析、舍入、移位、四则运算以及 MySQL 二进制编码（可排序）与 JSON 编解码。

#![allow(non_snake_case, non_upper_case_globals)]

use num_bigint::{BigInt, Sign};
use num_traits::{Signed, ToPrimitive, Zero};
use serde_json::Value;
use std::cmp::Ordering;
use std::fmt;

/// 舍入模式：与 Go 常量 ModeHalfUp / ModeTruncate / ModeCeiling 对应。
pub type RoundMode = i32;

/// 10^0 .. 10^9 幂次表，供 word 拆分与进位使用。
pub const ten0: i64 = 1;
pub const ten1: i64 = 10;
pub const ten2: i64 = 100;
pub const ten3: i64 = 1_000;
pub const ten4: i64 = 10_000;
pub const ten5: i64 = 100_000;
pub const ten6: i64 = 1_000_000;
pub const ten7: i64 = 10_000_000;
pub const ten8: i64 = 100_000_000;
pub const ten9: i64 = 1_000_000_000;
/// word 缓冲最大长度（9 个 word × 9 位 ≈ 81 位十进制）。
pub const maxWordBufLen: usize = 9;
/// 每个 word 容纳的十进制位数。
pub const digitsPerWord: usize = 9;
/// 二进制编码中完整 word 占用的字节数。
pub const wordSize: usize = 4;
/// 8 位十进制掩码（10^8）。
pub const digMask: i32 = ten8 as i32;
/// word 进制基数（10^9）。
pub const wordBase: i64 = ten9;
/// 单个 word 的最大十进制值（999_999_999）。
pub const wordMax: i32 = (wordBase - 1) as i32;
/// MySQL “未固定小数位”哨兵值（与 Go notFixedDec 一致）。
pub const notFixedDec: i8 = 31;
/// 四舍五入（half-up）。
pub const ModeHalfUp: RoundMode = 5;
/// 直接截断（向零）。
pub const ModeTruncate: RoundMode = 10;
/// Go 尚未完整支持的 Ceiling 模式；保留其逐 word 行为。
pub const ModeCeiling: RoundMode = 0;
/// Go/Rust MyDecimal 结构体的内存大小。
pub const MyDecimalStructSize: usize = 40;

/// MySQL DECIMAL 最大小数位数。
const MAX_DECIMAL_SCALE: usize = 30;
/// 0..9 次的 10 的幂，用于不足 9 位的小数 word 左对齐。
const POWERS10: [i32; 10] = [
    1,
    10,
    100,
    1_000,
    10_000,
    100_000,
    1_000_000,
    10_000_000,
    100_000_000,
    1_000_000_000,
];
/// 不足一整 word 的位数到打包字节数的映射（MySQL 二进制 DECIMAL 布局）。
const DIG2BYTES: [usize; 10] = [0, 1, 1, 2, 2, 3, 3, 4, 4, 4];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 十进制运算/解析错误，对齐 Go 的截断、溢出、除零等语义。
pub enum DecimalError {
    /// 精度不足导致截断。
    Truncated,
    /// 超出可表示范围。
    Overflow,
    /// 除数为零。
    DivByZero,
    /// 非法参数或损坏的编码。
    BadNumber,
    /// 字符串无法解析为合法十进制。
    TruncatedWrongValue,
    /// JSON 编解码失败。
    InvalidJson,
}

impl fmt::Display for DecimalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Truncated => "decimal truncated",
            Self::Overflow => "decimal overflow",
            Self::DivByZero => "decimal division by zero",
            Self::BadNumber => "bad decimal number",
            Self::TruncatedWrongValue => "truncated wrong DECIMAL value",
            Self::InvalidJson => "invalid decimal JSON",
        })
    }
}

impl std::error::Error for DecimalError {}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// MySQL DECIMAL 内存表示：整数位/小数位计数 + 符号 + word 缓冲。
pub struct MyDecimal {
    /// 整数部分十进制位数。
    pub digitsInt: i8,
    /// 小数部分十进制位数。
    pub digitsFrac: i8,
    /// 运算结果应保留的小数位（可与 digitsFrac 不同）。
    pub resultFrac: i8,
    /// 是否为负数；零值时通常为 false。
    pub negative: bool,
    /// 按 9 位一组存放的数字缓冲（整数 word 在前，小数 word 在后）。
    pub wordBuf: [i32; maxWordBufLen],
}

/// 计算 10^exponent 的大整数。
fn pow10_big(exponent: usize) -> BigInt {
    BigInt::from(10u8).pow(exponent as u32)
}

/// 将十进制位数换算为所需 word 数（向上取整）。
fn digits_to_words(digits: usize) -> usize {
    if digits == 0 {
        0
    } else {
        digits.div_ceil(digitsPerWord)
    }
}

/// 大整数的十进制位数（零视为 1 位）。
fn decimal_digits(value: &BigInt) -> usize {
    if value.is_zero() {
        1
    } else {
        value.abs().to_str_radix(10).len()
    }
}

/// 在给定小数位数 scale 下，unscaled 值的整数位数。
fn integer_digits(value: &BigInt, scale: usize) -> usize {
    decimal_digits(value).saturating_sub(scale)
}

/// 按 RoundMode 对 value/divisor 做带舍入的整除。
fn round_quotient(value: &BigInt, divisor: &BigInt, mode: RoundMode) -> BigInt {
    let mut quotient = value / divisor;
    let remainder = (value % divisor).abs();
    // Truncate 不进位；Ceiling 有余即进；其余按 half-up
    let increment = match mode {
        ModeTruncate => false,
        ModeCeiling => !remainder.is_zero(),
        _ => remainder * 2 >= *divisor,
    };
    if increment {
        if value.sign() == Sign::Minus {
            quotient -= 1;
        } else {
            quotient += 1;
        }
    }
    quotient
}

/// 尽力解析指数部分：可读前缀数字，非法后缀返回 Truncated。
fn parse_exponent_best_effort(text: &str) -> (i64, Result<(), DecimalError>) {
    let text = text.trim();
    if text.is_empty() {
        return (0, Err(DecimalError::Truncated));
    }
    let bytes = text.as_bytes();
    let mut index = 0;
    let negative = match bytes[0] {
        b'-' => {
            index = 1;
            true
        }
        b'+' => {
            index = 1;
            false
        }
        _ => false,
    };
    let mut value = 0_u64;
    let mut has_digit = false;
    let mut status = Ok(());
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            status = Err(DecimalError::Truncated);
            break;
        }
        has_digit = true;
        if value >= u64::MAX / 10 + 1 {
            return (0, Err(DecimalError::BadNumber));
        }
        value *= 10;
        let Some(next) = value.checked_add(u64::from(bytes[index] - b'0')) else {
            return (0, Err(DecimalError::BadNumber));
        };
        value = next;
        index += 1;
    }
    if !has_digit {
        return (0, Err(DecimalError::Truncated));
    }
    let int_cutoff = i64::MAX as u64 + 1;
    if (!negative && value >= int_cutoff) || (negative && value > int_cutoff) {
        return (
            if negative { i64::MIN } else { i64::MAX },
            Err(DecimalError::BadNumber),
        );
    }
    let signed = if negative {
        if value == i64::MAX as u64 + 1 {
            i64::MIN
        } else {
            -(value as i64)
        }
    } else {
        value as i64
    };
    (signed, status)
}

impl MyDecimal {
    /// 重置为零值。
    fn clear(&mut self) {
        *self = Self::default();
    }

    /// 由整数/小数字符串填充 wordBuf（小数 word 左对齐到 9 位）。
    fn set_parts(&mut self, integer: &str, fraction: &str, negative: bool) {
        self.clear();
        self.digitsInt = integer.len() as i8;
        self.digitsFrac = fraction.len() as i8;
        self.resultFrac = self.digitsFrac;

        // 整数部分：先写不足 9 位的高位 word，再按 9 位切分
        let mut word_index = 0usize;
        if !integer.is_empty() {
            let first = integer.len() % digitsPerWord;
            let mut offset = 0usize;
            if first != 0 {
                self.wordBuf[word_index] = integer[..first].parse().unwrap_or(0);
                word_index += 1;
                offset = first;
            }
            while offset < integer.len() {
                self.wordBuf[word_index] =
                    integer[offset..offset + digitsPerWord].parse().unwrap_or(0);
                word_index += 1;
                offset += digitsPerWord;
            }
        }

        // 小数部分：不足 9 位时乘 POWERS10 左对齐
        let mut offset = 0usize;
        while offset < fraction.len() {
            let end = (offset + digitsPerWord).min(fraction.len());
            let mut word = fraction[offset..end].parse::<i32>().unwrap_or(0);
            word *= POWERS10[digitsPerWord - (end - offset)];
            self.wordBuf[word_index] = word;
            word_index += 1;
            offset = end;
        }
        self.negative = negative && !self.IsZero();
    }

    /// 用“去掉小数点的整数 + 小数位数”设置内部表示。
    fn set_unscaled(&mut self, value: BigInt, scale: usize) {
        let negative = value.sign() == Sign::Minus;
        let mut digits = value.abs().to_str_radix(10);
        if digits.len() <= scale {
            let mut padded = String::with_capacity(scale + 1);
            padded.push_str(&"0".repeat(scale + 1 - digits.len()));
            padded.push_str(&digits);
            digits = padded;
        }
        let split = digits.len() - scale;
        let integer = digits[..split].trim_start_matches('0');
        let integer = if integer.is_empty() { "" } else { integer };
        self.set_parts(integer, &digits[split..], negative);
    }

    /// 还原为去掉小数点的有符号大整数（含当前 digitsFrac）。
    fn unscaled(&self) -> BigInt {
        let words_int = digits_to_words(self.digitsInt.max(0) as usize);
        let words_frac = digits_to_words(self.digitsFrac.max(0) as usize);
        let mut value = BigInt::zero();
        for word in self.wordBuf.iter().take(words_int + words_frac) {
            value = value * wordBase + i64::from(*word);
        }
        let padding = words_frac * digitsPerWord - self.digitsFrac.max(0) as usize;
        if padding > 0 {
            value /= pow10_big(padding);
        }
        if self.negative { -value } else { value }
    }

    /// Go arithmetic reads complete words, including multiplication guard
    /// digits beyond digitsFrac in the final word.
    fn word_scale(&self) -> usize {
        digits_to_words(self.digitsFrac.max(0) as usize) * 9
    }

    fn word_value(&self) -> BigInt {
        let count = digits_to_words(self.digitsInt.max(0) as usize) + self.word_scale() / 9;
        let mut value = BigInt::zero();
        for &word in self.wordBuf.iter().take(count) {
            value = value * wordBase + word;
        }
        if self.negative { -value } else { value }
    }

    fn significant_integer_digits(&self) -> usize {
        let mut remaining = self.digitsInt.max(0) as usize;
        let mut idx = 0;
        let mut width = if remaining == 0 {
            0
        } else {
            (remaining - 1) % 9 + 1
        };
        while remaining > 0 && self.wordBuf[idx] == 0 {
            remaining -= width;
            idx += 1;
            width = 9;
        }
        if remaining > 0 {
            remaining -= width.saturating_sub(self.wordBuf[idx].to_string().len());
        }
        remaining
    }

    fn significant_fraction_digits(&self) -> usize {
        let mut remaining = self.digitsFrac.max(0) as usize;
        let mut last = digits_to_words(self.digitsInt.max(0) as usize) + digits_to_words(remaining);
        let mut width = if remaining == 0 {
            0
        } else {
            (remaining - 1) % 9 + 1
        };
        while remaining > 0 && self.wordBuf[last - 1] == 0 {
            remaining -= width;
            last -= 1;
            width = 9;
        }
        if remaining > 0 {
            let mut power = 9 - (remaining - 1) % 9;
            while self.wordBuf[last - 1] % POWERS10[power] == 0 {
                remaining -= 1;
                power += 1;
            }
        }
        remaining
    }

    /// 将 unscaled 值写入缓冲；小数位不足时截断/舍入，整数位过多则 Overflow。
    fn fit_unscaled(
        &mut self,
        mut value: BigInt,
        mut scale: usize,
        mode: RoundMode,
    ) -> Result<(), DecimalError> {
        let int_words = digits_to_words(integer_digits(&value, scale));
        if int_words > maxWordBufLen {
            return Err(DecimalError::Overflow);
        }
        let available_scale = (maxWordBufLen - int_words) * digitsPerWord;
        let mut status = Ok(());
        if scale > available_scale {
            let divisor = pow10_big(scale - available_scale);
            value = round_quotient(&value, &divisor, mode);
            scale = available_scale;
            status = Err(DecimalError::Truncated);
            if digits_to_words(integer_digits(&value, scale)) > maxWordBufLen {
                return Err(DecimalError::Overflow);
            }
        }
        self.set_unscaled(value, scale);
        status
    }

    /// 规范化为 (整数文本, 小数文本)，去掉符号。
    fn normalized_strings(&self) -> (String, String) {
        let rendered = String::from_utf8(self.ToString()).expect("decimal is ASCII");
        let unsigned = rendered.strip_prefix('-').unwrap_or(&rendered);
        match unsigned.split_once('.') {
            Some((integer, fraction)) => (integer.to_string(), fraction.to_string()),
            None => (unsigned.to_string(), String::new()),
        }
    }

    /// 深拷贝（对齐 Go Clone）。
    pub fn Clone(&self) -> MyDecimal {
        self.clone()
    }

    /// 是否为负。
    pub fn IsNegative(&self) -> bool {
        self.negative
    }

    /// 返回小数位数。
    pub fn GetDigitsFrac(&self) -> i8 {
        self.digitsFrac
    }

    /// 返回整数位数。
    pub fn GetDigitsInt(&self) -> i8 {
        self.digitsInt
    }

    /// 按 resultFrac HalfUp 舍入后的十进制字符串。
    pub fn String(&self) -> String {
        let mut rounded = Self::default();
        let _ = self.round_into(&mut rounded, self.resultFrac as isize, ModeHalfUp, true);
        String::from_utf8(rounded.ToString()).expect("decimal is ASCII")
    }

    /// 不额外舍入，按当前 digitsFrac 输出 ASCII 字节。
    pub fn ToString(&self) -> Vec<u8> {
        let mut integer_digits = self.digitsInt.max(0) as usize;
        let int_words = digits_to_words(integer_digits);
        let mut start = 0;
        let mut first_width = if integer_digits == 0 {
            0
        } else {
            (integer_digits - 1) % 9 + 1
        };
        while integer_digits > 0 && self.wordBuf[start] == 0 {
            integer_digits -= first_width;
            first_width = 9;
            start += 1;
        }
        let mut text = String::new();
        if self.negative {
            text.push('-');
        }
        if integer_digits == 0 {
            text.push('0');
        } else {
            let width = first_width.min(self.wordBuf[start].to_string().len());
            // FromBin deliberately accepts some over-wide partial words in
            // Go. Print only the metadata's digit width, including any zero
            // created by dropping an over-wide most-significant digit.
            let first = self.wordBuf[start] % POWERS10[width];
            text.push_str(&format!("{first:0width$}"));
            for word in &self.wordBuf[start + 1..int_words] {
                text.push_str(&format!("{word:09}"));
            }
        }
        let mut remaining = self.digitsFrac.max(0) as usize;
        if remaining > 0 {
            text.push('.');
        }
        for word in self.wordBuf.iter().skip(int_words) {
            if remaining == 0 {
                break;
            }
            let width = remaining.min(9);
            let fragment = *word / POWERS10[9 - width];
            text.push_str(&format!("{fragment:0width$}"));
            remaining -= width;
        }
        text.into_bytes()
    }

    /// 从字符串解析；支持符号、小数点与科学计数法指数。
    pub fn FromString(&mut self, input: &[u8]) -> Result<(), DecimalError> {
        let previous = self.clone();
        self.clear();
        // Go parses bytes: invalid UTF-8 after an ASCII number is a suffix.
        let text = String::from_utf8_lossy(input);
        let text = text.trim_start_matches([' ', '\t']);
        if text.is_empty() {
            return Err(DecimalError::TruncatedWrongValue);
        }
        let bytes = text.as_bytes();
        let mut index = 0usize;
        let mut negative = previous.negative;
        if bytes[index] == b'-' || bytes[index] == b'+' {
            negative |= bytes[index] == b'-';
            index += 1;
        }
        let integer_start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        let integer_end = index;
        let mut fraction_end = index;
        if index < bytes.len() && bytes[index] == b'.' {
            index += 1;
            let fraction_start = index;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
            fraction_end = index;
            if integer_end == integer_start && fraction_end == fraction_start {
                return Err(DecimalError::TruncatedWrongValue);
            }
        } else if integer_end == integer_start {
            return Err(DecimalError::TruncatedWrongValue);
        }

        let raw_integer = &text[integer_start..integer_end];
        let raw_fraction = if fraction_end > integer_end {
            let dot = integer_end;
            if bytes.get(dot) == Some(&b'.') {
                &text[dot + 1..fraction_end]
            } else {
                ""
            }
        } else {
            ""
        };
        let mut status = Ok(());
        let mut integer = raw_integer;
        let int_words = digits_to_words(integer.len());
        if int_words > maxWordBufLen {
            integer = &integer[integer.len() - maxWordBufLen * digitsPerWord..];
            status = Err(DecimalError::Overflow);
        }
        let available_fraction_words = maxWordBufLen.saturating_sub(digits_to_words(integer.len()));
        let mut fraction = raw_fraction;
        if digits_to_words(fraction.len()) > available_fraction_words {
            fraction = &fraction[..available_fraction_words * digitsPerWord];
            if status.is_ok() {
                status = Err(DecimalError::Truncated);
            }
        }
        self.set_parts(integer, fraction, negative);
        let used = digits_to_words(integer.len()) + digits_to_words(fraction.len());
        self.wordBuf[used..].copy_from_slice(&previous.wordBuf[used..]);
        self.negative = negative && !self.IsZero();

        if index < bytes.len() && (bytes[index] == b'e' || bytes[index] == b'E') {
            let exponent_text = &text[index + 1..];
            let (exponent, exponent_status) = parse_exponent_best_effort(exponent_text);
            if exponent_status == Err(DecimalError::BadNumber) {
                self.clear();
            }
            if exponent > i32::MAX as i64 / 2 {
                let sign = self.negative;
                maxDecimal(maxWordBufLen * digitsPerWord, 0, self);
                self.negative = sign;
                status = Err(DecimalError::Overflow);
            } else if exponent < i32::MIN as i64 / 2 {
                self.clear();
                status = Err(DecimalError::Truncated);
            } else {
                if exponent_status.is_err() {
                    status = exponent_status;
                }
                if status != Err(DecimalError::Overflow)
                    && let Err(error) = self.Shift(exponent as isize)
                {
                    if error == DecimalError::Overflow {
                        maxDecimal(maxWordBufLen * digitsPerWord, 0, self);
                        self.negative = negative;
                    }
                    status = Err(error);
                }
            }
        } else if !text[index..].trim().is_empty() {
            status = Err(DecimalError::Truncated);
        }
        self.resultFrac = self.digitsFrac;
        status
    }

    /// 小数点移位：正数右移（×10^n），负数左移（÷10^n）。
    pub fn Shift(&mut self, shift: isize) -> Result<(), DecimalError> {
        if shift == 0 {
            return Ok(());
        }
        let (mut begin, mut end) = self.digit_bounds();
        if begin == end {
            self.clear();
            return Ok(());
        }
        let point = (digits_to_words(self.digitsInt as usize) * 9) as isize;
        let mut new_point = point.saturating_add(shift);
        let int_digits = new_point.saturating_sub(begin).max(0);
        let mut frac_digits = end.saturating_sub(new_point).max(0);
        let wi = digits_to_words(int_digits as usize);
        let mut wf = digits_to_words(frac_digits as usize);
        let mut status = Ok(());
        if wi + wf > maxWordBufLen {
            let lack = wi + wf - maxWordBufLen;
            if wf < lack {
                return Err(DecimalError::Overflow);
            }
            status = Err(DecimalError::Truncated);
            wf -= lack;
            let diff = frac_digits - (wf * 9) as isize;
            let source = self.clone();
            source.round_into(self, end - point - diff, ModeHalfUp, true)?;
            end -= diff;
            frac_digits = (wf * 9) as isize;
            if end <= begin {
                self.clear();
                return Err(DecimalError::Truncated);
            }
        }
        if shift % 9 != 0 {
            let (left, right, do_left) = if shift > 0 {
                let left = shift % 9;
                (left, 9 - left, left <= begin)
            } else {
                let right = -(shift % 9);
                let left = 9 - right;
                (left, right, 81 - end < right)
            };
            let mini = if do_left {
                self.mini_left_shift(left as usize, begin as usize, end as usize);
                -left
            } else {
                self.mini_right_shift(right as usize, begin as usize, end as usize);
                right
            };
            new_point += mini;
            if shift + mini == 0 && new_point - int_digits < 9 {
                self.digitsInt = int_digits as i8;
                self.digitsFrac = frac_digits as i8;
                return status;
            }
            begin += mini;
            end += mini;
        }
        let front = new_point - int_digits;
        if front >= 9 || front < 0 {
            let word_shift;
            if front > 0 {
                let amount = front / 9;
                let mut to = begin / 9 - amount;
                let mut barrier = (end - 1) / 9 - amount;
                while to <= barrier {
                    self.wordBuf[to as usize] = self.wordBuf[(to + amount) as usize];
                    to += 1;
                }
                barrier += amount;
                while to <= barrier {
                    self.wordBuf[to as usize] = 0;
                    to += 1;
                }
                word_shift = -amount;
            } else {
                let amount = (1 - front) / 9;
                let mut to = (end - 1) / 9 + amount;
                let mut barrier = begin / 9 + amount;
                while to >= barrier {
                    self.wordBuf[to as usize] = self.wordBuf[(to - amount) as usize];
                    to -= 1;
                }
                barrier -= amount;
                while to >= barrier {
                    self.wordBuf[to as usize] = 0;
                    to -= 1;
                }
                word_shift = amount;
            }
            begin += word_shift * 9;
            end += word_shift * 9;
            new_point += word_shift * 9;
        }
        let word_begin = begin / 9;
        let word_end = (end - 1) / 9;
        let mut word_point = if new_point == 0 {
            0
        } else {
            (new_point - 1) / 9
        };
        if word_point > word_end {
            while word_point > word_end {
                self.wordBuf[word_point as usize] = 0;
                word_point -= 1;
            }
        } else {
            while word_point < word_begin {
                self.wordBuf[word_point as usize] = 0;
                word_point += 1;
            }
        }
        self.digitsInt = int_digits as i8;
        self.digitsFrac = frac_digits as i8;
        status
    }

    fn digit_bounds(&self) -> (isize, isize) {
        let len =
            digits_to_words(self.digitsInt as usize) + digits_to_words(self.digitsFrac as usize);
        let mut first = 0;
        while first < len && self.wordBuf[first] == 0 {
            first += 1;
        }
        if first == len {
            return (0, 0);
        }
        let mut last = len - 1;
        let mut power = if first == 0 && self.digitsInt > 0 {
            (self.digitsInt as usize - 1) % 9
        } else {
            8
        };
        let mut begin = if first == 0 && self.digitsInt > 0 {
            8 - power
        } else {
            first * 9
        };
        while self.wordBuf[first] < POWERS10[power] {
            power -= 1;
            begin += 1;
        }
        while last > first && self.wordBuf[last] == 0 {
            last -= 1;
        }
        let mut end;
        if last == len - 1 && self.digitsFrac > 0 {
            let width = (self.digitsFrac as usize - 1) % 9 + 1;
            end = last * 9 + width;
            power = 10 - width;
        } else {
            end = (last + 1) * 9;
            power = 1;
        }
        while self.wordBuf[last] % POWERS10[power] == 0 {
            end -= 1;
            power += 1;
        }
        (begin as isize, end as isize)
    }

    fn mini_left_shift(&mut self, shift: usize, begin: usize, end: usize) {
        let mut from = begin / 9;
        let stop = (end - 1) / 9;
        let complement = 9 - shift;
        if begin % 9 < shift {
            self.wordBuf[from - 1] = self.wordBuf[from] / POWERS10[complement];
        }
        while from < stop {
            self.wordBuf[from] = self.wordBuf[from] % POWERS10[complement] * POWERS10[shift]
                + self.wordBuf[from + 1] / POWERS10[complement];
            from += 1;
        }
        self.wordBuf[from] = self.wordBuf[from] % POWERS10[complement] * POWERS10[shift];
    }

    fn mini_right_shift(&mut self, shift: usize, begin: usize, end: usize) {
        let mut from = (end - 1) / 9;
        let stop = begin / 9;
        let complement = 9 - shift;
        if 9 - ((end - 1) % 9 + 1) < shift {
            self.wordBuf[from + 1] = self.wordBuf[from] % POWERS10[shift] * POWERS10[complement];
        }
        while from > stop {
            self.wordBuf[from] = self.wordBuf[from] / POWERS10[shift]
                + self.wordBuf[from - 1] % POWERS10[shift] * POWERS10[complement];
            from -= 1;
        }
        self.wordBuf[from] /= POWERS10[shift];
    }

    /// 舍入到指定小数位 `frac`（负数表示舍到整数的 10 的幂次）。
    pub fn Round(
        &self,
        to: &mut MyDecimal,
        frac: isize,
        round_mode: RoundMode,
    ) -> Result<(), DecimalError> {
        self.round_into(to, frac, round_mode, false)
    }

    fn round_into(
        &self,
        to: &mut MyDecimal,
        mut frac: isize,
        round_mode: RoundMode,
        in_place: bool,
    ) -> Result<(), DecimalError> {
        // Keep Go's word-level rounding, including its unsupported ceiling
        // behavior and carry/truncation metadata. Numeric re-scaling alone
        // loses the reserved integer words and may overrun a full buffer.
        let mut words_frac_to = if frac > 0 {
            (frac + 8) / 9
        } else {
            (frac + 1) / 9
        };
        let words_frac = digits_to_words(self.digitsFrac as usize) as isize;
        let words_int = digits_to_words(self.digitsInt as usize) as isize;
        let mut status = Ok(());
        if words_int + words_frac_to > maxWordBufLen as isize {
            words_frac_to = maxWordBufLen as isize - words_int;
            frac = words_frac_to * 9;
            status = Err(DecimalError::Truncated);
        }
        if (self.digitsInt as isize).saturating_add(frac) < 0 {
            to.clear();
            return Ok(());
        }
        to.wordBuf = self.wordBuf;
        to.negative = self.negative;
        to.digitsInt = if in_place {
            self.digitsInt
        } else {
            (words_int.min(maxWordBufLen as isize) * 9) as i8
        };
        if words_frac_to > words_frac {
            for idx in words_int + words_frac..words_int + words_frac_to {
                to.wordBuf[idx as usize] = 0;
            }
            to.digitsFrac = frac as i8;
            to.resultFrac = to.digitsFrac;
            return status;
        }
        if frac >= self.digitsFrac as isize {
            to.digitsFrac = frac as i8;
            to.resultFrac = to.digitsFrac;
            return status;
        }
        let mut idx = words_int + words_frac_to - 1;
        if frac == words_frac_to * 9 {
            let increment = match round_mode {
                ModeCeiling => (idx + 1..=idx + words_frac - words_frac_to)
                    .any(|i| self.wordBuf[i as usize] != 0),
                ModeHalfUp => self.wordBuf[(idx + 1) as usize] / digMask >= 5,
                _ => false,
            };
            if increment {
                if idx >= 0 {
                    to.wordBuf[idx as usize] += 1;
                } else {
                    idx += 1;
                    to.wordBuf[idx as usize] = wordBase as i32;
                }
            } else if words_int + words_frac_to == 0 {
                to.clear();
                return Ok(());
            }
        } else {
            let pos = (words_frac_to * 9 - frac - 1) as usize;
            let mut shifted = to.wordBuf[idx as usize] / POWERS10[pos];
            let next = shifted % 10;
            if next > round_mode || (round_mode == ModeHalfUp && next == 5) {
                shifted += 10;
            }
            to.wordBuf[idx as usize] = POWERS10[pos] * (shifted - next);
        }
        if words_frac_to < words_frac {
            let start = if frac == 0 && words_int == 0 {
                1
            } else {
                words_int + words_frac_to
            };
            to.wordBuf[start as usize..].fill(0);
        }
        if to.wordBuf[idx as usize] >= wordBase as i32 {
            let mut carry = 1;
            to.wordBuf[idx as usize] -= wordBase as i32;
            while carry == 1 && idx > 0 {
                idx -= 1;
                let sum = to.wordBuf[idx as usize] + carry;
                carry = i32::from(sum >= wordBase as i32);
                to.wordBuf[idx as usize] = sum % wordBase as i32;
            }
            if carry > 0 {
                if words_int + words_frac_to >= maxWordBufLen as isize {
                    words_frac_to -= 1;
                    frac = words_frac_to * 9;
                    status = Err(DecimalError::Truncated);
                }
                for i in (1..=words_int + words_frac_to.max(0)).rev() {
                    if i < maxWordBufLen as isize {
                        to.wordBuf[i as usize] = to.wordBuf[i as usize - 1];
                    } else {
                        status = Err(DecimalError::Overflow);
                    }
                }
                idx = 0;
                to.wordBuf[0] = 1;
                if to.digitsInt < 81 {
                    to.digitsInt += 1;
                } else {
                    status = Err(DecimalError::Overflow);
                }
            }
        } else {
            while to.wordBuf[idx as usize] == 0 {
                if idx == 0 {
                    to.digitsInt = 1;
                    to.digitsFrac = frac.max(0) as i8;
                    to.negative = false;
                    to.wordBuf[..(words_frac_to + 1).max(0) as usize].fill(0);
                    to.resultFrac = to.digitsFrac;
                    return Ok(());
                }
                idx -= 1;
            }
        }
        let first_digit = to.digitsInt as usize % 9;
        if first_digit > 0 && to.wordBuf[idx as usize] >= POWERS10[first_digit] {
            to.digitsInt += 1;
        }
        to.digitsFrac = frac.max(0) as i8;
        to.resultFrac = to.digitsFrac;
        status
    }

    /// 从 Parquet 有符号大端整数 + scale 还原 DECIMAL，并消耗输入缓冲区。
    pub fn FromParquetArray(
        &mut self,
        buffer: &mut [u8],
        scale: isize,
    ) -> Result<(), DecimalError> {
        if buffer.is_empty() {
            self.clear();
            return Err(DecimalError::BadNumber);
        }
        self.negative = buffer[0] & 0x80 != 0;
        if self.negative {
            for byte in buffer.iter_mut() {
                *byte = !*byte;
            }
            for byte in buffer.iter_mut().rev() {
                *byte = byte.wrapping_add(1);
                if *byte != 0 {
                    break;
                }
            }
        }
        let mut start = 0;
        while start < buffer.len() && buffer[start] == 0 {
            start += 1;
        }
        let mut words = 0;
        while start < buffer.len() {
            let mut remainder = 0u64;
            for i in start..buffer.len() {
                let value = (remainder << 8) | u64::from(buffer[i]);
                let quotient = value / wordBase as u64;
                remainder = value % wordBase as u64;
                buffer[i] = quotient as u8;
                if quotient == 0 && i == start {
                    start += 1;
                }
            }
            if words >= maxWordBufLen {
                return Err(DecimalError::Overflow);
            }
            self.wordBuf[words] = remainder as i32;
            words += 1;
        }
        self.wordBuf[..words].reverse();
        self.digitsFrac = 0;
        self.resultFrac = 0;
        self.digitsInt = (words * 9) as i8;
        self.Shift(scale.saturating_neg())?;
        let source = self.clone();
        source.round_into(self, scale, ModeTruncate, true)
    }

    /// 从有符号整数构造（无小数位）。
    pub fn FromInt(&mut self, value: i64) -> &mut MyDecimal {
        if value < 0 {
            self.negative = true;
        }
        self.FromUint(value.unsigned_abs())
    }

    /// 从无符号整数构造（无小数位）。
    pub fn FromUint(&mut self, value: u64) -> &mut MyDecimal {
        let mut count = 1;
        let mut x = value;
        while x >= wordBase as u64 {
            count += 1;
            x /= wordBase as u64;
        }
        self.digitsFrac = 0;
        self.digitsInt = (count * 9) as i8;
        x = value;
        for idx in (0..count).rev() {
            self.wordBuf[idx] = (x % wordBase as u64) as i32;
            x /= wordBase as u64;
        }
        self
    }

    /// 转为 i64；溢出/有小数时返回错误码并给出截断值。
    pub fn ToInt(&self) -> (i64, Result<(), DecimalError>) {
        let scale = self.word_scale();
        let divisor = pow10_big(scale);
        let value = self.word_value();
        let integer = &value / &divisor;
        let converted = match integer.to_i64() {
            Some(converted) => converted,
            None if self.negative => i64::MIN,
            None => i64::MAX,
        };
        if integer.to_i64().is_none() {
            return (converted, Err(DecimalError::Overflow));
        }
        if !(value % divisor).is_zero() {
            (converted, Err(DecimalError::Truncated))
        } else {
            (converted, Ok(()))
        }
    }

    /// 转为 u64；负值直接 Overflow。
    pub fn ToUint(&self) -> (u64, Result<(), DecimalError>) {
        if self.negative {
            return (0, Err(DecimalError::Overflow));
        }
        let scale = self.word_scale();
        let divisor = pow10_big(scale);
        let value = self.word_value();
        let integer = &value / &divisor;
        let converted = match integer.to_u64() {
            Some(converted) => converted,
            None => return (u64::MAX, Err(DecimalError::Overflow)),
        };
        if !(value % divisor).is_zero() {
            (converted, Err(DecimalError::Truncated))
        } else {
            (converted, Ok(()))
        }
    }

    /// 按 Go 的最短浮点格式解析；非有限值返回 TruncatedWrongValue。
    pub fn FromFloat64(&mut self, value: f64) -> Result<(), DecimalError> {
        if !value.is_finite() {
            self.clear();
            return Err(DecimalError::TruncatedWrongValue);
        }
        let text = if value.abs() >= 1e6 || (value != 0.0 && value.abs() < 1e-4) {
            format!("{value:e}")
        } else {
            value.to_string()
        };
        self.FromString(text.as_bytes())
    }

    /// 经字符串解析为 f64。
    pub fn ToFloat64(&self) -> Result<f64, DecimalError> {
        if self.digitsInt as i16 + self.digitsFrac as i16 > 12 {
            return self
                .String()
                .parse::<f64>()
                .map_err(|_| DecimalError::Overflow);
        }
        // Go's fast path sums words and rounds at resultFrac. Use correctly
        // rounded decimal powers, as in its literal pow10off81 table.
        let power = |n: isize| format!("1e{n}").parse::<f64>().unwrap();
        let wi = digits_to_words(self.digitsInt.max(0) as usize);
        let wf = digits_to_words(self.digitsFrac.max(0) as usize);
        let mut value = 0.0;
        for i in 0..wi {
            value += f64::from(self.wordBuf[i]) * power(((wi - i - 1) * 9) as isize);
        }
        for i in 0..wf {
            value += f64::from(self.wordBuf[wi + i]) * power(-((i + 1) as isize * 9));
        }
        let unit = power(self.resultFrac as isize);
        value = (value * unit).round() / unit;
        Ok(if self.negative { -value } else { value })
    }

    /// 按 precision/frac 编码为 MySQL 可排序二进制 DECIMAL。
    pub fn ToBin(&self, precision: isize, frac: isize) -> (Vec<u8>, Result<(), DecimalError>) {
        self.WriteBin(precision, frac, Vec::new())
    }

    /// 将二进制编码追加写入已有缓冲；高位符号位翻转以保持字节序排序。
    pub fn WriteBin(
        &self,
        precision: isize,
        frac: isize,
        mut buffer: Vec<u8>,
    ) -> (Vec<u8>, Result<(), DecimalError>) {
        if precision < 0
            || precision as usize > digitsPerWord * maxWordBufLen
            || frac < 0
            || frac as usize > MAX_DECIMAL_SCALE
            || frac > precision
        {
            return (buffer, Err(DecimalError::BadNumber));
        }
        let precision = precision as usize;
        let frac = frac as usize;
        if precision == 0 {
            return (buffer, Err(DecimalError::BadNumber));
        }
        let digits_int = precision - frac;
        let source_scale = self.digitsFrac.max(0) as usize;
        let mut scaled = self.word_value();
        let mut status = Ok(());
        let stored_scale = self.word_scale();
        if stored_scale > frac {
            scaled /= pow10_big(stored_scale - frac);
        } else if stored_scale < frac {
            scaled *= pow10_big(frac - stored_scale);
        }
        let actual_int_digits = integer_digits(&scaled, frac);
        if actual_int_digits > digits_int {
            scaled %= pow10_big(precision);
            status = Err(DecimalError::Overflow);
        }
        let source_frac_size = source_scale / 9 * 4 + DIG2BYTES[source_scale % 9];
        let target_frac_size = frac / 9 * 4 + DIG2BYTES[frac % 9];
        if target_frac_size < source_frac_size
            || (target_frac_size == source_frac_size
                && (frac % 9 < source_scale % 9 || frac / 9 < source_scale / 9))
        {
            status = Err(DecimalError::Truncated);
        }
        let negative =
            self.negative && (self.significant_integer_digits() != 0 || source_scale > 0);
        let mut digits = scaled.abs().to_str_radix(10);
        if digits.len() < precision {
            digits = format!("{}{}", "0".repeat(precision - digits.len()), digits);
        } else if digits.len() > precision {
            digits = digits[digits.len() - precision..].to_string();
        }

        let size = DecimalBinSize(precision as isize, frac as isize).unwrap();
        let start = buffer.len();
        buffer.resize(start + size, 0);
        let output = &mut buffer[start..];
        let mut byte_index = 0usize;
        let mut digit_index = 0usize;
        let leading = digits_int % digitsPerWord;
        if leading > 0 {
            let value = digits[..leading].parse::<u32>().unwrap_or(0);
            write_word(&mut output[byte_index..], value, DIG2BYTES[leading]);
            byte_index += DIG2BYTES[leading];
            digit_index += leading;
        }
        for _ in 0..(digits_int / digitsPerWord) {
            let value = digits[digit_index..digit_index + digitsPerWord]
                .parse::<u32>()
                .unwrap_or(0);
            write_word(&mut output[byte_index..], value, wordSize);
            byte_index += wordSize;
            digit_index += digitsPerWord;
        }
        for _ in 0..(frac / digitsPerWord) {
            let value = digits[digit_index..digit_index + digitsPerWord]
                .parse::<u32>()
                .unwrap_or(0);
            write_word(&mut output[byte_index..], value, wordSize);
            byte_index += wordSize;
            digit_index += digitsPerWord;
        }
        let trailing = frac % digitsPerWord;
        if trailing > 0 {
            let value = digits[digit_index..digit_index + trailing]
                .parse::<u32>()
                .unwrap_or(0);
            write_word(&mut output[byte_index..], value, DIG2BYTES[trailing]);
        }
        // MySQL 二进制 DECIMAL：按位取反表示负数，再翻转最高位使字节序可比较
        if negative {
            for byte in output.iter_mut() {
                *byte = !*byte;
            }
        }
        output[0] ^= 0x80;
        (buffer, status)
    }

    /// 生成去尾零后的二进制 hash key（末尾附加小数位数）。
    pub fn ToHashKey(&self) -> Result<Vec<u8>, DecimalError> {
        let digits_int = self.significant_integer_digits();
        let digits_frac = self.significant_fraction_digits();
        let precision = (digits_int + digits_frac).max(1);
        let (mut buffer, status) = self.ToBin(precision as isize, digits_frac as isize);
        if let Err(error) = status {
            if error != DecimalError::Truncated {
                return Err(error);
            }
        }
        buffer.push(digits_frac as u8);
        Ok(buffer)
    }

    /// 预估 ToHashKey 字节长度。
    pub fn HashKeySize(&self) -> Result<usize, DecimalError> {
        let digits_int = self.significant_integer_digits();
        let digits_frac = self.significant_fraction_digits();
        let precision = (digits_int + digits_frac).max(1);
        Ok(DecimalBinSize(precision as isize, digits_frac as isize)? + 1)
    }

    /// 返回 (总精度, 小数位数)。
    pub fn PrecisionAndFrac(&self) -> (usize, usize) {
        let frac = self.digitsFrac.max(0) as usize;
        ((self.significant_integer_digits() + frac).max(1), frac)
    }

    /// wordBuf 全零即为零值。
    pub fn IsZero(&self) -> bool {
        self.wordBuf.iter().all(|word| *word == 0)
    }

    /// 从 MySQL 二进制 DECIMAL 解码；返回消耗字节数与状态。
    pub fn FromBin(
        &mut self,
        encoded: &[u8],
        precision: isize,
        frac: isize,
    ) -> (usize, Result<(), DecimalError>) {
        if encoded.is_empty() {
            self.clear();
            return (0, Err(DecimalError::BadNumber));
        }
        let size = match DecimalBinSize(precision, frac) {
            Ok(size) if size > 0 && size <= 40 => size,
            _ => return (0, Err(DecimalError::BadNumber)),
        };
        let int_digits = (precision - frac) as usize;
        let wi = int_digits / 9;
        let leading = int_digits % 9;
        let mut wf = frac as usize / 9;
        let mut trailing = frac as usize % 9;
        let int_words = wi + usize::from(leading > 0);
        let frac_words = wf + usize::from(trailing > 0);
        // Go's malformed integer-overflow path can index outside wordBuf;
        // reject that invalid encoding before indexing in Rust.
        if int_words > maxWordBufLen {
            self.clear();
            return (size, Err(DecimalError::Overflow));
        }
        let status = if int_words + frac_words > maxWordBufLen {
            wf = maxWordBufLen - int_words;
            trailing = 0;
            Err(DecimalError::Truncated)
        } else {
            Ok(())
        };
        let mut raw = [0u8; 40];
        let count = encoded.len().min(size);
        raw[..count].copy_from_slice(&encoded[..count]);
        let mask = if raw[0] & 0x80 == 0 { -1i32 } else { 0 };
        raw[0] ^= 0x80;
        let mut offset = 0usize;
        let mut next = |bytes: usize| {
            let mut word = if raw[offset] & 0x80 != 0 { -1i32 } else { 0 };
            for &byte in &raw[offset..offset + bytes] {
                word = (word << 8) | i32::from(byte);
            }
            offset += bytes;
            word ^ mask
        };
        self.negative = mask != 0;
        self.digitsInt = int_digits as i8;
        self.digitsFrac = (wf * 9 + trailing) as i8;
        let mut idx = 0usize;
        if leading > 0 {
            let word = next(DIG2BYTES[leading]);
            self.wordBuf[idx] = word;
            if word < 0 || i64::from(word) >= 10i64.pow(leading as u32 + 1) {
                self.clear();
                return (size, Err(DecimalError::BadNumber));
            }
            if word != 0 {
                idx += 1;
            } else {
                self.digitsInt -= leading as i8;
            }
        }
        for _ in 0..wi {
            let word = next(4);
            self.wordBuf[idx] = word;
            if !(0..=wordMax).contains(&word) {
                self.clear();
                return (size, Err(DecimalError::BadNumber));
            }
            if idx > 0 || word != 0 {
                idx += 1;
            } else {
                self.digitsInt -= 9;
            }
        }
        for _ in 0..wf {
            let word = next(4);
            self.wordBuf[idx] = word;
            if !(0..=wordMax).contains(&word) {
                self.clear();
                return (size, Err(DecimalError::BadNumber));
            }
            idx += 1;
        }
        if trailing > 0 {
            let word = next(DIG2BYTES[trailing]).wrapping_mul(POWERS10[9 - trailing]);
            self.wordBuf[idx] = word;
            if !(0..=wordMax).contains(&word) {
                self.clear();
                return (size, Err(DecimalError::BadNumber));
            }
        }
        if self.digitsInt == 0 && self.digitsFrac == 0 {
            self.clear();
        }
        self.resultFrac = frac as i8;
        (size, status)
    }

    /// 比较两个 DECIMAL，返回 -1/0/1。
    pub fn Compare(&self, other: &MyDecimal) -> isize {
        if self.negative != other.negative {
            return if self.negative { -1 } else { 1 };
        }
        let scale = (self.word_scale()).max(other.word_scale());
        let left = self.word_value() * pow10_big(scale - self.word_scale());
        let right = other.word_value() * pow10_big(scale - other.word_scale());
        match left.cmp(&right) {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        }
    }

    /// 将内部字段序列化为 JSON 对象。
    pub fn MarshalJSON(&self) -> Result<Vec<u8>, DecimalError> {
        Ok(format!("{{\"DigitsInt\":{},\"DigitsFrac\":{},\"ResultFrac\":{},\"Negative\":{},\"WordBuf\":{}}}",
            self.digitsInt, self.digitsFrac, self.resultFrac, self.negative,
            serde_json::to_string(&self.wordBuf).map_err(|_| DecimalError::InvalidJson)?
        ).into_bytes())
    }

    /// 从 JSON 对象还原内部字段。
    pub fn UnmarshalJSON(&mut self, data: &[u8]) -> Result<(), DecimalError> {
        let value: Value = serde_json::from_slice(data).map_err(|_| DecimalError::InvalidJson)?;
        let mut replacement = MyDecimal::default();
        if value.is_null() {
            *self = replacement;
            return Ok(());
        }
        value.as_object().ok_or(DecimalError::InvalidJson)?;
        // Decode members separately so duplicate keys retain Go's input order.
        let mut rest = data
            .iter()
            .position(|&b| b == b'{')
            .map(|i| &data[i + 1..])
            .unwrap();
        let mut fields = Vec::new();
        loop {
            rest = rest.trim_ascii_start();
            if rest.starts_with(b"}") {
                break;
            }
            let mut keys = serde_json::Deserializer::from_slice(rest).into_iter::<String>();
            let key = keys
                .next()
                .ok_or(DecimalError::InvalidJson)?
                .map_err(|_| DecimalError::InvalidJson)?;
            rest = rest[keys.byte_offset()..].trim_ascii_start();
            rest = &rest[1..]; // colon, already checked by the whole-value parse
            let mut values = serde_json::Deserializer::from_slice(rest).into_iter::<Value>();
            let field = values
                .next()
                .ok_or(DecimalError::InvalidJson)?
                .map_err(|_| DecimalError::InvalidJson)?;
            rest = rest[values.byte_offset()..].trim_ascii_start();
            fields.push((key, field));
            if rest.starts_with(b",") {
                rest = &rest[1..];
            }
        }
        // encoding/json accepts absent/null fields, case-insensitive names,
        // short arrays (zero filled), and extra array entries (discarded).
        for (name, value) in fields {
            if value.is_null() {
                continue;
            }
            match name.to_ascii_lowercase().as_str() {
                "digitsint" | "digitsfrac" | "resultfrac" => {
                    let number = value
                        .as_i64()
                        .and_then(|n| i8::try_from(n).ok())
                        .ok_or(DecimalError::InvalidJson)?;
                    match name.to_ascii_lowercase().as_str() {
                        "digitsint" => replacement.digitsInt = number,
                        "digitsfrac" => replacement.digitsFrac = number,
                        _ => replacement.resultFrac = number,
                    }
                }
                "negative" => {
                    replacement.negative = value.as_bool().ok_or(DecimalError::InvalidJson)?
                }
                "wordbuf" => {
                    let words = value.as_array().ok_or(DecimalError::InvalidJson)?;
                    // Array decoding zero-fills the unused tail.
                    let previous = replacement.wordBuf;
                    replacement.wordBuf.fill(0);
                    for (i, (target, source)) in
                        replacement.wordBuf.iter_mut().zip(words).enumerate()
                    {
                        if source.is_null() {
                            *target = previous[i];
                        }
                        if !source.is_null() {
                            *target = source
                                .as_i64()
                                .and_then(|n| i32::try_from(n).ok())
                                .ok_or(DecimalError::InvalidJson)?;
                        }
                    }
                }
                _ => {}
            }
        }
        *self = replacement;
        Ok(())
    }
}

/// 按大端写入 `size` 字节的 word 片段。
fn write_word(buffer: &mut [u8], value: u32, size: usize) {
    let bytes = value.to_be_bytes();
    buffer[..size].copy_from_slice(&bytes[wordSize - size..]);
}

/// 按大端读取 `size` 字节的 word 片段。
fn read_word(buffer: &[u8], size: usize) -> u32 {
    let mut bytes = [0u8; wordSize];
    bytes[wordSize - size..].copy_from_slice(&buffer[..size]);
    u32::from_be_bytes(bytes)
}

/// 计算给定 precision/frac 的二进制 DECIMAL 字节长度。
pub fn DecimalBinSize(precision: isize, frac: isize) -> Result<usize, DecimalError> {
    if precision < 0 || frac < 0 || frac > precision {
        return Err(DecimalError::BadNumber);
    }
    let digits_int = (precision - frac) as usize;
    let frac = frac as usize;
    let words_int = digits_int / digitsPerWord;
    let words_frac = frac / digitsPerWord;
    let extra_int = digits_int % digitsPerWord;
    let extra_frac = frac % digitsPerWord;
    Ok(words_int * wordSize + DIG2BYTES[extra_int] + words_frac * wordSize + DIG2BYTES[extra_frac])
}

/// 从带 precision/frac 头的缓冲窥探完整编码长度。
pub fn DecimalPeak(buffer: &[u8]) -> Result<usize, DecimalError> {
    if buffer.len() < 3 {
        return Err(DecimalError::BadNumber);
    }
    Ok(DecimalBinSize(buffer[0] as isize, buffer[1] as isize)? + 2)
}

/// 取反；零值保持非负。
pub fn DecimalNeg(from: &MyDecimal) -> MyDecimal {
    let mut result = from.clone();
    if !result.IsZero() {
        result.negative = !result.negative;
    }
    result
}

/// 将两个操作数对齐到相同小数位，返回 (左, 右, 公共 scale)。
fn align_values(left: &MyDecimal, right: &MyDecimal) -> (BigInt, BigInt, usize) {
    let left_scale = left.word_scale();
    let right_scale = right.word_scale();
    let scale = left_scale.max(right_scale);
    (
        left.word_value() * pow10_big(scale - left_scale),
        right.word_value() * pow10_big(scale - right_scale),
        scale,
    )
}

/// 精确加法；溢出时将结果置为最大 DECIMAL 并返回 Overflow。
pub fn DecimalAdd(
    from1: &MyDecimal,
    from2: &MyDecimal,
    to: &mut MyDecimal,
) -> Result<(), DecimalError> {
    decimal_add_sub(from1, from2, to, false)
}

/// Subtraction shares Go's doAdd/doSub dispatch and operand truncation.
pub fn DecimalSub(
    from1: &MyDecimal,
    from2: &MyDecimal,
    to: &mut MyDecimal,
) -> Result<(), DecimalError> {
    decimal_add_sub(from1, from2, to, true)
}

fn decimal_add_sub(
    a: &MyDecimal,
    b: &MyDecimal,
    to: &mut MyDecimal,
    subtract: bool,
) -> Result<(), DecimalError> {
    to.clear();
    let result_frac = a.resultFrac.max(b.resultFrac);
    to.resultFrac = result_frac;
    let adding = a.negative == (b.negative != subtract);
    let aw = digits_to_words(a.digitsInt as usize);
    let bw = digits_to_words(b.digitsInt as usize);
    let mut iw = if adding {
        aw.max(bw)
    } else {
        let leading =
            |d: &MyDecimal, w: usize| w - d.wordBuf[..w].iter().take_while(|&&x| x == 0).count();
        leading(a, aw).max(leading(b, bw))
    };
    if adding {
        let high = if aw > bw {
            a.wordBuf[0]
        } else if bw > aw {
            b.wordBuf[0]
        } else {
            a.wordBuf[0] + b.wordBuf[0]
        };
        if high > wordMax - 1 {
            iw += 1;
        }
    } else if a.word_value().abs() * pow10_big(b.word_scale())
        == b.word_value().abs() * pow10_big(a.word_scale())
    {
        to.digitsFrac = result_frac;
        return Ok(());
    }
    if iw > maxWordBufLen {
        maxDecimal(81, 0, to);
        to.resultFrac = result_frac;
        return Err(DecimalError::Overflow);
    }
    let requested = a.digitsFrac.max(b.digitsFrac) as usize;
    let scale = requested.min((maxWordBufLen - iw) * 9);
    // Go truncates each operand before the addition/subtraction: discarded
    // words cannot contribute a carry or borrow to the retained result.
    let rescale = |d: &MyDecimal| {
        let value = d.word_value();
        let old = d.word_scale();
        let scale = digits_to_words(scale) * 9;
        if old > scale {
            value / pow10_big(old - scale)
        } else {
            value * pow10_big(scale - old)
        }
    };
    let left = rescale(a);
    let right = rescale(b);
    let value = if subtract { left - right } else { left + right };
    to.set_unscaled(value, digits_to_words(scale) * 9);
    to.digitsFrac = scale as i8;
    // Preserve the integer word reservation used by subsequent Round/ToBin.
    let current = digits_to_words(to.digitsInt as usize);
    if current < iw {
        to.wordBuf
            .copy_within(..maxWordBufLen - (iw - current), iw - current);
        to.wordBuf[..iw - current].fill(0);
    }
    to.digitsInt = (iw * 9) as i8;
    to.resultFrac = result_frac;
    if adding {
        to.negative = a.negative;
    }
    if scale < requested {
        Err(DecimalError::Truncated)
    } else {
        Ok(())
    }
}

/// 精确乘法；缓冲不足时裁剪小数 word，可能 Truncated。
pub fn DecimalMul(a: &MyDecimal, b: &MyDecimal, to: &mut MyDecimal) -> Result<(), DecimalError> {
    to.clear();
    let wi1 = digits_to_words(a.digitsInt as usize) as isize;
    let wi2 = digits_to_words(b.digitsInt as usize) as isize;
    let mut wf1 = digits_to_words(a.digitsFrac as usize) as isize;
    let mut wf2 = digits_to_words(b.digitsFrac as usize) as isize;
    let requested_i = digits_to_words(a.digitsInt as usize + b.digitsInt as usize);
    let wi = requested_i.min(maxWordBufLen);
    let wf = (wf1 + wf2).min((maxWordBufLen - wi) as isize);
    to.resultFrac = a
        .resultFrac
        .wrapping_add(b.resultFrac)
        .min(MAX_DECIMAL_SCALE as i8);
    to.negative = a.negative != b.negative;
    to.digitsFrac = a.digitsFrac.wrapping_add(b.digitsFrac).min(notFixedDec);
    to.digitsInt = (wi * 9) as i8;
    if requested_i > maxWordBufLen {
        return Err(DecimalError::Overflow);
    }
    let status = if wf < wf1 + wf2 {
        Err(DecimalError::Truncated)
    } else {
        Ok(())
    };
    if status.is_err() {
        to.digitsFrac = to.digitsFrac.min((wf * 9) as i8);
        let removed = wf1 + wf2 - wf;
        let first = removed >> 1;
        if wf1 <= wf2 {
            wf1 -= first;
            wf2 -= removed - first;
        } else {
            wf2 -= first;
            wf1 -= removed - first;
        }
    }
    let mut start_to = wi as isize + wf - 1;
    for i in (0..wi1 + wf1).rev() {
        let mut carry = 0i64;
        let mut out = start_to;
        for j in (0..wi2 + wf2).rev() {
            let product = i64::from(a.wordBuf[i as usize]) * i64::from(b.wordBuf[j as usize]);
            let sum = i64::from(to.wordBuf[out as usize]) + product % wordBase + carry;
            to.wordBuf[out as usize] = (sum % wordBase) as i32;
            carry = sum / wordBase + product / wordBase;
            out -= 1;
        }
        while carry > 0 {
            if out < 0 {
                return Err(DecimalError::Overflow);
            }
            let sum = i64::from(to.wordBuf[out as usize]) + carry;
            to.wordBuf[out as usize] = (sum % wordBase) as i32;
            carry = sum / wordBase;
            out -= 1;
        }
        start_to -= 1;
    }
    if to.negative && to.wordBuf[..wi + wf as usize].iter().all(|&x| x == 0) {
        let frac = to.resultFrac;
        to.clear();
        to.digitsFrac = frac;
        to.resultFrac = frac;
    }
    let mut start = 0;
    let mut count = wi + digits_to_words(to.digitsFrac.max(0) as usize);
    while to.wordBuf[start] == 0 && to.digitsInt > 9 {
        start += 1;
        to.digitsInt -= 9;
        count -= 1;
    }
    if start > 0 {
        to.wordBuf.copy_within(start..start + count, 0);
    }
    status
}

/// 精确除法；`frac_incr` 控制额外保留的小数位，除零返回 DivByZero。
pub fn DecimalDiv(
    from1: &MyDecimal,
    from2: &MyDecimal,
    to: &mut MyDecimal,
    frac_incr: isize,
) -> Result<(), DecimalError> {
    to.clear();
    to.resultFrac = (from1.resultFrac as isize + frac_incr).min(MAX_DECIMAL_SCALE as isize) as i8;
    // validateArgs resets the destination even on division by zero.
    if from2.word_value().is_zero() {
        return Err(DecimalError::DivByZero);
    }
    if from1.word_value().is_zero() {
        to.digitsFrac = to.resultFrac;
        return Ok(());
    }
    let scale1 = from1.digitsFrac.max(0) as usize;
    let scale2 = from2.digitsFrac.max(0) as usize;
    let rounded_scale1 = digits_to_words(scale1) * digitsPerWord;
    let rounded_scale2 = digits_to_words(scale2) * digitsPerWord;
    let padding = rounded_scale1 - scale1 + rounded_scale2 - scale2;
    let adjusted_increment = (frac_incr.max(0) as usize).saturating_sub(padding);
    let requested = rounded_scale1 + rounded_scale2 + adjusted_increment;
    let leading = |d: &MyDecimal| *d.wordBuf.iter().find(|&&w| w != 0).unwrap();
    let int_estimate = decimal_digits(&from1.unscaled()) as isize
        - scale1 as isize
        - (decimal_digits(&from2.word_value()) as isize - from2.word_scale() as isize)
        + isize::from(leading(from1) >= leading(from2));
    let requested_int_words = digits_to_words(int_estimate.max(0) as usize);
    let int_words = requested_int_words.min(maxWordBufLen);
    let requested_frac_words = digits_to_words(requested);
    let frac_words = requested_frac_words.min(maxWordBufLen - int_words);
    let output_scale = frac_words * digitsPerWord;
    let numerator = from1.word_value() * pow10_big(from2.word_scale() + output_scale);
    let denominator = from2.word_value() * pow10_big(from1.word_scale());
    let mut quotient = numerator / denominator;
    if requested_int_words > maxWordBufLen {
        quotient /= pow10_big((requested_int_words - maxWordBufLen) * digitsPerWord);
    }
    let result_frac = to.resultFrac;
    let negative = from1.negative != from2.negative;
    let mut raw = quotient.abs();
    for i in (0..int_words + frac_words).rev() {
        to.wordBuf[i] = (&raw % wordBase).to_i32().unwrap();
        raw /= wordBase;
    }
    let start = to.wordBuf[..int_words]
        .iter()
        .take_while(|&&w| w == 0)
        .count();
    to.digitsInt = integer_digits(&quotient, output_scale) as i8;
    if quotient.is_zero() {
        to.digitsInt = 0;
    }
    if start > 0 {
        to.wordBuf.copy_within(start.., 0);
    }
    to.digitsFrac = output_scale as i8;
    to.resultFrac = result_frac;
    to.negative = negative && !to.IsZero();
    if requested_int_words > maxWordBufLen {
        Err(DecimalError::Overflow)
    } else if frac_words < requested_frac_words {
        Err(DecimalError::Truncated)
    } else {
        Ok(())
    }
}

/// 取模（余数），除零返回 DivByZero。
pub fn DecimalMod(
    from1: &MyDecimal,
    from2: &MyDecimal,
    to: &mut MyDecimal,
) -> Result<(), DecimalError> {
    to.clear();
    to.resultFrac = from1.resultFrac.max(from2.resultFrac);
    if from2.word_value().is_zero() {
        return Err(DecimalError::DivByZero);
    }
    if from1.word_value().is_zero() {
        to.digitsFrac = to.resultFrac;
        return Ok(());
    }
    let (left, right, scale) = align_values(from1, from2);
    let status = to.fit_unscaled(left % right, scale, ModeTruncate);
    to.digitsFrac = to.digitsFrac.min(from1.digitsFrac.max(from2.digitsFrac));
    to.resultFrac = from1.resultFrac.max(from2.resultFrac);
    status
}

/// 构造指定 precision/frac 的全 9 最大正 DECIMAL。
fn maxDecimal(precision: usize, frac: usize, to: &mut MyDecimal) {
    if precision == 0 || frac > precision {
        to.clear();
        return;
    }
    let integer = "9".repeat(precision - frac);
    let fraction = "9".repeat(frac);
    to.set_parts(&integer, &fraction, false);
}

/// 从 i64 构造 MyDecimal。
pub fn NewDecFromInt(value: i64) -> MyDecimal {
    let mut decimal = MyDecimal::default();
    decimal.FromInt(value);
    decimal
}

/// 从 u64 构造 MyDecimal。
pub fn NewDecFromUint(value: u64) -> MyDecimal {
    let mut decimal = MyDecimal::default();
    decimal.FromUint(value);
    decimal
}

/// 测试辅助：从 f64 构造，失败则 panic。
pub fn NewDecFromFloatForTest(value: f64) -> MyDecimal {
    let mut decimal = MyDecimal::default();
    decimal
        .FromFloat64(value)
        .expect("test float must be representable as MyDecimal");
    decimal
}

/// 测试辅助：从字符串构造，失败则 panic。
pub fn NewDecFromStringForTest(value: &str) -> MyDecimal {
    let mut decimal = MyDecimal::default();
    decimal
        .FromString(value.as_bytes())
        .expect("test string must be a valid MyDecimal");
    decimal
}

/// 构造指定精度的最大正数或最小负数（全 9）。
pub fn NewMaxOrMinDec(negative: bool, precision: usize, frac: usize) -> MyDecimal {
    let mut decimal = MyDecimal::default();
    if frac <= precision {
        let text = format!(
            "{}{}.{}",
            if negative { "-" } else { "+" },
            "9".repeat(precision - frac),
            "9".repeat(frac)
        );
        let _ = decimal.FromString(text.as_bytes());
    }
    decimal
}
