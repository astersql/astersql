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

// NTILE 窗口函数测试。
//
// 测试验证余数优先分配给靠前桶的分桶序列。


/// 验证 8 行分 3 桶时，余数 2 使前两桶各多一行：大小为 3/3/2。
#[test]
fn ntile_distributes_remainder_to_earlier_buckets() {
    // NTILE(3) 且 num_rows=8：quotient=2，remainder=2 → 桶序列 1,1,1,2,2,2,3,3。
    let mut ntile = crate::func_ntile::Ntile::new(Some(3));
    ntile.update(8);
    assert_eq!(
        (0..8)
            .map(|_| ntile.next_value().unwrap())
            .collect::<Vec<_>>(),
        vec![1, 1, 1, 2, 2, 2, 3, 3]
    );
}

/// Go 的 `ResetPartialResult` 只重置行数和输出游标；在下一次 update 前仍保留
/// 上个分区算出的桶宽。这个生命周期边界也应保持一致。
#[test]
fn ntile_reset_preserves_bucket_width_until_next_update() {
    let mut ntile = crate::func_ntile::Ntile::new(Some(3));
    ntile.update(8);
    ntile.reset();

    assert_eq!(
        (0..4).map(|_| ntile.next_value()).collect::<Vec<_>>(),
        vec![Some(1), Some(1), Some(1), Some(2)]
    );
}
