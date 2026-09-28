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

// `DUCET0900Table` 生成表形状与代表值的迁移期单元测试。
//
// 对应 Go 侧对 Unicode 9.0.0 表的边界检查：Hangul Jamo 单权重、长权重首段非零，
// 以及表长与若干已知码点权重与 Go 生成结果一致。

use super::DUCET0900Table;

#[test]
/// 遍历 Hangul Jamo 区间，确认高 48 位为 0（仅保留一个 uint16 权重）。
fn hangul_jamo_has_only_one_weight() {
    // 掩码保留高 48 位：非零则说明该 Jamo 被错误压成多权重。
    for rune in 0x1100..0x11ff {
        assert_eq!(DUCET0900Table.map_table4[rune] & 0xffff_ffff_ffff_0000, 0);
    }
}

#[test]
/// 长权重映射中每项首段权重不得为 0，避免调用方误判为空。
fn long_rune_first_word_is_not_zero() {
    assert!(
        DUCET0900Table
            .long_rune_map
            .iter()
            .all(|(_, weights)| weights[0] != 0)
    );
}

#[test]
/// 校验表长、代表码点权重及长权重样例与 Go 生成语义一致。
fn generated_table_shape_and_representative_values_match_go() {
    // 代表值抽样：表长、TAB/A/替换字符及两处 long_rune_map 样例。
    assert_eq!(DUCET0900Table.map_table4.len(), 183_969);
    assert_eq!(DUCET0900Table.map_table4[0x09], 0x201);
    assert_eq!(DUCET0900Table.map_table4[0x41], 0x1c47);
    assert_eq!(DUCET0900Table.map_table4[0xfffd], 0xfffd);

    assert_eq!(DUCET0900Table.long_rune_map.len(), 27);
    assert_eq!(
        DUCET0900Table
            .long_rune_map
            .iter()
            .find(|(rune, _)| *rune == 0x321d)
            .map(|(_, weights)| *weights),
        Some([0x3c01_3c7b_3c00_0317, 0x318_3cd4_3c77])
    );
    assert_eq!(
        DUCET0900Table
            .long_rune_map
            .iter()
            .find(|(rune, _)| *rune == 0xfffd)
            .map(|(_, weights)| *weights),
        Some([0xfffd, 0])
    );
}
