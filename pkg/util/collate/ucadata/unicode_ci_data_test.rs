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

// Unicode 4.0.0 生成表与原始对照表一致性测试。
//
// 对应 Go `unicode_ci_data_test.go`：逐项比对 `DUCET0400Table` 与原始
// `MAP_TABLE`/`LONG_RUNE_MAP`，并检查 4.0.0 / 9.0.0 长权重项两两不重复。

// 这些测试比较新生成的 Unicode 4.0.0 表与原始表，并检查 LongRuneMap 项唯一性。

use super::unicode_0900_ai_ci_data_generated::DUCET0900Table;
use super::unicode_ci_data_generated::DUCET0400Table;
use super::unicode_ci_data_original_test::{LONG_RUNE_MAP, MAP_TABLE};

// test_unicode_0400_is_the_same 对应 Go 的 TestUnicode0400IsTheSame。
// 它把生成表逐项对照原始 mapTable/longRuneMap，防止生成器悄悄改变历史数据。
#[test]
/// 对应 Go `TestUnicode0400IsTheSame`：生成表与原始表数值完全一致。
pub fn test_unicode_0400_is_the_same() {
    assert_eq!(DUCET0400Table.MapTable4.len(), MAP_TABLE.len());

    for (idx, item) in DUCET0400Table.MapTable4.iter().enumerate() {
        // Go 的 require.Equal 附带 rune 和十六进制上下文；保留同等诊断信息。
        assert_eq!(
            *item,
            MAP_TABLE[idx],
            "0x{:X} {}: 0x{:X} in new, 0x{:X} in old",
            idx,
            char::from_u32(idx as u32).unwrap_or('\u{FFFD}'),
            *item,
            MAP_TABLE[idx]
        );
    }

    for (k, v) in DUCET0400Table.LongRuneMap.iter() {
        let old = long_rune_weights(*k);
        assert_eq!(
            v[0],
            old[0],
            "{}[0]: 0x{:X} in new, 0x{:X} in old",
            display_rune(*k),
            v[0],
            old[0]
        );
        assert_eq!(
            v[1],
            old[1],
            "{}[1]: 0x{:X} in new, 0x{:X} in old",
            display_rune(*k),
            v[1],
            old[1]
        );
    }
}

// test_all_item_in_long_rune_map_is_unique 对应 Go 的 TestAllItemInLongRUneMapIsUnique。
// 两层循环保留 Go 的 O(n^2) 检查，明确比较不同 rune 的长权重切片是否重复。
#[test]
/// 对应 Go `TestAllItemInLongRUneMapIsUnique`：不同 rune 的长权重不得相等。
pub fn test_all_item_in_long_rune_map_is_unique() {
    for (k1, v1) in DUCET0400Table.LongRuneMap.iter() {
        for (k2, v2) in DUCET0400Table.LongRuneMap.iter() {
            if k1 == k2 {
                continue;
            }
            assert_ne!(
                v1,
                v2,
                "{}((0x{:X}) and {}(0x{:X}) are equal",
                display_rune(*k1),
                k1,
                display_rune(*k2),
                k2
            );
        }
    }

    for (k1, v1) in DUCET0900Table.long_rune_map.iter() {
        for (k2, v2) in DUCET0900Table.long_rune_map.iter() {
            if k1 == k2 {
                continue;
            }
            assert_ne!(
                v1,
                v2,
                "{}((0x{:X}) and {}(0x{:X}) are equal",
                display_rune(*k1),
                k1,
                display_rune(*k2),
                k2
            );
        }
    }
}

// long_rune_weights 对应 Go 中 longRuneMap[k] 的索引读取。
// 原始表在 实现中保存为元组切片，因此用线性查找保留“缺失即测试失败”的语义。
/// 在原始 `LONG_RUNE_MAP` 中按 rune 线性查找；缺失则测试失败。
fn long_rune_weights(rune: u32) -> [u64; 2] {
    for (key, weights) in LONG_RUNE_MAP.iter() {
        if *key == rune {
            return *weights;
        }
    }
    panic!("missing original long rune map entry for 0x{:X}", rune);
}

// display_rune 保留 Go 格式串中 %c 的意图；非法码点用 U+FFFD 作为诊断替代字符。
/// 将码点格式化为诊断字符；非法码点用 U+FFFD 替代。
fn display_rune(rune: u32) -> char {
    char::from_u32(rune).unwrap_or('\u{FFFD}')
}
