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

// PERCENT_RANK 窗口函数测试，对齐 Go `TestMemPercentRank` 的三种输入形状，
// 并覆盖 Go 实现的 Reset、peer 比较和最终结果公式。

use crate::func_rank::DEF_ROW_SIZE;

#[test]
fn update_reports_go_row_memory_delta_for_each_batch_shape() {
    let cases = [vec![0_i64], vec![0, 0, 0], vec![0, 1, 2, 3]];

    for rows in cases {
        let expected = rows.len() as i64 * DEF_ROW_SIZE;
        let mut rank = crate::func_percent_rank::PercentRank::default();
        assert_eq!(rank.update(rows), expected);
    }
}

/// 验证输入 \[1,1,3,4\] 时：同行组百分位同为 0，随后为 2/3 与 1。
#[test]
fn percent_rank_uses_rank_gaps_for_peer_groups() {
    // n=4：rank 序列 1,1,3,4 → PERCENT_RANK = 0, 0, 2/3, 1。
    let mut rank = crate::func_percent_rank::PercentRank::default();
    rank.update([1, 1, 3, 4]);
    assert_eq!(rank.next(), Some(0.0));
    assert_eq!(rank.next(), Some(0.0));
    assert_eq!(rank.next(), Some(2.0 / 3.0));
    assert_eq!(rank.next(), Some(1.0));
    assert_eq!(rank.next(), None);
}

#[test]
fn single_row_is_zero_and_reset_reuses_the_partial_result() {
    let mut rank = crate::func_percent_rank::PercentRank::default();
    assert_eq!(rank.update([7]), DEF_ROW_SIZE);
    assert_eq!(rank.next(), Some(0.0));

    rank.reset();
    assert_eq!(rank.update([2, 3]), 2 * DEF_ROW_SIZE);
    assert_eq!(rank.next(), Some(0.0));
    assert_eq!(rank.next(), Some(1.0));
}

#[test]
fn next_by_uses_ordering_equality_to_identify_peers() {
    let mut rank = crate::func_percent_rank::PercentRank::default();
    rank.update([(1, "a"), (1, "b"), (2, "a")]);

    let compare_first_column = |left: &(i32, &str), right: &(i32, &str)| left.0.cmp(&right.0);
    assert_eq!(rank.next_by(compare_first_column), Some(0.0));
    assert_eq!(rank.next_by(compare_first_column), Some(0.0));
    assert_eq!(rank.next_by(compare_first_column), Some(1.0));
}
