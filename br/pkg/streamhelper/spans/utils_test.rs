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

//! 对应 Go `utils_test.go`：验证 `ValuedSetEquals` 的覆盖等价语义。
//! 重点覆盖：严格相等、同值切分、取值/边界不一致、±∞ 端点与乱序输入。

use crate::{Span, Value, Valued, ValuedSetEquals};

/// 对应 Go `TestValuedEquals`：双向调用 `ValuedSetEquals`，结果须与 `required` 一致。
#[test]
fn test_valued_equals() {
    // 对应 Go 测试内闭包 `s`：用字符串键快速构造 Valued。
    let s = |start: &str, end: &str, val: Value| -> Valued {
        Valued {
            Key: Span {
                StartKey: start.as_bytes().to_vec(),
                EndKey: end.as_bytes().to_vec(),
            },
            Value: val,
        }
    };

    // required=true 表示两侧覆盖集合应判定相等。
    struct Case {
        input_a: Vec<Valued>,
        input_b: Vec<Valued>,
        required: bool,
    }

    let cases = vec![
        // 右端不同且无法靠切分对齐 → 不相等。
        Case {
            input_a: vec![s("0001", "0002", 3)],
            input_b: vec![s("0001", "0003", 3)],
            required: false,
        },
        // 完全相同分段 → 相等。
        Case {
            input_a: vec![s("0001", "0002", 3)],
            input_b: vec![s("0001", "0002", 3)],
            required: true,
        },
        // 同值被切成两段与一整段覆盖同一区间 → 相等。
        Case {
            input_a: vec![s("0001", "0003", 3)],
            input_b: vec![s("0001", "0002", 3), s("0002", "0003", 3)],
            required: true,
        },
        // 覆盖区间相同但取值不同 → 不相等。
        Case {
            input_a: vec![s("0001", "0003", 4)],
            input_b: vec![s("0001", "0002", 3), s("0002", "0003", 3)],
            required: false,
        },
        // 切分后某一段取值不同 → 不相等。
        Case {
            input_a: vec![s("0001", "0003", 3)],
            input_b: vec![s("0001", "0002", 4), s("0002", "0003", 3)],
            required: false,
        },
        // 右端延伸越过整段 → 覆盖不一致。
        Case {
            input_a: vec![s("0001", "0003", 3)],
            input_b: vec![s("0001", "0002", 3), s("0002", "0004", 3)],
            required: false,
        },
        // 左端从 -∞ 起，与从 0001 起的覆盖不对齐。
        Case {
            input_a: vec![s("", "0003", 3)],
            input_b: vec![s("0001", "0002", 3), s("0002", "0003", 3)],
            required: false,
        },
        // 右端为 ∞ 但中间出现空洞（缺 0003–0004）→ 不相等。
        Case {
            input_a: vec![s("0001", "", 1)],
            input_b: vec![s("0001", "0003", 1), s("0004", "", 1)],
            required: false,
        },
        // 输入顺序不同但集合相同 → 相等（实现内部会排序）。
        Case {
            input_a: vec![s("0001", "0004", 1), s("0001", "0002", 1)],
            input_b: vec![s("0001", "0002", 1), s("0001", "0004", 1)],
            required: true,
        },
    ];

    let run = |c: Case| {
        // Go 对两种参数顺序都断言，保证比较对称。
        assert_eq!(
            c.required,
            ValuedSetEquals(c.input_a.clone(), c.input_b.clone())
        );
        assert_eq!(c.required, ValuedSetEquals(c.input_b, c.input_a));
    };

    for (i, c) in cases.into_iter().enumerate() {
        // Go 子测试名：fmt.Sprintf("#%d", i+1)
        println!("#{}", i + 1);
        run(c);
    }
}
