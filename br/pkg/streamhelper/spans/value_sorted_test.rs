// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! 对应 Go `value_sorted_test.go`：验证值序索引在 Merge 后的过滤遍历。
//! Helpers `s`/`kv` 复用同包 `sorted_test`（与 Go 同包测试 helper 一致）。
//! 断言侧用 `TraverseValuesLessThan` + `ValuedSetEquals`，只关心 Value < 阈值的覆盖。
//! 推进器依赖该索引找出“最落后”区间，故阈值过滤是核心契约。

use crate::sorted_test::{kv, s};
use crate::{Debug, Full, NewFullWith, Sorted, Value, Valued, ValuedSetEquals};

/// 对应 Go `TestSortedBasic`：顺序 Merge 后按 `retain_less_than` 过滤值序结果。
/// 逐步 Debug 打印，便于对照 Go 侧键序/值序双视图。
#[test]
fn test_sorted_basic() {
    // retain_less_than：只保留 Value 严格小于该阈值的段。
    // 阈值本身不包含在结果中（LessThan，非 LessOrEqual）。
    struct Case {
        input_sequence: Vec<Valued>,
        retain_less_than: Value,
        result: Vec<Valued>,
    }

    let run = |c: Case| {
        // 全键空间初值 0，再包一层 Sorted 以维护 valueIdx。
        let mut full = Sorted(NewFullWith(&Full(), 0));
        println!("test_sorted_basic");
        for i in c.input_sequence {
            full.Merge(i);
            // 对应 Go `spans.Debug(full)`：打印键序与值序两份视图。
            Debug(&full);
        }

        // 值序过滤：验证索引与主树在阈值语义上一致。
        // 若索引未正确删旧插新，这里会多出或缺少段。
        let mut result: Vec<Valued> = Vec::new();
        full.TraverseValuesLessThan(c.retain_less_than, |v| {
            result.push(v);
            true
        });

        assert!(
            ValuedSetEquals(result.clone(), c.result.clone()),
            "{:?}\nvs\n{:?}",
            result,
            c.result
        );
    };

    let cases = vec![
        // 阈值 10：所有段 Value 均 < 10，结果等于完整键序覆盖。
        // 作为值序与键序一致性的基线用例。
        Case {
            input_sequence: vec![kv(s("0001", "0002"), 1), kv(s("0002", "0003"), 2)],
            result: vec![
                kv(s("", "0001"), 0),
                kv(s("0001", "0002"), 1),
                kv(s("0002", "0003"), 2),
                kv(s("0003", ""), 0),
            ],
            retain_less_than: 10,
        },
        // 阈值 1：仅初值 0 的两端空隙保留；中间已被更大值覆盖。
        Case {
            input_sequence: vec![
                kv(s("0001", "0002"), 1),
                kv(s("0002", "0003"), 2),
                kv(s("0001", "0003"), 4),
            ],
            retain_less_than: 1,
            result: vec![kv(s("", "0001"), 0), kv(s("0003", ""), 0)],
        },
        // 阈值 5：排除 Value=5 的 `[0004,0008)`，其余落后段保留。
        Case {
            input_sequence: vec![
                kv(s("0001", "0004"), 3),
                kv(s("0004", "0008"), 5),
                kv(s("0001", "0007"), 4),
                kv(s("", "0002"), 2),
            ],
            retain_less_than: 5,
            result: vec![
                kv(s("", "0001"), 2),
                kv(s("0001", "0004"), 4),
                kv(s("0008", ""), 0),
            ],
        },
        // 多次抬高后阈值 11：只剩 Value 为 5 与 10 的两段（20 被排除）。
        Case {
            input_sequence: vec![
                kv(s("0001", "0004"), 3),
                kv(s("0004", "0008"), 5),
                kv(s("0001", "0007"), 4),
                kv(s("", "0002"), 2),
                kv(s("0001", "0004"), 5),
                kv(s("0008", ""), 10),
                kv(s("", "0001"), 20),
            ],
            retain_less_than: 11,
            result: vec![kv(s("0001", "0008"), 5), kv(s("0008", ""), 10)],
        },
    ];

    for (i, c) in cases.into_iter().enumerate() {
        // Go 子测试名：fmt.Sprintf("#%d", i+1)
        println!("#{}", i + 1);
        run(c);
    }
}
