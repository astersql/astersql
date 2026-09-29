// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// `VAR_POP`（总体方差）聚合函数测试。
//
// 测试验证 `VarianceState::merge` 在分区合并后不损失精度。

/// 校验两分区总体方差合并后与一次性累计结果一致（期望值为 1.25）。
#[test]
fn population_variance_merges_partitions_without_precision_loss() {
    // 左分区 {1,2}、右分区 {3,4}，合并后应对齐全集 {1,2,3,4} 的总体方差。
    let mut left = crate::func_varpop::VarianceState::default();
    left.update([Some(1.0), Some(2.0)]);
    let mut right = crate::func_varpop::VarianceState::default();
    right.update([Some(3.0), Some(4.0)]);
    left.merge(&right);
    assert_eq!(left.count(), 4);
    assert_eq!(left.population_variance(), Some(1.25));
}

/// 覆盖 Go `TestVarpop` 的空输入、NULL 跳过、正常结果与 reset 生命周期。
#[test]
fn population_variance_matches_go_normal_and_empty_cases() {
    let mut state = crate::func_varpop::VarianceState::default();
    assert_eq!(state.population_variance(), None);

    state.update([None, Some(0.0), Some(1.0), Some(2.0), Some(3.0), Some(4.0)]);
    assert_eq!(state.population_variance(), Some(2.0));

    state.reset();
    assert_eq!(state.count(), 0);
    assert_eq!(state.population_variance(), None);
}

/// Go 的 `map[float64]` 将正零与负零视为同一 DISTINCT 键。
#[test]
fn distinct_population_variance_treats_signed_zero_as_one_value() {
    let mut state = crate::func_varpop::DistinctVariance::default();
    state.update([Some(0.0), Some(-0.0), Some(1.0)]);

    assert_eq!(state.population_variance(), Some(0.25));
}
