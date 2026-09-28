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

// MySQL SET 类型表示与解析，对齐 Go `types` 包。
//
// SET 用位掩码 `Value` 标记成员集合，`Name` 为逗号分隔的规范化成员名；
// 解析时先按校对规则匹配名称，失败再尝试把输入当作整数掩码。

use std::collections::HashSet;
use std::fmt;
use std::sync::LazyLock;

use collate::GetCollator;

/// 空 SET：Name 为空串，Value 为 0。
pub static zeroSet: LazyLock<Set> = LazyLock::new(|| Set {
    Name: String::new(),
    Value: 0,
});

// Set is for MySQL Set type.
/// MySQL SET 值：规范化名称与位掩码。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Set {
    /// 逗号分隔、按定义顺序排列的成员名。
    pub Name: String,
    /// 位掩码：第 i 位对应 `elems[i]`。
    pub Value: u64,
}

impl Set {
    // String implements fmt.Stringer interface.
    /// 返回规范化成员名字符串。
    pub fn String(&self) -> String {
        self.Name.clone()
    }

    // ToNumber changes Set to float64 for numeric operation.
    /// 将位掩码转为 float64，供数值运算使用。
    pub fn ToNumber(&self) -> f64 {
        self.Value as f64
    }

    // Copy deep copy a Set.
    /// 深拷贝（clone）。
    pub fn Copy(&self) -> Set {
        self.clone()
    }
}

impl fmt::Display for Set {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.Name)
    }
}

/// SET 解析失败错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetError(String);

impl fmt::Display for SetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SetError {}

// ParseSet creates a Set with name or value.
/// 先按名称解析，失败则尝试把 `name` 当作整数掩码解析。
pub fn ParseSet(elems: &[String], name: &str, collation: &str) -> Result<Set, SetError> {
    if let Ok(set_name) = ParseSetName(elems, name, collation) {
        return Ok(set_name);
    }

    if let Some(number) = parse_uint_base_zero(name) {
        return ParseSetValue(elems, number);
    }

    Err(SetError(format!(
        "item {} is not in Set {}",
        name,
        format_elems(elems)
    )))
}

// ParseSetName creates a Set with name.
/// 按逗号拆分输入，用校对键匹配 `elems` 成员并组装位掩码。
pub fn ParseSetName(elems: &[String], name: &str, collation: &str) -> Result<Set, SetError> {
    if name.is_empty() {
        return Ok(zeroSet.Copy());
    }

    let collator = GetCollator(collation);
    // 先收集输入各段的校对键，再按 elems 定义顺序消解并置位
    let mut marked: HashSet<Vec<u8>> = name.split(',').map(|part| collator.Key(part)).collect();
    let mut items = Vec::with_capacity(marked.len());
    let mut value = 0_u64;

    for (index, elem) in elems.iter().enumerate() {
        if marked.remove(&collator.Key(elem)) {
            value |= 1_u64 << index;
            items.push(elem.clone());
        }
    }

    if marked.is_empty() {
        return Ok(Set {
            Name: items.join(","),
            Value: value,
        });
    }

    Err(SetError(format!(
        "item {} is not in Set {}",
        name,
        format_elems(elems)
    )))
}

/// 编译期构造每位 1<<i 的掩码表。
const fn build_set_index_value() -> [u64; 64] {
    let mut values = [0_u64; 64];
    let mut index = 0;
    while index < values.len() {
        values[index] = 1_u64 << index;
        index += 1;
    }
    values
}

/// 编译期构造每位取反掩码表，用于清除已匹配位。
const fn build_set_index_invert_value() -> [u64; 64] {
    let mut values = [0_u64; 64];
    let mut index = 0;
    while index < values.len() {
        values[index] = !(1_u64 << index);
        index += 1;
    }
    values
}

/// 第 i 位为 1 的掩码常量表。
pub static setIndexValue: [u64; 64] = build_set_index_value();
/// 第 i 位为 0、其余为 1 的取反掩码表。
pub static setIndexInvertValue: [u64; 64] = build_set_index_invert_value();

// The Go init function fills the two mask tables. Rust initializes them at
// compile time, so calling init only confirms that initialization has happened.
/// 对应 Go init：确认掩码表已初始化（Rust 在编译期完成）。
pub fn init() {
    let _ = (&setIndexValue, &setIndexInvertValue);
}

// ParseSetValue creates a Set with special number.
/// 按位掩码数值解析 SET；残留未映射位则报错。
pub fn ParseSetValue(elems: &[String], mut number: u64) -> Result<Set, SetError> {
    if number == 0 {
        return Ok(zeroSet.Copy());
    }

    let value = number;
    let mut items = Vec::new();
    for (index, elem) in elems.iter().enumerate() {
        if number & setIndexValue[index] != 0 {
            items.push(elem.clone());
            number &= setIndexInvertValue[index];
        }
    }

    // 仍有未映射到 elems 的高位则非法
    if number != 0 {
        return Err(SetError(format!(
            "invalid number {} for Set {}",
            number,
            format_elems(elems)
        )));
    }

    Ok(Set {
        Name: items.join(","),
        Value: value,
    })
}

/// 错误信息中的 elems 列表格式化。
fn format_elems(elems: &[String]) -> String {
    format!("[{}]", elems.join(" "))
}

/// 解析 Go `strconv.ParseUint(s, 0, 64)` 风格的无符号整数（含 0x/0b/0o 与下划线）。
fn parse_uint_base_zero(input: &str) -> Option<u64> {
    let input = input.strip_prefix('+').unwrap_or(input);
    if input.is_empty() || input.starts_with('-') {
        return None;
    }

    // 按前缀选择进制；单独前导 0 走八进制（与 Go ParseUint base=0 一致）
    let (radix, digits, prefix_allows_underscore) = if let Some(rest) = input
        .strip_prefix("0x")
        .or_else(|| input.strip_prefix("0X"))
    {
        (16, rest, true)
    } else if let Some(rest) = input
        .strip_prefix("0b")
        .or_else(|| input.strip_prefix("0B"))
    {
        (2, rest, true)
    } else if let Some(rest) = input
        .strip_prefix("0o")
        .or_else(|| input.strip_prefix("0O"))
    {
        (8, rest, true)
    } else if input.len() > 1 && input.starts_with('0') {
        (8, &input[1..], true)
    } else {
        (10, input, false)
    };

    let mut value = 0_u64;
    let mut saw_digit = false;
    let mut previous_was_digit = prefix_allows_underscore;
    for byte in digits.bytes() {
        if byte == b'_' {
            if !previous_was_digit {
                return None;
            }
            previous_was_digit = false;
            continue;
        }

        let digit = match byte {
            b'0'..=b'9' => u32::from(byte - b'0'),
            b'a'..=b'f' => u32::from(byte - b'a') + 10,
            b'A'..=b'F' => u32::from(byte - b'A') + 10,
            _ => return None,
        };
        if digit >= radix {
            return None;
        }
        value = value
            .checked_mul(u64::from(radix))?
            .checked_add(u64::from(digit))?;
        saw_digit = true;
        previous_was_digit = true;
    }

    (saw_digit && previous_was_digit).then_some(value)
}
