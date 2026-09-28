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

// `gb18030_bin` Collator：按 GB18030 字节编码做 binary 比较与排序 key。
//
// GB18030 是中国国家标准多字节字符集；bin 变体不折叠大小写，PAD SPACE 时先裁尾空格。

use std::any::Any;

use crate::bin::binPattern;
use crate::collate::{
    Collator, WildcardPattern, decodeRune, truncateTailingSpace, truncateTailingSpaceBytes,
};

/// `gb18030_bin` 排序规则实现（空结构体，行为由 Collator impl 提供）。
#[derive(Default)]
pub struct gb18030BinCollator;

/// 将单个 Unicode 字符编码为 GB18030 字节；无法编码时用 `?`（0x3F）占位。
fn encode_char(ch: char) -> Vec<u8> {
    // Go's NewCustomGB18030Encoder applies the repository's GB18030-2022
    // overrides before falling back to the standard encoder.
    if let Some(encoded) = parser_charset::unicode_to_gb18030().get(&ch) {
        return parser_charset::convert_u32_to_bytes(*encoded);
    }
    let text = ch.to_string();
    let (encoded, _, had_errors) = encoding_rs::GB18030.encode(&text);
    if had_errors {
        vec![b'?']
    } else {
        encoded.into_owned()
    }
}

impl Collator for gb18030BinCollator {
    fn Compare(&self, a: &str, b: &str) -> i32 {
        // 逐 rune 编码为 GB18030 后比较字节序列；一侧先结束则较短者更小。
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
        // GB18030 单字符最多 4 字节。
        (s.chars().count() * 4) as i32
    }
    fn Pattern(&self) -> Box<dyn WildcardPattern> {
        Box::new(gb18030BinPattern::default())
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

/// 委托给 `binPattern` 的通配符匹配器（字节级 LIKE）。
#[derive(Default)]
pub struct gb18030BinPattern {
    inner: binPattern,
}

impl WildcardPattern for gb18030BinPattern {
    fn Compile(&mut self, patternStr: &str, escape: u8) {
        self.inner.Compile(patternStr, escape);
    }
    fn DoMatch(&self, str_: &str) -> bool {
        self.inner.DoMatch(str_)
    }
}
