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

// AVG 聚合函数单元测试。
//
// 测试覆盖 Float AVG 的 partial 合并、NULL 忽略，以及 DISTINCT AVG。

use crate::func_avg::{DecimalAvg, DistinctFloatAvg, FloatAvg};
use crate::func_sum::Decimal;

/// 校验 Float AVG：合并 `(count, sum)` partial、跳过 NULL，以及 DISTINCT 去重平均。
#[test]
fn float_avg_merges_partials_and_ignores_nulls() {
    // (2,4.0)+(2,12.0) => sum=16, count=4 => avg=4.0；中间 None 被忽略。
    let mut avg = FloatAvg::default();
    avg.update_partial([Some((2, 4.0)), None, Some((2, 12.0))]);
    assert_eq!(avg.result(), Some(4.0));

    // DISTINCT：{1.0, 3.0} 平均为 2.0；重复 1.0 与 None 不计入。
    let mut distinct = DistinctFloatAvg::default();
    distinct.update([Some(1.0), Some(1.0), Some(3.0), None]);
    assert_eq!(distinct.result(), Some(2.0));
}

/// Go 的普通与 HighPrecision Float AVG 都按输入顺序直接执行 `sum += value`。
#[test]
fn float_avg_preserves_go_sequential_addition() {
    let mut avg = FloatAvg::default();
    avg.update([Some(1.0e16), Some(1.0), Some(-1.0e16)]);
    assert_eq!(avg.result(), Some(0.0));
}

/// Go partial 更新先累加 sum，再累加 count；零 count 不会丢弃非零 sum。
#[test]
fn float_avg_preserves_zero_count_partial_sum() {
    let mut avg = FloatAvg::default();
    avg.update_partial([Some((0, 5.0))]);
    avg.update([Some(1.0)]);
    assert_eq!(avg.partial_result(), (1, 6.0));
    assert_eq!(avg.result(), Some(6.0));
}

#[test]
fn decimal_avg_preserves_zero_count_partial_sum() {
    let mut avg = DecimalAvg::default();
    avg.update_partial([Some((0, Decimal::new(50, 1)))])
        .unwrap();
    avg.update([Some(Decimal::new(10, 1))]).unwrap();
    assert_eq!(avg.partial_result(), (1, Decimal::new(60, 1)));
    assert_eq!(avg.result(1).unwrap(), Some(Decimal::new(60, 1)));
}
