// Copyright 2023 PingCAP, Inc.
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

// Unicode 9.0.0 ai_ci Collator 的权重转换与通配符匹配。
//
// `convertRuneUnicodeCI0900` 将 rune 映射为最多两段 UCA 权重；通配符
// 比较要求两段权重均相等。对应 Go `unicode_0900_ai_ci.go`。

// 这个文件保存 Unicode 9.0.0 ai_ci collator 的权重转换和 wildcard pattern 逻辑。

#![allow(dead_code)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use crate::{WildcardPattern, stringutil, ucadata};

// Go generate directive from the source file:
// - go run ./ucaimpl/main.go -- unicode_0900_ai_ci_generated.go
// Rust 这里只记录生成关系，不在本文件里触发代码生成。

/// Unicode 9.0.0 ai_ci 权重实现：预处理（no-op）、查表与 pattern 构造。
// unicode0900Impl 对应 Go 的空结构体，承载 Unicode 9.0.0 ai_ci 的预处理、权重和 pattern 构造方法。
#[derive(Clone, Default)]
pub struct unicode0900Impl {}

impl unicode0900Impl {
    // Clone 对应 Go 值接收者 Clone：返回一个新的空实现值。
    pub fn Clone(&self) -> unicode0900Impl {
        unicode0900Impl {}
    }

    // Preprocess 对应 Go 的 no-op 预处理；0900 ai_ci 不截断字符串。
    pub fn Preprocess(&self, s: &str) -> String {
        s.to_string()
    }

    // GetWeight 对应 Go 的权重查询入口，实际逻辑委托给 convertRuneUnicodeCI0900。
    pub fn GetWeight(&self, r: char) -> (u64, u64) {
        convertRuneUnicodeCI0900(r)
    }

    // Pattern 对应 Go 的 WildcardPattern 构造，返回 Unicode 9.0.0 ai_ci 专用匹配器。
    pub fn Pattern(&self) -> Box<dyn WildcardPattern> {
        Box::new(unicode0900AICIPattern::default())
    }
}

/// 将 rune 转为最多两段 UCA 权重；表外 rune 使用 FBC0 区间兜底。
// convertRuneUnicodeCI0900 对应 Go 的同名函数：把 rune 转成最多两段 UCA 权重。
pub fn convertRuneUnicodeCI0900(r: char) -> (u64, u64) {
    let raw = r as u32;
    if raw as usize > ucadata::DUCET0900Table.map_table4.len() {
        // Go 对表外 rune 构造 FBC0 区间的兜底权重，保留位运算形状。
        let high = (raw >> 15) as u64;
        let low = (((raw & 0x7FFF) | 0x8000) as u64) << 16;
        return (high + 0xFBC0 + low, 0);
    }

    let first = ucadata::DUCET0900Table.map_table4[raw as usize];
    if first == ucadata::LongRune8 {
        // LongRune8 表示该 rune 需要到 LongRuneMap 读取第二段权重。
        let weight = ucadata::DUCET0900Table
            .long_rune_map
            .binary_search_by_key(&raw, |(key, _)| *key)
            .ok()
            .map(|index| ucadata::DUCET0900Table.long_rune_map[index].1)
            .expect("LongRune8 sentinel must have generated weights");
        return (weight[0], weight[1]);
    }
    (first, 0)
}

/// 已编译的 Unicode 9.0.0 ai_ci 通配符 pattern。
// unicode0900AICIPattern 保存已编译的通配符 pattern 字符和类型；字段顺序对应 Go 结构体。
#[derive(Default)]
pub struct unicode0900AICIPattern {
    patChars: Vec<char>,
    patTypes: Vec<u8>,
}

impl WildcardPattern for unicode0900AICIPattern {
    // Compile implements WildcardPattern interface.
    // Compile 对应 Go 的 stringutil.CompilePatternInner，按 escape 字节拆出 pattern 字符和类型。
    fn Compile(&mut self, patternStr: &str, escape: u8) {
        let (patChars, patTypes) = stringutil::CompilePatternInner(patternStr, escape);
        self.patChars = patChars;
        self.patTypes = patTypes;
    }

    // DoMatch implements WildcardPattern interface.
    // DoMatch 对应 Go 的 DoMatchCustomized；比较闭包要求两段权重都相同。
    fn DoMatch(&self, str_: &str) -> bool {
        stringutil::DoMatchCustomized(str_, &self.patChars, &self.patTypes, |a: char, b: char| {
            let (aFirst, aSecond) = convertRuneUnicodeCI0900(a);
            let (bFirst, bSecond) = convertRuneUnicodeCI0900(b);

            aFirst == bFirst && aSecond == bSecond
        })
    }
}
