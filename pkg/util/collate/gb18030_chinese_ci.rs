// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// `gb18030_chinese_ci` Collator：按 GB18030 中文 CI（大小写不敏感）权重比较与生成 key。
//
// 权重表嵌入为 `gb18030_weight.data`，每个 Unicode code point 占 4 字节小端权重。

use std::any::Any;

use crate::collate::{
    Collator, WildcardPattern, compareCommon, compareCommonBytes, decodeRune, truncateTailingSpace,
    truncateTailingSpaceBytes,
};
use crate::stringutil;

/// 嵌入的 GB18030 chinese_ci 权重二进制表（按 code point × 4 字节索引）。
static gb18030WeightData: &[u8] = include_bytes!("gb18030_weight.data");
/// 权重表覆盖的最大 Unicode code point（含）。
pub const gb18030MaxCodePoint: u32 = 0x10ffff;

/// `gb18030_chinese_ci` 排序规则实现。
#[derive(Default)]
pub struct gb18030ChineseCICollator;

impl Collator for gb18030ChineseCICollator {
    fn Compare(&self, a: &str, b: &str) -> i32 {
        compareCommon(a, b, gb18030ChineseCISortKey)
    }
    fn CompareBytes(&self, a: &[u8], b: &[u8]) -> i32 {
        compareCommonBytes(a, b, gb18030ChineseCISortKey)
    }
    fn Key(&self, str_: &str) -> Vec<u8> {
        self.KeyWithoutTrimRightSpace(truncateTailingSpace(str_))
    }
    fn KeyBytes(&self, value: &[u8]) -> Vec<u8> {
        key_bytes_without_trim(truncateTailingSpaceBytes(value))
    }
    fn ImmutableKey(&self, str_: &str) -> Vec<u8> {
        self.Key(str_)
    }
    fn KeyWithoutTrimRightSpace(&self, str_: &str) -> Vec<u8> {
        let mut result = Vec::with_capacity(str_.len() * 2);
        for ch in str_.chars() {
            let weight = gb18030ChineseCISortKey(ch);
            // 大端风格可变长写出：高位字节仅在权重超出该字节宽度时追加。
            for shift in [24, 16, 8] {
                if weight > (1_u32 << shift) - 1 {
                    result.push((weight >> shift) as u8);
                }
            }
            result.push(weight as u8);
        }
        result
    }
    fn MaxKeyLen(&self, s: &str) -> i32 {
        (s.chars().count() * 4) as i32
    }
    fn Pattern(&self) -> Box<dyn WildcardPattern> {
        Box::new(gb18030ChineseCIPattern::default())
    }
    fn Clone(&self) -> Box<dyn Collator> {
        Box::new(Self)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn key_bytes_without_trim(value: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(value.len() * 2);
    let mut index = 0;
    while index < value.len() {
        let (rune, invalid) = decodeRune(value, &mut index);
        if invalid {
            return result;
        }
        let weight = gb18030ChineseCISortKey(rune);
        for shift in [24, 16, 8] {
            if weight > (1_u32 << shift) - 1 {
                result.push((weight >> shift) as u8);
            }
        }
        result.push(weight as u8);
    }
    result
}

/// LIKE pattern：用同一套 chinese_ci 权重做字符等价判断。
#[derive(Default)]
pub struct gb18030ChineseCIPattern {
    patChars: Vec<char>,
    patTypes: Vec<u8>,
}

impl WildcardPattern for gb18030ChineseCIPattern {
    fn Compile(&mut self, patternStr: &str, escape: u8) {
        (self.patChars, self.patTypes) = stringutil::CompilePatternInner(patternStr, escape);
    }
    fn DoMatch(&self, str_: &str) -> bool {
        stringutil::DoMatchCustomized(str_, &self.patChars, &self.patTypes, |a, b| {
            gb18030ChineseCISortKey(a) == gb18030ChineseCISortKey(b)
        })
    }
}

/// 查表得到 rune 的 GB18030 chinese_ci 排序权重；越界返回 0x3F。
pub fn gb18030ChineseCISortKey(r: char) -> u32 {
    let code = r as u32;
    if code > gb18030MaxCodePoint {
        return 0x3f;
    }
    let start = code as usize * 4;
    u32::from_le_bytes(
        gb18030WeightData[start..start + 4]
            .try_into()
            .expect("complete GB18030 weight"),
    )
}
