// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 聚合函数通用测试夹具与契约测试。
//
// 可执行测试验证 AVG/COUNT 等实现的 update → merge → reset 契约。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_variables,
    unused_mut
)]


/// 可执行契约测试依赖的生产聚合实现。
use crate::func_avg::FloatAvg;
use crate::func_count::CountAggregator;

/// 验证 Go 通用聚合夹具的核心契约：NULL 跳过、partial/final 合并、
/// 空输入结果、reset 以及 reset 后的状态复用。
#[test]
fn generic_aggregate_partial_final_and_reset_contract_matches_go() {
    let empty = FloatAvg::default();
    assert_eq!(empty.partial_result(), (0, 0.0));
    assert_eq!(empty.result(), None);

    let mut left = FloatAvg::default();
    left.update([Some(1.0), None, Some(3.0)]);
    let mut right = FloatAvg::default();
    right.update([Some(5.0), Some(7.0)]);
    left.merge(&right);
    assert_eq!(left.partial_result(), (4, 16.0));
    assert_eq!(left.result(), Some(4.0));
    left.reset();
    assert_eq!(left.partial_result(), (0, 0.0));
    assert_eq!(left.result(), None);
    left.update_partial([Some((2, 10.0)), None, Some((0, 99.0))]);
    // Go updates the partial sum before adding its count, so a non-null sum
    // remains observable even when the corresponding count is zero.
    assert_eq!(left.partial_result(), (2, 109.0));
    assert_eq!(left.result(), Some(54.5));

    let mut count = CountAggregator::default();
    count.update([Some(1), None, Some(2)]).unwrap();
    assert_eq!(count.value(), 2);
    count.update_partial([Some(3), None]).unwrap();
    assert_eq!(count.value(), 5);

    let mut other_count = CountAggregator::default();
    other_count.update([Some("x"), None, Some("y")]).unwrap();
    count.merge(&other_count).unwrap();
    assert_eq!(count.value(), 7);
    count.reset();
    assert_eq!(count.value(), 0);
    count.update([Some(true), None]).unwrap();
    assert_eq!(count.value(), 1);
}
