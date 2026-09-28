// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// CUME_DIST 窗口函数单元测试：覆盖 Go 的部分结果大小、逐行内存增量、
// reset 复用，以及 peer group 在末秩处共享同一累积分布值。

use crate::func_cume_dist::{CumeDist, DEF_PARTIAL_RESULT_CUME_DIST_SIZE};
use crate::func_rank::DEF_ROW_SIZE;
use std::mem::size_of;

#[test]
fn cume_dist_memory_accounting_and_reset_match_go() {
    assert_eq!(
        DEF_PARTIAL_RESULT_CUME_DIST_SIZE,
        size_of::<CumeDist<()>>() as i64
    );

    let mut state = CumeDist::default();
    assert_eq!(state.update([1]), DEF_ROW_SIZE);
    assert_eq!(state.update([1, 2]), 2 * DEF_ROW_SIZE);
    assert_eq!(state.next(), Some(2.0 / 3.0));

    state.reset();
    assert_eq!(state.next(), None);
    assert_eq!(state.update([7, 7, 7, 7]), 4 * DEF_ROW_SIZE);
    assert_eq!(state.next(), Some(1.0));
}

#[test]
fn cume_dist_uses_the_supplied_order_by_peer_comparer() {
    let mut state = CumeDist::default();
    state.update([(1, "a"), (1, "b"), (2, "a")]);

    let compare_first_column = |left: &(i32, &str), right: &(i32, &str)| left.0.cmp(&right.0);
    assert_eq!(state.next_by(compare_first_column), Some(2.0 / 3.0));
    assert_eq!(state.next_by(compare_first_column), Some(2.0 / 3.0));
    assert_eq!(state.next_by(compare_first_column), Some(1.0));
    assert_eq!(state.next_by(compare_first_column), None);
}

/// 校验序列 `[1,1,2,3,3]` 上各 peer 组的 CUME_DIST：2/5、3/5、5/5。
#[test]
fn cume_dist_advances_peer_groups_at_their_last_rank() {
    let mut state = CumeDist::default();
    state.update([1, 1, 2, 3, 3]);
    // 两个 1：last_rank=2 => 0.4；单个 2：0.6；两个 3：1.0。
    assert_eq!(state.next(), Some(0.4));
    assert_eq!(state.next(), Some(0.4));
    assert_eq!(state.next(), Some(0.6));
    assert_eq!(state.next(), Some(1.0));
    assert_eq!(state.next(), Some(1.0));
    assert_eq!(state.next(), None);
}
