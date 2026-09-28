// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use std::mem::size_of;

use crate::func_sum_int::{
    DEF_PARTIAL_RESULT_4_SUM_DISTINCT_INT64_SIZE, DEF_PARTIAL_RESULT_4_SUM_DISTINCT_UINT64_SIZE,
    SumDistinctInt64, SumDistinctUint64,
};

#[test]
fn distinct_partial_result_sizes_match_the_concrete_states() {
    assert_eq!(
        DEF_PARTIAL_RESULT_4_SUM_DISTINCT_INT64_SIZE,
        size_of::<SumDistinctInt64>() as i64
    );
    assert_eq!(
        DEF_PARTIAL_RESULT_4_SUM_DISTINCT_UINT64_SIZE,
        size_of::<SumDistinctUint64>() as i64
    );
}

#[test]
fn distinct_reset_releases_set_capacity_like_go() {
    let mut signed = SumDistinctInt64::default();
    assert!(signed.update([Some(1)]) > 0);
    signed.reset();
    assert!(signed.update([Some(2)]) > 0);

    let mut unsigned = SumDistinctUint64::default();
    assert!(unsigned.update([Some(1)]) > 0);
    unsigned.reset();
    assert!(unsigned.update([Some(2)]) > 0);
}
