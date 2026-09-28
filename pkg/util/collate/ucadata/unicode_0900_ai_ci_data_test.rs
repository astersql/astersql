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

// Unicode 9.0.0 AI/CI 生成表的包内形状测试。
//
// 对应 Go `unicode_0900_ai_ci_data_test.go`：只校验 `DUCET0900Table` 的权重布局，
// 不访问外部 IO；覆盖 Hangul Jamo 单权重与长权重首段非零约束。

// 这些测试只校验生成的 UCA 9.0.0 数据表形状，不做 IO 或外部依赖调用。

use super::unicode_0900_ai_ci_data_generated::DUCET0900Table;

// test_hangul_jamo_has_only_one_weight 对应 Go 的 TestHangulJamoHasOnlyOneWeight。
// Go 遍历 Hangul Jamo 区间，确认每个表项高 48 位为 0，只保留一个 uint16 权重。
#[test]
/// 对应 Go `TestHangulJamoHasOnlyOneWeight`：Jamo 区间仅单权重。
pub fn test_hangul_jamo_has_only_one_weight() {
    for i in 0x1100usize..0x11ffusize {
        // 这里保留 Go 的按位掩码断言：若高位非零，说明该 Jamo 被错误压成了多权重。
        assert_eq!(0u64, DUCET0900Table.map_table4[i] & 0xFFFFFFFFFFFF0000u64);
    }
}

// test_first_is_not_zero 对应 Go 的 TestFirstIsNotZero。
// LongRuneMap 的首段权重不能为 0，否则调用方会把长权重项误判为空。
#[test]
/// 对应 Go `TestFirstIsNotZero`：`long_rune_map` 首段权重非零。
pub fn test_first_is_not_zero() {
    for (_rune, weights) in DUCET0900Table.long_rune_map.iter() {
        assert_ne!(weights[0], 0u64);
    }
}
