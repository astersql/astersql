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

// RANK / DENSE_RANK 窗口函数测试。
//
// 测试对比 `Rank`（标准排名）与 `DenseRank`（稠密排名）在 peer 并列后的序号差异。
// 窗口函数（window function）按分区内排序结果编号，不改变结果集行数；
// peer 指排序键相同的并列行。

/// 验证 RANK 与 DENSE_RANK 仅在 peer 出现 gap 后才分叉。
///
/// 输入 `[1,1,3,4]`：并列 1 后，RANK 跳到 3（留出并列占位），DENSE_RANK 连续为 2。
#[test]
fn rank_and_dense_rank_diverge_only_after_peer_gaps() {
    let rows = [1, 1, 3, 4];
    // dense=false：标准 RANK，并列后序号跳过。
    let mut rank = crate::func_rank::Rank::new(false);
    rank.update(rows);
    assert_eq!(
        (0..4).map(|_| rank.next().unwrap()).collect::<Vec<_>>(),
        vec![1, 1, 3, 4]
    );
    // dense=true：DENSE_RANK，并列后序号连续递增。
    let mut dense = crate::func_rank::Rank::new(true);
    dense.update(rows);
    assert_eq!(
        (0..4).map(|_| dense.next().unwrap()).collect::<Vec<_>>(),
        vec![1, 1, 2, 3]
    );
}

/// Go 的 `rank.UpdatePartialResult` 使用包级 `DefRowSize` 记账；Rank 不能另造
/// 一个基于 `usize` 的行大小，否则窗口函数间的内存增量会不一致。
#[test]
fn rank_uses_the_shared_row_memory_size() {
    assert_eq!(
        crate::func_rank::DEF_ROW_SIZE,
        crate::aggfuncs::DEF_ROW_SIZE
    );

    let mut rank = crate::func_rank::Rank::new(false);
    assert_eq!(rank.update([1, 2, 3]), 3 * crate::aggfuncs::DEF_ROW_SIZE);
}

#[test]
fn row_comparer_uses_the_first_non_equal_ordering_column() {
    type Row = (i32, i32);
    let comparer = crate::func_rank::RowComparer::new(vec![
        |left: &Row, right: &Row| left.0.cmp(&right.0),
        |left: &Row, right: &Row| left.1.cmp(&right.1),
    ]);

    assert_eq!(comparer.compare(&(1, 9), &(2, 0)), std::cmp::Ordering::Less);
    assert_eq!(
        comparer.compare(&(1, 9), &(1, 3)),
        std::cmp::Ordering::Greater
    );
    assert_eq!(
        comparer.compare(&(1, 9), &(1, 9)),
        std::cmp::Ordering::Equal
    );
    assert_eq!(
        crate::func_rank::RowComparer::<Row>::new(vec![]).compare(&(1, 9), &(2, 0)),
        std::cmp::Ordering::Equal
    );
}

#[test]
fn rank_reset_reuses_the_state_for_a_new_partition() {
    let mut rank = crate::func_rank::Rank::new(false);
    assert_eq!(rank.next(), None);
    rank.update([1, 1, 2]);
    assert_eq!(rank.next(), Some(1));
    assert_eq!(rank.next(), Some(1));
    assert_eq!(rank.state().cur_idx, 2);

    rank.reset();
    assert_eq!(rank.state().cur_idx, 0);
    assert_eq!(rank.state().last_rank, 0);
    assert!(rank.state().rows.is_empty());
    rank.update([8, 9]);
    assert_eq!(rank.next(), Some(1));
    assert_eq!(rank.next(), Some(2));
    assert_eq!(rank.next(), None);
}
