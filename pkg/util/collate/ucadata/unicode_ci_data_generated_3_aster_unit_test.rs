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

// `DUCET0400Table` 生成表边界与长权重语义的迁移期单元测试。
//
// 校验主表长度、已知码点权重、长权重二分查找，以及 `LongRuneMap` 键有序且权重唯一。

use super::unicode_ci_data_generated::DUCET0400Table;

#[test]
/// 校验主表边界与若干代表码点权重（含大小写不敏感的 A/a）。
fn map_table_matches_unicode_0400_boundaries_and_known_weights() {
    assert_eq!(DUCET0400Table.MapTable4.len(), 65_536);
    assert_eq!(DUCET0400Table.map_table_weight(0x0000), Some(0));
    assert_eq!(DUCET0400Table.map_table_weight(0x0009), Some(0x201));
    assert_eq!(DUCET0400Table.map_table_weight('A' as u32), Some(0xE33));
    assert_eq!(DUCET0400Table.map_table_weight('a' as u32), Some(0xE33));
    assert_eq!(DUCET0400Table.map_table_weight(0xFFFF), Some(0xFFFF_FBC1));
    assert_eq!(DUCET0400Table.map_table_weight(0x1_0000), None);
}

#[test]
/// 校验长权重查找与 Go map 语义一致；普通字母不应命中长权重表。
fn long_rune_lookup_matches_generated_go_map_semantics() {
    assert_eq!(
        DUCET0400Table.long_rune_weight(0x321D),
        Some([0x1D6E_1DC6_1D6D_0288, 0x289_1E03_1DC2])
    );
    assert_eq!(
        DUCET0400Table.long_rune_weight(0xFDFB),
        Some([0x135E_0209_13AB_135E, 0x13B7_13AB_1350_13AB])
    );
    assert_eq!(DUCET0400Table.long_rune_weight('A' as u32), None);
}

#[test]
/// 校验长权重键严格递增，且相邻/全局权重不重复。
fn long_rune_entries_have_unique_keys_and_weights() {
    // 相邻窗口：键有序唯一，权重亦不相等。
    for pair in DUCET0400Table.LongRuneMap.windows(2) {
        assert!(
            pair[0].0 < pair[1].0,
            "long-rune keys must be sorted and unique"
        );
        assert_ne!(
            pair[0].1, pair[1].1,
            "adjacent long-rune weights must differ"
        );
    }

    // 全局唯一：每个权重向量在后续条目中不得再次出现。
    for (index, (_, weights)) in DUCET0400Table.LongRuneMap.iter().enumerate() {
        assert!(
            DUCET0400Table.LongRuneMap[index + 1..]
                .iter()
                .all(|(_, other)| other != weights),
            "long-rune weights must be globally unique"
        );
    }
}
