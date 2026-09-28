// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// `VAR_SAMP`（样本方差）聚合函数测试。
//
// 对应 Go 的 `func_varsamp_test.go`：覆盖普通聚合与 partial merge，并验证
// original/partial DISTINCT 两个执行阶段都有独立的公开类型名。

use crate::func_varsamp::{
    VarSamp4Float64, VarSampOriginal4DistinctFloat64, VarSampPartial4DistinctFloat64,
};

#[test]
fn test_varsamp() {
    let mut state = VarSamp4Float64::default();
    state.update((0..5).map(|value| Some(value as f64)));
    assert_eq!(state.sample_variance(), Some(2.5));

    let mut empty = VarSamp4Float64::default();
    assert_eq!(empty.sample_variance(), None);
    empty.update([Some(1.0), None]);
    assert_eq!(empty.sample_variance(), None);
}

#[test]
fn test_merge_partial_result4_varsamp() {
    let mut left = VarSamp4Float64::default();
    left.update([Some(0.0), Some(1.0), Some(2.0)]);
    let mut right = VarSamp4Float64::default();
    right.update([Some(3.0), Some(4.0)]);
    left.merge(&right);
    assert_eq!(left.count(), 5);
    assert_eq!(left.sample_variance(), Some(2.5));
}

#[test]
fn distinct_original_and_partial_stages_match_go_contract() {
    let mut original = VarSampOriginal4DistinctFloat64::default();
    original.update([Some(1.0), Some(1.0), Some(2.0)]);
    let mut partial = VarSampPartial4DistinctFloat64::default();
    partial.update([Some(3.0)]);
    original.merge(&partial);
    assert_eq!(original.sample_variance(), Some(1.0));
}

/// 校验 distinct 输入去重后样本方差为 1.0（集合 {1,2,3}，除以 n-1）。
#[test]
fn sample_variance_uses_distinct_values_and_n_minus_one() {
    // 重复的 1.0 只计一次；去重后 n=3，样本方差 (0+1+1)/2 = 1。
    let mut state = crate::func_varpop::DistinctVariance::default();
    state.update([Some(1.0), Some(1.0), Some(2.0), Some(3.0)]);
    assert_eq!(state.sample_variance(), Some(1.0));
}
