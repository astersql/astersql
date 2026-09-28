// Copyright 2020 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// Unicode 4.0.0 ci Collator 的局部实现与通配符匹配。
//
// 提供权重查询（含 longRune 多段权重）和 `WildcardPattern`；生成版
// Collator 委托本文件做预处理与 `GetWeight`。对应 Go `unicode_0400_ci.go`。

// 这个文件保存 Unicode 4.0.0 ci collator 的局部实现和 wildcard pattern 逻辑。

#![allow(dead_code)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

// magic number indicate weight has 2 uint64, should get from `longRuneMap`
/// MapTable4 哨兵值：表示该 rune 权重需从 LongRuneMap 取两段 uint64。
// longRune 对应 Go 中的 uint64 常量；它是 MapTable4 里的哨兵值，表示需要读取 LongRuneMap 的两段权重。
pub const longRune: u64 = 0xFFFD;

use crate::{WildcardPattern, stringutil, truncateTailingSpace, ucadata};

// Go generate directive from the source file:
// - go run ./ucaimpl/main.go -- unicode_0400_ci_generated.go
// Rust 这里只记录生成关系，不在本文件里触发代码生成。

/// Unicode 4.0.0 ci 权重实现：预处理、查表与通配符构造。
// unicode0400Impl 对应 Go 的空结构体，承载 Unicode 4.0.0 ci 的预处理、权重和 pattern 构造方法。
#[derive(Clone, Default)]
pub struct unicode0400Impl {}

impl unicode0400Impl {
    // Clone 对应 Go 值接收者 Clone：返回一个新的空实现值。
    pub fn Clone(&self) -> unicode0400Impl {
        unicode0400Impl {}
    }

    // Preprocess 对应 Go 的字符串预处理：比较前截断尾部空格。
    pub fn Preprocess(&self, s: &str) -> String {
        truncateTailingSpace(s).to_owned()
    }

    // GetWeight 对应 Go 的权重查询，返回 first/second 两段 uint64 权重。
    pub fn GetWeight(&self, r: char) -> (u64, u64) {
        let idx = r as usize;
        if idx > 0xFFFF {
            // Go 对超出 BMP 的 rune 统一返回 0xFFFD 权重；这里保留同样的兜底。
            return (0xFFFD, 0);
        }

        if ucadata::DUCET0400Table.MapTable4[idx] == longRune {
            // longRune 哨兵表示权重大于一段 uint64，需要继续查 LongRuneMap。
            let weight = ucadata::DUCET0400Table
                .long_rune_weight(r as u32)
                .expect("longRune sentinel must have generated weights");
            return (weight[0], weight[1]);
        }

        (ucadata::DUCET0400Table.MapTable4[idx], 0)
    }

    // Pattern 对应 Go 的 WildcardPattern 构造，返回 Unicode 4.0.0 ci 专用匹配器。
    pub fn Pattern(&self) -> Box<dyn WildcardPattern> {
        Box::new(unicodePattern::default())
    }
}
/// 已编译的 Unicode 4.0.0 ci 通配符 pattern（字符与类型数组）。
// unicodePattern 保存已编译的通配符 pattern 字符和类型；字段顺序对应 Go 结构体。
#[derive(Default)]
pub struct unicodePattern {
    patChars: Vec<char>,
    patTypes: Vec<u8>,
}

impl WildcardPattern for unicodePattern {
    // Compile implements WildcardPattern interface.
    // Compile 对应 Go 的 stringutil.CompilePatternInner，按 escape 字节拆出 pattern 字符和类型。
    fn Compile(&mut self, patternStr: &str, escape: u8) {
        let (patChars, patTypes) = stringutil::CompilePatternInner(patternStr, escape);
        self.patChars = patChars;
        self.patTypes = patTypes;
    }

    // DoMatch implements WildcardPattern interface.
    // DoMatch 对应 Go 的 DoMatchCustomized；比较闭包使用 UCA 4.0.0 权重判断大小写/重音不敏感相等。
    fn DoMatch(&self, str_: &str) -> bool {
        stringutil::DoMatchCustomized(str_, &self.patChars, &self.patTypes, |a: char, b: char| {
            if (a as u32) > 0xFFFF || (b as u32) > 0xFFFF {
                // Go 对非 BMP rune 不查 4.0.0 表，直接按 rune 值相等判断。
                return a == b;
            }

            let ar = ucadata::DUCET0400Table.MapTable4[a as usize];
            let br = ucadata::DUCET0400Table.MapTable4[b as usize];
            if ar != br {
                return false;
            }

            if ar == longRune {
                // Go 遇到 longRune 时不比较 LongRuneMap 内容，要求原 rune 完全相同。
                return a == b;
            }

            true
        })
    }
}
