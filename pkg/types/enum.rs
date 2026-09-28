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

// MySQL ENUM 类型：名称/序号解析与字符串、数值表示。
//
// ENUM 成员从 1 起编号；解析时先按校对规则匹配名称，
// 失败则尝试把输入当作整数（支持 0x/0b/0o 等进制）再按序号取值。

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// ENUM 取值：Name 为成员名，Value 为 1 起的序号。
pub struct Enum {
    pub Name: String,
    pub Value: u64,
}

impl Enum {
    /// 深拷贝 Name 字符串，Value 按值复制。
    pub fn Copy(&self) -> Enum {
        Enum {
            Name: self.Name.clone(),
            Value: self.Value,
        }
    }

    /// 返回成员名（显示用）。
    pub fn String(&self) -> String {
        self.Name.clone()
    }

    /// 返回序号的浮点形式，供数值上下文使用。
    pub fn ToNumber(&self) -> f64 {
        self.Value as f64
    }
}

impl std::fmt::Display for Enum {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.Name)
    }
}

/// 先按名解析，再尝试把 name 当无符号整数按序号解析。
pub fn ParseEnum(
    elems: &[String],
    name: &str,
    collation: &str,
) -> Result<Enum, errors::SharedError> {
    if let Ok(value) = ParseEnumName(elems, name, collation) {
        return Ok(value);
    }
    if let Some(number) = parse_uint_base_zero(name) {
        return ParseEnumValue(elems, number);
    }
    Err(enum_name_error(elems, name))
}

/// 用指定校对规则在 elems 中查找 name，命中则 Value=下标+1。
pub fn ParseEnumName(
    elems: &[String],
    name: &str,
    collation: &str,
) -> Result<Enum, errors::SharedError> {
    let collator = collate::GetCollator(collation);
    for (index, element) in elems.iter().enumerate() {
        if collator.Compare(element, name) == 0 {
            return Ok(Enum {
                Name: element.clone(),
                Value: index as u64 + 1,
            });
        }
    }
    Err(enum_name_error(elems, name))
}

/// 按 1..len(elems) 序号取成员；越界返回截断类错误。
pub fn ParseEnumValue(elems: &[String], number: u64) -> Result<Enum, errors::SharedError> {
    if number == 0 || number > elems.len() as u64 {
        return Err(wrap_truncated(format!(
            "convert to MySQL enum failed: number {number} overflow enum boundary [1, {}]",
            elems.len()
        )));
    }
    Ok(Enum {
        Name: elems[(number - 1) as usize].clone(),
        Value: number,
    })
}

/// 构造「名称不在 ENUM 列表」的截断错误。
fn enum_name_error(elems: &[String], name: &str) -> errors::SharedError {
    wrap_truncated(format!(
        "convert to MySQL enum failed: item {name} is not in enum [{}]",
        elems.join(" ")
    ))
}

/// 以 ErrTruncated 为 cause 包装消息。
fn wrap_truncated(message: String) -> errors::SharedError {
    let cause = errors::SharedError::new((**ErrTruncated).clone());
    errors::Wrap(Some(cause), message).expect("a present error remains present")
}

/// 解析无符号整数，支持 0x/0b/0o 前缀与前导 0 八进制（对齐 Go ParseUint base=0）。
fn parse_uint_base_zero(value: &str) -> Option<u64> {
    if value.is_empty() || value.starts_with(['+', '-']) {
        return None;
    }

    // 按前缀选择进制；无前缀且以 0 开头长度>1 时视为八进制
    let (digits, radix, has_base_prefix) = if let Some(digits) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        (digits, 16, true)
    } else if let Some(digits) = value
        .strip_prefix("0b")
        .or_else(|| value.strip_prefix("0B"))
    {
        (digits, 2, true)
    } else if let Some(digits) = value
        .strip_prefix("0o")
        .or_else(|| value.strip_prefix("0O"))
    {
        (digits, 8, true)
    } else if value.len() > 1 && value.starts_with('0') {
        (&value[1..], 8, true)
    } else {
        (value, 10, false)
    };

    if digits.is_empty()
        || (digits.starts_with('_') && !has_base_prefix)
        || digits.ends_with('_')
        || digits.contains("__")
    {
        return None;
    }
    let digits = digits.replace('_', "");
    u64::from_str_radix(&digits, radix).ok()
}
