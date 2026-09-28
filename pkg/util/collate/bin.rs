// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 二进制排序规则（collation）：按字节原样比较/生成键，及尾空格填充变体。
//
// 对应 Go `pkg/util/collate/bin.go`。`binCollator` 严格按字节比较；
// `binPaddingCollator` 比较与 Key 时截去尾空格（PAD SPACE 语义）；
// Pattern 分字节级（binary）与 rune 级（derived）通配匹配。

use std::any::Any;

use crate::collate::{Collator, WildcardPattern, truncateTailingSpace, truncateTailingSpaceBytes};
use crate::stringutil;

/// 字符串字典序比较，映射为 Go 风格 -1/0/1。
fn compare(a: &str, b: &str) -> i32 {
    compare_bytes(a.as_bytes(), b.as_bytes())
}

fn compare_bytes(a: &[u8], b: &[u8]) -> i32 {
    match a.cmp(b) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// 严格二进制排序器：比较与 Key 均保留尾空格。
#[derive(Default)]
pub struct binCollator;

impl Collator for binCollator {
    fn Compare(&self, a: &str, b: &str) -> i32 {
        compare(a, b)
    }
    fn CompareBytes(&self, a: &[u8], b: &[u8]) -> i32 {
        compare_bytes(a, b)
    }
    fn Key(&self, str_: &str) -> Vec<u8> {
        str_.as_bytes().to_vec()
    }
    fn KeyBytes(&self, value: &[u8]) -> Vec<u8> {
        value.to_vec()
    }
    fn ImmutableKey(&self, str_: &str) -> Vec<u8> {
        str_.as_bytes().to_vec()
    }
    fn KeyWithoutTrimRightSpace(&self, str_: &str) -> Vec<u8> {
        str_.as_bytes().to_vec()
    }
    fn MaxKeyLen(&self, s: &str) -> i32 {
        s.len() as i32
    }
    fn Pattern(&self) -> Box<dyn WildcardPattern> {
        Box::new(binPattern::default())
    }
    fn Clone(&self) -> Box<dyn Collator> {
        Box::new(Self)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// derived 二进制排序器：比较同 bin，通配按 rune 编译。
#[derive(Default)]
pub struct derivedBinCollator;

impl Collator for derivedBinCollator {
    fn Compare(&self, a: &str, b: &str) -> i32 {
        compare(a, b)
    }
    fn CompareBytes(&self, a: &[u8], b: &[u8]) -> i32 {
        compare_bytes(a, b)
    }
    fn Key(&self, str_: &str) -> Vec<u8> {
        str_.as_bytes().to_vec()
    }
    fn KeyBytes(&self, value: &[u8]) -> Vec<u8> {
        value.to_vec()
    }
    fn ImmutableKey(&self, str_: &str) -> Vec<u8> {
        str_.as_bytes().to_vec()
    }
    fn KeyWithoutTrimRightSpace(&self, str_: &str) -> Vec<u8> {
        str_.as_bytes().to_vec()
    }
    fn MaxKeyLen(&self, s: &str) -> i32 {
        s.len() as i32
    }
    fn Pattern(&self) -> Box<dyn WildcardPattern> {
        Box::new(derivedBinPattern::default())
    }
    fn Clone(&self) -> Box<dyn Collator> {
        Box::new(Self)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// PAD SPACE 二进制排序器：Compare/Key 截去尾空格，KeyWithoutTrimRightSpace 保留。
#[derive(Default)]
pub struct binPaddingCollator;

impl Collator for binPaddingCollator {
    fn Compare(&self, a: &str, b: &str) -> i32 {
        compare(truncateTailingSpace(a), truncateTailingSpace(b))
    }
    fn CompareBytes(&self, a: &[u8], b: &[u8]) -> i32 {
        compare_bytes(truncateTailingSpaceBytes(a), truncateTailingSpaceBytes(b))
    }
    fn Key(&self, str_: &str) -> Vec<u8> {
        truncateTailingSpace(str_).as_bytes().to_vec()
    }
    fn KeyBytes(&self, value: &[u8]) -> Vec<u8> {
        truncateTailingSpaceBytes(value).to_vec()
    }
    fn ImmutableKey(&self, str_: &str) -> Vec<u8> {
        self.Key(str_)
    }
    fn KeyWithoutTrimRightSpace(&self, str_: &str) -> Vec<u8> {
        str_.as_bytes().to_vec()
    }
    fn MaxKeyLen(&self, s: &str) -> i32 {
        s.len() as i32
    }
    fn Pattern(&self) -> Box<dyn WildcardPattern> {
        Box::new(derivedBinPattern::default())
    }
    fn Clone(&self) -> Box<dyn Collator> {
        Box::new(Self)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// rune 级通配模式（derived bin）：用 CompilePattern/DoMatch。
#[derive(Default)]
pub struct derivedBinPattern {
    patChars: Vec<char>,
    patTypes: Vec<u8>,
}

impl WildcardPattern for derivedBinPattern {
    fn Compile(&mut self, patternStr: &str, escape: u8) {
        (self.patChars, self.patTypes) = stringutil::CompilePattern(patternStr, escape);
    }
    fn DoMatch(&self, str_: &str) -> bool {
        stringutil::DoMatch(str_, &self.patChars, &self.patTypes)
    }
}

/// 字节级通配模式（binary）：用 CompilePatternBinary/DoMatchBinary。
#[derive(Default)]
pub struct binPattern {
    patChars: Vec<u8>,
    patTypes: Vec<u8>,
}

impl WildcardPattern for binPattern {
    fn Compile(&mut self, patternStr: &str, escape: u8) {
        (self.patChars, self.patTypes) = stringutil::CompilePatternBinary(patternStr, escape);
    }
    fn DoMatch(&self, str_: &str) -> bool {
        stringutil::DoMatchBinary(str_, &self.patChars, &self.patTypes)
    }
}
