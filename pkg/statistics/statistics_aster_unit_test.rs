// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// Aster 侧统计单元测试：FM Sketch 去重/合并/编解码与 GEE NDV 估计。

use crate::*;

/// 验证 FM Sketch 去重、压缩合并、内存占用与编解码往返。
#[test]
fn fm_sketch_deduplicates_compacts_merges_and_round_trips() {
    let statement_context = stmtctx::NewStmtCtx();
    let mut left = NewFMSketch(2);
    left.InsertValue(&statement_context, types::NewIntDatum(1))
        .unwrap();
    left.InsertValue(&statement_context, types::NewIntDatum(1))
        .unwrap();
    assert_eq!(left.NDV(), 1);
    left.InsertValue(&statement_context, types::NewIntDatum(2))
        .unwrap();

    let mut right = NewFMSketch(2);
    right
        .InsertRowValue(
            &statement_context,
            &[types::NewIntDatum(3), types::NewStringDatum("x".to_owned())],
        )
        .unwrap();
    left.MergeFMSketch(&right);
    assert!(left.NDV() >= 2);
    assert!(left.MemoryUsage() >= 16);

    let encoded = EncodeFMSketch(Some(&left)).unwrap();
    let decoded = DecodeFMSketch(Some(&encoded)).unwrap().unwrap();
    assert_eq!(decoded.NDV(), left.NDV());
    assert_eq!(decoded.mask(), left.mask());
    assert_eq!(decoded.hash_values(), left.hash_values());
}

/// 验证 GEE（Good-Turing 类）NDV 估计在边界与全单例场景下的取值范围。
#[test]
fn gee_estimator_matches_go_bounds_and_singleton_behavior() {
    assert_eq!(EstimateNDVByGEE(10, 0, 100, 1_000), 10);
    let estimate = EstimateNDVByGEE(10, 5, 100, 1_000);
    assert!((10..=1_000).contains(&estimate));
    assert_eq!(EstimateNDVByGEE(10, 10, 10, 10), 10);
}
