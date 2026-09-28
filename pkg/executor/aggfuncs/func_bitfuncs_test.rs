// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 位运算聚合函数单元测试。

use crate::aggfuncs::PartialResult4BitFunc;
use crate::func_bitfuncs::{BitAggKind, BitAggregator, DEF_PARTIAL_RESULT_4_BIT_FUNC_SIZE};
use std::mem::size_of;

/// 校验 BIT_OR / BIT_AND / BIT_XOR 的累积结果，以及 XOR 滑动窗口恒等式。
#[test]
fn bit_aggregates_update_merge_and_slide_with_go_identities() {
    // 1|4 = 5；NULL 跳过。
    let mut or = BitAggregator::new(BitAggKind::Or);
    or.update([Some(1), Some(4), None]);
    assert_eq!(or.value(), 5);
    // 7&3 = 3。
    let mut and = BitAggregator::new(BitAggKind::And);
    and.update([Some(7), Some(3)]);
    assert_eq!(and.value(), 3);
    // 1^2^3 = 0；再 slide 移出 1、移入 4：0^1^4 = 5。
    let mut xor = BitAggregator::new(BitAggKind::Xor);
    xor.update([Some(1), Some(2), Some(3)]);
    assert_eq!(xor.value(), 0);
    xor.slide([Some(1)], [Some(4)]);
    assert_eq!(xor.value(), 5);
}

/// 对应 Go TestMergePartialResult4BitFuncs 的三种 partial-result merge。
#[test]
fn bit_aggregates_merge_partial_results_like_go() {
    let cases = [
        (BitAggKind::And, 0_i64, 0_i64, 0_u64),
        (BitAggKind::Or, 7, 7, 7),
        (BitAggKind::Xor, 4, 5, 1),
    ];

    for (kind, source_value, destination_value, expected) in cases {
        let mut source = BitAggregator::new(kind);
        source.update([Some(source_value)]);
        let mut destination = BitAggregator::new(kind);
        destination.update([Some(destination_value)]);

        destination.merge(&source);

        assert_eq!(destination.value(), expected, "merge mismatch for {kind:?}");
    }
}

/// Go 的 BIT_AND 空集/reset 为 MaxUint64，OR/XOR 则为 0；NULL 不改变状态。
#[test]
fn bit_aggregates_empty_null_and_reset_states_match_go() {
    for (kind, identity) in [
        (BitAggKind::Or, 0),
        (BitAggKind::Xor, 0),
        (BitAggKind::And, u64::MAX),
    ] {
        let mut aggregate = BitAggregator::new(kind);
        assert_eq!(aggregate.value(), identity);
        aggregate.update([None, Some(-1), None]);
        assert_eq!(aggregate.value(), u64::MAX);
        aggregate.reset();
        assert_eq!(aggregate.value(), identity);
    }
}

/// 对应 Go TestMemBitFunc：三种聚合共用一个 u64 partial result。
#[test]
fn bit_aggregate_partial_result_size_matches_go() {
    assert_eq!(
        DEF_PARTIAL_RESULT_4_BIT_FUNC_SIZE,
        size_of::<PartialResult4BitFunc>() as i64
    );
    assert_eq!(DEF_PARTIAL_RESULT_4_BIT_FUNC_SIZE, size_of::<u64>() as i64);
}
