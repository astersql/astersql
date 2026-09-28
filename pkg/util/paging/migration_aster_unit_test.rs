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

// paging 迁移回归测试：GrowPagingSize 边界与 CalculateSeekCnt 增长区间对照 Go。

use super::{CalculateSeekCnt, GrowPagingSize, MinAllowedMaxPagingSize, MinPagingSize};

/// 几何级数前 8 项之和对应的行数上限，用于 seek 次数用例分界。
const PAGING_GROWING_SUM: u64 = ((2_u64 << 7) - 1) * MinPagingSize;

/// 验证倍增、顶到上限、防御性抬高 max、以及 u64 边界行为。
#[test]
fn grow_paging_size_matches_go_boundaries() {
    assert_eq!(
        GrowPagingSize(MinPagingSize, MinAllowedMaxPagingSize),
        MinPagingSize * 2,
    );
    assert_eq!(
        GrowPagingSize(MinAllowedMaxPagingSize, MinAllowedMaxPagingSize),
        MinAllowedMaxPagingSize,
    );
    assert_eq!(
        GrowPagingSize(MinAllowedMaxPagingSize / 2 + 1, MinAllowedMaxPagingSize),
        MinAllowedMaxPagingSize,
    );
    assert_eq!(GrowPagingSize(1, 0), 2);
    assert_eq!(GrowPagingSize(u64::MAX, u64::MAX), u64::MAX - 1);
}

/// 按期望行数落入的增长区间，验证 seek 次数估值与 Go 一致。
#[test]
fn calculate_seek_count_matches_go_growth_ranges() {
    let cases = [
        (0, 0.0),
        (1, 1.0),
        (MinPagingSize, 1.0),
        (MinPagingSize + 1, 1.0),
        (MinPagingSize * 2, 2.0),
        (PAGING_GROWING_SUM, 8.0),
        (PAGING_GROWING_SUM + 1, 9.0),
        (PAGING_GROWING_SUM + MinAllowedMaxPagingSize, 9.0),
    ];

    for (expect_count, expected) in cases {
        assert_eq!(
            CalculateSeekCnt(expect_count),
            expected,
            "expect_count={expect_count}"
        );
    }
}

/// u64 溢出环绕须与 Go uint64 一致，debug 构建也不可 panic。
#[test]
fn calculate_seek_count_preserves_go_u64_wraparound() {
    // Go uint64 arithmetic wraps here before the division. Rust must preserve
    // that behavior in debug builds instead of panicking on overflow.
    assert_eq!(CalculateSeekCnt(u64::MAX), 8.0);
}
