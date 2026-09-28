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

// SUM 聚合函数测试。
//
// 可执行测试覆盖本模块的 SUM 状态契约。


/// 验证 FloatSum 累加、merge 与滑动窗口更新。
///
/// 路径：update(1,NULL,2,3)→6 → merge(4)→10 → slide 移出 1,2 移入 8 →15。
#[test]
fn float_sum_merges_and_slides_non_null_values() {
    let mut sum = crate::func_sum::FloatSum::default();
    sum.update([Some(1.0), None, Some(2.0), Some(3.0)]);
    assert_eq!(sum.value(), Some(6.0));
    let mut other = crate::func_sum::FloatSum::default();
    other.update([Some(4.0)]);
    sum.merge(&other);
    assert_eq!(sum.value(), Some(10.0));
    sum.slide([Some(1.0), Some(2.0)], [Some(8.0)]);
    assert_eq!(sum.value(), Some(15.0));
}

/// Go 的普通 Float SUM 逐项执行 `sum += value`，不会启用补偿求和。
#[test]
fn float_sum_preserves_go_sequential_addition() {
    let mut sum = crate::func_sum::FloatSum::default();
    sum.update([Some(1.0e16), Some(1.0), Some(-1.0e16)]);
    assert_eq!(sum.value(), Some(0.0));
}

/// Go 的 Float SUM 滑窗先加入窗口尾部，再移除窗口头部；浮点顺序可观察。
#[test]
fn float_sum_slide_preserves_go_update_order() {
    let mut sum = crate::func_sum::FloatSum::default();
    sum.update([Some(1.0e16)]);
    sum.slide([Some(1.0e16)], [Some(1.0)]);
    assert_eq!(sum.value(), Some(0.0));
}

/// Go Decimal SUM 也先加入 incoming；因此中间溢出必须在移除 outgoing 前返回。
#[test]
fn decimal_sum_slide_reports_go_ordered_intermediate_overflow() {
    use crate::func_sum::{Decimal, DecimalSum};

    let mut sum = DecimalSum::default();
    sum.update([Some(Decimal::new(i128::MAX - 1, 0))]).unwrap();
    let error = sum
        .slide(
            [Some(Decimal::new(i128::MAX - 1, 0))],
            [Some(Decimal::new(2, 0))],
        )
        .unwrap_err();
    assert!(error.0.contains("out of range"));
}

/// Go `map[float64]` 合并正负零，但因为 NaN 不等于自身而保留每次插入。
#[test]
fn distinct_float_sum_matches_go_float_key_equality() {
    let mut sum = crate::func_sum::DistinctFloatSum::default();
    sum.update([Some(0.0), Some(-0.0)]);
    assert_eq!(sum.len(), 1);

    let nan = f64::from_bits(0x7ff8_0000_0000_0001);
    sum.update([Some(nan), Some(nan)]);
    assert_eq!(sum.len(), 3);
    assert!(sum.value().unwrap().is_nan());
}

/// Go `MyDecimal::ToHashKey` 去除尾随零，数值相同但 scale 不同的值只保留一个。
#[test]
fn distinct_decimal_sum_matches_go_normalized_hash_key() {
    use crate::func_sum::{Decimal, DistinctDecimalSum};

    let mut sum = DistinctDecimalSum::default();
    sum.update([Some(Decimal::new(10, 1)), Some(Decimal::new(100, 2))]);
    assert_eq!(sum.len(), 1);
    assert_eq!(sum.value().unwrap(), Some(Decimal::new(10, 1)));
}
