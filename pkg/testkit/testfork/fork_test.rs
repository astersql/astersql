// Copyright 2022 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// `testfork` 组合枚举行为测试。
//
// 验证 `Pick` / `PickEnum` 在条件分支下仍能穷尽所有取值组合。

use super::fork::{Pick, PickEnum, RunTest};

/// 第三维取值：按 `x` 分支选择数值或文本，用于构造不规则组合树。
#[derive(Debug, PartialEq, Eq)]
enum ThirdValue {
    /// `x == 2` 时走数值候选。
    Number(i32),
    /// 其它 `x` 时走文本候选。
    Text(String),
}

/// 对照 Go 子测试：枚举 x∈{1,2,3}、y∈{a,b} 及条件 z，共 12 组组合。
#[test]
fn TestForkSubTest() {
    let mut values = Vec::new();

    // 条件 Pick：x==2 时 z 取数字，否则取字符串，组合数仍应穷尽。
    RunTest(|t| {
        let x = Pick(t, vec![1, 2, 3]);
        let y = PickEnum(t, "a".to_owned(), vec!["b".to_owned()]);
        let z = if x == 2 {
            ThirdValue::Number(PickEnum(t, 10, vec![11]))
        } else {
            ThirdValue::Text(Pick(t, vec!["g".to_owned(), "h".to_owned()]))
        };
        values.push((x, y, z));
    });

    assert_eq!(
        values,
        vec![
            (1, "a".to_owned(), ThirdValue::Text("g".to_owned())),
            (1, "a".to_owned(), ThirdValue::Text("h".to_owned())),
            (1, "b".to_owned(), ThirdValue::Text("g".to_owned())),
            (1, "b".to_owned(), ThirdValue::Text("h".to_owned())),
            (2, "a".to_owned(), ThirdValue::Number(10)),
            (2, "a".to_owned(), ThirdValue::Number(11)),
            (2, "b".to_owned(), ThirdValue::Number(10)),
            (2, "b".to_owned(), ThirdValue::Number(11)),
            (3, "a".to_owned(), ThirdValue::Text("g".to_owned())),
            (3, "a".to_owned(), ThirdValue::Text("h".to_owned())),
            (3, "b".to_owned(), ThirdValue::Text("g".to_owned())),
            (3, "b".to_owned(), ThirdValue::Text("h".to_owned())),
        ]
    );
}
