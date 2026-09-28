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
// 块注释内保留 Go `TestMemNtile` 的部分结果大小与默认内存增量绑定；
// 可执行部分验证余数优先分配给靠前桶的分桶序列。

/*
// NTILE 窗口函数部分结果的内存测试用例。
// WindowMemCaseDraft 对应 Go windowMemTest 的测试输入轮廓。
pub struct WindowMemCaseDraft {
    pub funcName: &'static str,
    pub fieldType: &'static str,
    pub begin: i64,
    pub end: i64,
    pub step: i64,
    pub partialResultSize: &'static str,
    pub updateMemDelta: &'static str,
}

// test_mem_ntile 对应 Go TestMemNtile，验证 NTILE 在不同分区行数下使用同一部分结果大小和默认内存增量函数。
#[test]
pub fn test_mem_ntile() {
    let tests = vec![
        WindowMemCaseDraft {
            funcName: "ast.WindowFuncNtile",
            fieldType: "mysql.TypeLonglong",
            begin: 1,
            end: 1,
            step: 1,
            partialResultSize: "aggfuncs.DefPartialResult4Ntile",
            updateMemDelta: "defaultUpdateMemDeltaGens",
        },
        WindowMemCaseDraft {
            funcName: "ast.WindowFuncNtile",
            fieldType: "mysql.TypeLonglong",
            begin: 1,
            end: 3,
            step: 0,
            partialResultSize: "aggfuncs.DefPartialResult4Ntile",
            updateMemDelta: "defaultUpdateMemDeltaGens",
        },
        WindowMemCaseDraft {
            funcName: "ast.WindowFuncNtile",
            fieldType: "mysql.TypeLonglong",
            begin: 1,
            end: 4,
            step: 1,
            partialResultSize: "aggfuncs.DefPartialResult4Ntile",
            updateMemDelta: "defaultUpdateMemDeltaGens",
        },
    ];

    for test in tests {
        // Go 这里直接调用 testWindowAggMemFunc(t, test)；只保留 harness 委托语义。
        testWindowAggMemFunc(test);
    }
}
*/

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
