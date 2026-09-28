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

// `gbk_chinese_ci` Collator：按 GBK 中文 CI（大小写不敏感）权重比较与生成 key。
//
// 权重来自 `gbk_chinese_ci_data` 静态表；超出 BMP 的 code point 使用默认权重 0x3F。

use std::any::Any;

use crate::collate::{
    Collator, WildcardPattern, compareCommon, compareCommonBytes, decodeRune, truncateTailingSpace,
    truncateTailingSpaceBytes,
};
use crate::gbk_chinese_ci_data::gbkChineseCISortKeyTable;
use crate::stringutil;

/// `gbk_chinese_ci` 排序规则实现。
#[derive(Default)]
pub struct gbkChineseCICollator;

impl Collator for gbkChineseCICollator {
    fn Compare(&self, a: &str, b: &str) -> i32 {
        compareCommon(a, b, gbkChineseCISortKey)
    }
    fn CompareBytes(&self, a: &[u8], b: &[u8]) -> i32 {
        compareCommonBytes(a, b, gbkChineseCISortKey)
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
            let weight = gbkChineseCISortKey(ch);
            // 权重大于一字节时先写高字节，再写低字节。
            if weight > 0xff {
                result.push((weight >> 8) as u8);
            }
            result.push(weight as u8);
        }
        result
    }
    fn MaxKeyLen(&self, s: &str) -> i32 {
        (s.chars().count() * 2) as i32
    }
    fn Pattern(&self) -> Box<dyn WildcardPattern> {
        Box::new(gbkChineseCIPattern::default())
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
        let weight = gbkChineseCISortKey(rune);
        if weight > 0xff {
            result.push((weight >> 8) as u8);
        }
        result.push(weight as u8);
    }
    result
}

/// LIKE pattern：用同一套 chinese_ci 权重做字符等价判断。
#[derive(Default)]
pub struct gbkChineseCIPattern {
    patChars: Vec<char>,
    patTypes: Vec<u8>,
}

impl WildcardPattern for gbkChineseCIPattern {
    fn Compile(&mut self, patternStr: &str, escape: u8) {
        (self.patChars, self.patTypes) = stringutil::CompilePatternInner(patternStr, escape);
    }
    fn DoMatch(&self, str_: &str) -> bool {
        stringutil::DoMatchCustomized(str_, &self.patChars, &self.patTypes, |a, b| {
            gbkChineseCISortKey(a) == gbkChineseCISortKey(b)
        })
    }
}

/// 查表得到 rune 的 GBK chinese_ci 排序权重；非 BMP 返回 0x3F。
pub fn gbkChineseCISortKey(r: char) -> u32 {
    let code = r as usize;
    if code > 0xffff {
        0x3f
    } else {
        gbkChineseCISortKeyTable[code] as u32
    }
}
