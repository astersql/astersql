// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// MAX/MIN 聚合与滑动窗口 deque 测试。
//
// 测试验证整组 MAX/MIN 排序，以及单调队列在入队/过期剔除后的队头极值。

use crate::func_max_min::{
    BinaryJson, DurationValue, MaxMin, MinMaxDeque, NamedValue, TimeValue, update_float32,
    update_float64,
};

/// 验证 MAX/MIN 忽略 NULL 取极值，以及 deque 入队支配与 dequeue 过期后的队头。
#[test]
fn max_min_and_sliding_deque_keep_production_ordering() {
    // MAX：跳过 None，结果为 5。
    let mut max = MaxMin::new(true);
    max.update([Some(2), None, Some(5), Some(1)]);
    assert_eq!(max.value(), Some(&5));
    // MIN：结果为 1。
    let mut min = MaxMin::new(false);
    min.update([Some(2), Some(5), Some(1)]);
    assert_eq!(min.value(), Some(&1));
    // 单调 MAX 队列：5 支配 2；再入 3 后队为 [5,3]；dequeue(2) 去掉下标 <2 的 5，队头剩 3。
    let mut deque = MinMaxDeque::new(true);
    deque.enqueue(0, 2, Ord::cmp);
    deque.enqueue(1, 5, Ord::cmp);
    deque.enqueue(2, 3, Ord::cmp);
    assert_eq!(deque.front().map(|pair| pair.item), Some(5));
    deque.dequeue(2);
    assert_eq!(deque.front().map(|pair| pair.item), Some(3));
}

/// Go `MinMaxDeque.Enqueue` removes equal tail values as well as dominated
/// values, retaining the newest row index for a peer value.
#[test]
fn sliding_deque_replaces_equal_tail_with_newest_index() {
    let mut max = MinMaxDeque::new(true);
    max.enqueue(4, 7, Ord::cmp);
    max.enqueue(9, 7, Ord::cmp);
    assert_eq!(
        max.front().map(|pair| (pair.index, pair.item)),
        Some((9, 7))
    );
    assert_eq!(max.back().map(|pair| (pair.index, pair.item)), Some((9, 7)));

    let mut min = MinMaxDeque::new(false);
    min.enqueue(4, 7, Ord::cmp);
    min.enqueue(9, 7, Ord::cmp);
    assert_eq!(
        min.front().map(|pair| (pair.index, pair.item)),
        Some((9, 7))
    );
}

/// Go compares TIME by its packed instant and DURATION by nanoseconds; display
/// metadata must not change MAX/MIN selection.
#[test]
fn temporal_max_min_ignores_type_and_fsp_metadata() {
    let earlier_with_larger_metadata = TimeValue {
        packed: 10,
        kind: u8::MAX,
        fsp: 6,
    };
    let later_with_smaller_metadata = TimeValue {
        packed: 11,
        kind: 0,
        fsp: 0,
    };
    let mut time_max = MaxMin::new(true);
    time_max.update([
        Some(earlier_with_larger_metadata),
        Some(later_with_smaller_metadata.clone()),
    ]);
    assert_eq!(time_max.value(), Some(&later_with_smaller_metadata));

    let first_time = TimeValue {
        packed: 20,
        kind: 1,
        fsp: 6,
    };
    let same_instant = TimeValue {
        packed: 20,
        kind: 2,
        fsp: 0,
    };
    let mut time_min = MaxMin::new(false);
    time_min.update([Some(first_time.clone()), Some(same_instant)]);
    assert_eq!(
        time_min.value().map(|value| (value.kind, value.fsp)),
        Some((1, 6))
    );

    let same_duration_different_fsp = [
        DurationValue { nanos: 42, fsp: 6 },
        DurationValue { nanos: 42, fsp: 0 },
    ];
    let mut duration_min = MaxMin::new(false);
    duration_min.update(same_duration_different_fsp.clone().map(Some));
    assert_eq!(duration_min.value().map(|value| value.fsp), Some(6));
}

/// Go ENUM/SET MAX/MIN compares the collated name only; the numeric payload is
/// retained from the first peer value when names compare equal.
#[test]
fn named_value_order_ignores_numeric_payload() {
    let first = NamedValue {
        name: "same".to_owned(),
        value: 9,
    };
    let peer = NamedValue {
        name: "same".to_owned(),
        value: 1,
    };
    let mut min = MaxMin::new(false);
    min.update([Some(first.clone()), Some(peer)]);
    assert_eq!(min.value().map(|value| value.value), Some(first.value));
}

/// Go compares signed and unsigned JSON numbers by numeric value, not their
/// distinct binary type codes.
#[test]
fn binary_json_order_uses_json_semantics() {
    let signed = BinaryJson {
        type_code: 0x09,
        value: 7_i64.to_le_bytes().to_vec(),
    };
    let unsigned = BinaryJson {
        type_code: 0x0a,
        value: 7_u64.to_le_bytes().to_vec(),
    };
    let mut max = MaxMin::new(true);
    max.update([Some(signed.clone()), Some(unsigned)]);
    assert_eq!(max.value().map(|value| value.type_code), Some(0x09));
}

/// Go's generic `cmp.Compare` orders NaN before ordinary floating-point
/// values, rather than treating incomparable values as equal.
#[test]
fn float_max_min_matches_go_nan_ordering() {
    let mut max32 = MaxMin::new(true);
    update_float32(&mut max32, [Some(f32::NAN), Some(1.0)]);
    assert_eq!(max32.value(), Some(&1.0));

    let mut min64 = MaxMin::new(false);
    update_float64(&mut min64, [Some(1.0), Some(f64::NAN)]);
    assert!(min64.value().is_some_and(|value| value.is_nan()));
}
