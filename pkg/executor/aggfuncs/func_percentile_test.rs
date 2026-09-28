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

// APPROX_PERCENTILE 聚合测试。
//
// 测试验证 50% 序数秩选择、忽略 NULL，以及合并后源缓冲被清空。


/// 验证 50% 百分位对 \[9,1,5,3,7\]（忽略 NULL）选出中位 5，且 merge 清空源。
#[test]
fn percentile_selects_one_based_ordinal_and_ignores_nulls() {
    // 有效样本 5 个：ceil(0.5*5)=3 → 第 3 小为 5；None 不进入样本。
    let mut percentile = crate::func_percentile::Percentile::new(50);
    percentile.update([Some(9), None, Some(1), Some(5), Some(3), Some(7)]);
    assert_eq!(percentile.result(), Some(&5));
    // 合并后源侧 samples 应被 append 清空。
    let mut source = crate::func_percentile::Percentile::new(50);
    source.update([Some(11)]);
    percentile.merge_from(&mut source);
    assert!(source.values().is_empty());
}

#[test]
fn percentile_matches_go_boundaries_memory_and_reset_lifecycle() {
    use crate::func_percentile::{DEF_SLICE_SIZE, Percentile};
    use std::mem::size_of;

    assert_eq!(DEF_SLICE_SIZE, size_of::<Vec<()>>() as i64);
    assert_eq!(crate::func_percentile::ordinal_rank(0, 100), 0);
    assert_eq!(crate::func_percentile::ordinal_rank(28, 100), 28);

    let mut percentile = Percentile::new(100);
    assert_eq!(
        percentile.update([Some(1_i64), None, Some(28)]),
        2 * size_of::<i64>() as i64
    );
    assert_eq!(percentile.result(), Some(&28));
    percentile.reset();
    assert!(percentile.values().is_empty());
    assert_eq!(
        percentile.capacity(),
        0,
        "Go reset releases the backing slice"
    );

    let mut zero = Percentile::new(0);
    zero.update([Some(1_i64)]);
    assert_eq!(zero.result(), None);
}

#[test]
fn percentile_executes_all_go_typed_paths() {
    use crate::func_max_min::{DurationValue, TimeValue};
    use crate::func_percentile::Percentile;
    use crate::func_sum::Decimal;

    let mut real32 = Percentile::new(50);
    real32.update([Some(4.0_f32), Some(2.0), Some(3.0)]);
    assert_eq!(real32.result_float32(), Some(&3.0));

    let mut real64 = Percentile::new(50);
    real64.update([Some(4.0_f64), Some(2.0), Some(3.0)]);
    assert_eq!(real64.result_float64(), Some(&3.0));

    let mut decimal = Percentile::new(50);
    decimal.update([
        Some(Decimal::new(400, 2)),
        Some(Decimal::new(200, 2)),
        Some(Decimal::new(300, 2)),
    ]);
    assert_eq!(decimal.result(), Some(&Decimal::new(300, 2)));

    let mut time = Percentile::new(50);
    time.update([4, 2, 3].map(|packed| {
        Some(TimeValue {
            packed,
            kind: 1,
            fsp: 0,
        })
    }));
    assert_eq!(time.result().map(|value| value.packed), Some(3));

    let mut duration = Percentile::new(50);
    duration.update([4, 2, 3].map(|nanos| Some(DurationValue { nanos, fsp: 0 })));
    assert_eq!(duration.result().map(|value| value.nanos), Some(3));
}
