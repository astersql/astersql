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

// `gbk_bin` Collator：按 GBK 字节编码做 binary 比较与排序 key。
//
// GBK 是简体中文常用双字节编码；bin 变体不折叠大小写，PAD SPACE 时先裁尾空格。

use std::any::Any;

use crate::bin::derivedBinPattern;
use crate::collate::{
    Collator, WildcardPattern, decodeRune, sign, truncateTailingSpace, truncateTailingSpaceBytes,
};

/// `gbk_bin` 排序规则实现。
#[derive(Default)]
pub struct gbkBinCollator;

/// 将单个 Unicode 字符编码为 GBK 字节；无法编码时用 `?`（0x3F）占位。
fn encode_char(ch: char) -> Vec<u8> {
    let text = ch.to_string();
    let mut encoder = parser_charset::NewCustomGBKEncoder();
    let mut destination = Vec::with_capacity(2);
    encoder
        .transform(&mut destination, text.as_bytes())
        .unwrap_or_else(|_| vec![b'?'])
}

impl Collator for gbkBinCollator {
    fn Compare(&self, a: &str, b: &str) -> i32 {
        // 逐 rune 编码为 GBK 后比较字节序列。
        let mut left = truncateTailingSpace(a).chars();
        let mut right = truncateTailingSpace(b).chars();
        loop {
            match (left.next(), right.next()) {
                (Some(x), Some(y)) => match encode_char(x).cmp(&encode_char(y)) {
                    std::cmp::Ordering::Less => return -1,
                    std::cmp::Ordering::Greater => return 1,
                    std::cmp::Ordering::Equal => {}
                },
                (Some(_), None) => return 1,
                (None, Some(_)) => return -1,
                (None, None) => return 0,
            }
        }
    }
    fn CompareBytes(&self, a: &[u8], b: &[u8]) -> i32 {
        compare_encoded_bytes(truncateTailingSpaceBytes(a), truncateTailingSpaceBytes(b))
    }
    fn Key(&self, str_: &str) -> Vec<u8> {
        self.KeyWithoutTrimRightSpace(truncateTailingSpace(str_))
    }
    fn KeyBytes(&self, value: &[u8]) -> Vec<u8> {
        encode_bytes(truncateTailingSpaceBytes(value))
    }
    fn ImmutableKey(&self, str_: &str) -> Vec<u8> {
        self.Key(str_)
    }
    fn KeyWithoutTrimRightSpace(&self, str_: &str) -> Vec<u8> {
        str_.chars().flat_map(encode_char).collect()
    }
    fn MaxKeyLen(&self, s: &str) -> i32 {
        // GBK 单字符最多 2 字节。
        (s.chars().count() * 2) as i32
    }
    fn Pattern(&self) -> Box<dyn WildcardPattern> {
        Box::new(gbkBinPattern::default())
    }
    fn Clone(&self) -> Box<dyn Collator> {
        Box::new(Self)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn encode_bytes(value: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(value.len());
    let mut index = 0;
    while index < value.len() {
        let (rune, invalid) = decodeRune(value, &mut index);
        if invalid {
            result.push(b'?');
        } else {
            result.extend(encode_char(rune));
        }
    }
    result
}

fn compare_encoded_bytes(a: &[u8], b: &[u8]) -> i32 {
    match encode_bytes(a).cmp(&encode_bytes(b)) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// 委托给 `derivedBinPattern` 的通配符匹配器。
#[derive(Default)]
pub struct gbkBinPattern {
    inner: derivedBinPattern,
}

impl WildcardPattern for gbkBinPattern {
    fn Compile(&mut self, patternStr: &str, escape: u8) {
        self.inner.Compile(patternStr, escape);
    }
    fn DoMatch(&self, str_: &str) -> bool {
        self.inner.DoMatch(str_)
    }
}

/// 比较两长度，返回 -1/0/1（保留自 Go，当前未使用）。
#[allow(dead_code)]
fn compare_length(a: usize, b: usize) -> i32 {
    sign(a as isize - b as isize)
}
