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

// 分页工具单元测试。
//
// 对照 Go `pkg/util/paging/paging_test.go`，覆盖分页大小按倍数增长与 seek 次数估算的纯计算用例。

// 对照 pkg/util/paging/paging_test.go，覆盖分页大小增长和 seek 次数估算的全部纯计算用例。

use super::paging::*;

/// 验证 `GrowPagingSize`：按 `pagingSizeGrow` 倍增，且不超过允许的最大分页上限。
// TestGrowPagingSize 对应 Go 的同名测试，验证分页大小按倍数增长并受最小上限保护。
#[test]
fn test_grow_paging_size() {
    // require.Equal 在 Go 中比较实际值和期望值；用 assert_eq! 保留断言语义。
    assert_eq!(
        GrowPagingSize(MinPagingSize, MinAllowedMaxPagingSize),
        MinPagingSize * pagingSizeGrow,
    );
    assert_eq!(
        GrowPagingSize(MinAllowedMaxPagingSize, MinAllowedMaxPagingSize),
        MinAllowedMaxPagingSize,
    );
    assert_eq!(
        GrowPagingSize(
            MinAllowedMaxPagingSize / pagingSizeGrow + 1,
            MinAllowedMaxPagingSize
        ),
        MinAllowedMaxPagingSize,
    );
}

/// 验证 `CalculateSeekCnt`：按分页增长序列估算 seek 次数，允许 0.1 浮点误差。
// TestCalculateSeekCnt 对应 Go 的 seek 次数估算测试。
// Go 使用 require.InDelta 允许 0.1 浮点误差；这里用本地闭包保留相同误差窗口。
#[test]
fn test_calculate_seek_cnt() {
    // 本地闭包模拟 Go require.InDelta，误差窗口同为 0.1。
    let assert_in_delta = |actual: f64, expected: f64| {
        assert!(
            (actual - expected).abs() <= 0.1,
            "actual {actual} expected {expected} within delta 0.1",
        );
    };

    assert_in_delta(CalculateSeekCnt(0), 0.0);
    assert_in_delta(CalculateSeekCnt(1), 1.0);
    assert_in_delta(CalculateSeekCnt(MinPagingSize), 1.0);
    assert_in_delta(
        CalculateSeekCnt(pagingGrowingSum),
        (maxPagingSizeShift + 1) as f64,
    );
    assert_in_delta(
        CalculateSeekCnt(pagingGrowingSum + 1),
        (maxPagingSizeShift + 2) as f64,
    );
    assert_in_delta(
        CalculateSeekCnt(pagingGrowingSum + MinAllowedMaxPagingSize),
        (maxPagingSizeShift + 2) as f64,
    );
}
