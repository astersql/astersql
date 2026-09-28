// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// KeyRanges 辅助方法：索引、切片、按键拆分与展示。
//
// KeyRange 表示半开键区间 `[start, end)`；KeyRanges 是一组有序区间。
// Split 按给定键切开，用于按 Region（键空间分片）边界拆分 coprocessor 任务。

use std::fmt::{self, Display, Formatter};

pub use crate::batch_request_sender::{KeyRange, KeyRanges};

impl KeyRanges {
    /// 按索引借用第 `index` 个区间；越界返回 `None`。
    pub fn ref_at(&self, index: usize) -> Option<&KeyRange> {
        self.0.get(index)
    }

    /// 按索引克隆第 `index` 个区间；越界返回 `None`。
    pub fn at(&self, index: usize) -> Option<KeyRange> {
        self.ref_at(index).cloned()
    }

    /// 取半开下标区间 `[from, to)` 的子 KeyRanges。
    pub fn slice(&self, from: usize, to: usize) -> Self {
        assert!(from <= to && to <= self.len(), "invalid KeyRanges slice");
        Self(self.0[from..to].to_vec())
    }

    /// 依次访问每个 KeyRange。
    pub fn for_each(&self, mut visit: impl FnMut(&KeyRange)) {
        for range in &self.0 {
            visit(range);
        }
    }

    /// Splits at the first range whose end is greater than `key`. If that range
    /// contains the key, both halves receive the corresponding truncated range.
    ///
    /// 按键拆分：找到第一个 `end > key` 的区间；若键落在该区间内，左右两侧各保留截断后的半段。
    pub fn split(&self, key: &[u8]) -> (Self, Self) {
        // 跳过所有 end 非空且 end <= key 的区间，定位可能包含 key 的位置。
        let index = self
            .0
            .partition_point(|range| !range.end.is_empty() && range.end.as_slice() <= key);
        // 键落在当前区间内部时，左右各追加一段截断后的 KeyRange。
        if let Some(range) = self.0.get(index)
            && key > range.start.as_slice()
        {
            let mut left = self.0[..index].to_vec();
            left.push(KeyRange {
                start: range.start.clone(),
                end: key.to_vec(),
            });
            let mut right = Vec::with_capacity(self.len() - index);
            right.push(KeyRange {
                start: key.to_vec(),
                end: range.end.clone(),
            });
            right.extend_from_slice(&self.0[index + 1..]);
            return (Self(left), Self(right));
        }
        (self.slice(0, index), self.slice(index, self.len()))
    }

    /// 克隆为普通 `Vec<KeyRange>`。
    pub fn to_ranges(&self) -> Vec<KeyRange> {
        self.0.clone()
    }

    /// 用新的区间列表整体替换内部存储。
    pub fn reset(&mut self, ranges: Vec<KeyRange>) {
        self.0 = ranges;
    }

    /// 转为 protobuf 风格区间列表（当前与 `to_ranges` 相同）。
    pub fn to_pb_ranges(&self) -> Vec<KeyRange> {
        self.0.clone()
    }
}

impl Display for KeyRanges {
    /// 将各区间格式化为 `[start, end]` 连续输出。
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        for range in &self.0 {
            formatter.write_str("[")?;
            write_go_quoted_bytes(formatter, &range.start)?;
            formatter.write_str(", ")?;
            write_go_quoted_bytes(formatter, &range.end)?;
            formatter.write_str("]")?;
        }
        Ok(())
    }
}

/// Matches Go's `%q` formatting for a byte slice: valid UTF-8 is quoted as text,
/// while control characters and invalid UTF-8 bytes use Go string escapes.
fn write_go_quoted_bytes(formatter: &mut Formatter<'_>, bytes: &[u8]) -> fmt::Result {
    formatter.write_str("\"")?;
    let mut remaining = bytes;
    while !remaining.is_empty() {
        match std::str::from_utf8(remaining) {
            Ok(text) => {
                write_go_quoted_text(formatter, text)?;
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                let valid_text = std::str::from_utf8(&remaining[..valid])
                    .expect("Utf8Error::valid_up_to always delimits valid UTF-8");
                write_go_quoted_text(formatter, valid_text)?;
                let invalid = error.error_len().unwrap_or(remaining.len() - valid);
                for byte in &remaining[valid..valid + invalid] {
                    write!(formatter, "\\x{byte:02x}")?;
                }
                remaining = &remaining[valid + invalid..];
            }
        }
    }
    formatter.write_str("\"")
}

fn write_go_quoted_text(formatter: &mut Formatter<'_>, text: &str) -> fmt::Result {
    for character in text.chars() {
        match character {
            '\u{7}' => formatter.write_str("\\a")?,
            '\u{8}' => formatter.write_str("\\b")?,
            '\u{c}' => formatter.write_str("\\f")?,
            '\n' => formatter.write_str("\\n")?,
            '\r' => formatter.write_str("\\r")?,
            '\t' => formatter.write_str("\\t")?,
            '\u{b}' => formatter.write_str("\\v")?,
            '\\' => formatter.write_str("\\\\")?,
            '"' => formatter.write_str("\\\"")?,
            '\0'..='\u{6}' | '\u{e}'..='\u{1f}' | '\u{7f}' => {
                write!(formatter, "\\x{:02x}", character as u32)?;
            }
            '\u{2028}' | '\u{2029}' => write!(formatter, "\\u{:04x}", character as u32)?,
            _ => write!(formatter, "{character}")?,
        }
    }
    Ok(())
}

/// Go 风格构造函数：由 `Vec<KeyRange>` 创建 `KeyRanges`。
#[allow(non_snake_case)]
pub fn NewKeyRanges(ranges: Vec<KeyRange>) -> KeyRanges {
    KeyRanges::new(ranges)
}
