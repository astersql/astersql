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

// testfork 迁移期单元测试。
//
// 校验 `RunTest` / `Pick` / `PickEnum` 的分支依赖笛卡尔积枚举顺序、
// pick 栈非法状态报错、ValuesText 格式，以及失败后仍遍历剩余组合——与 Go 行为对齐。

use super::*;
use std::cell::Cell;

/// 第三维取值：`x == 2` 时走数值分支，否则走文本分支，用于构造依赖分支的组合空间。
#[derive(Debug, PartialEq)]
enum ThirdValue {
    Number(i32),
    Text(String),
}

/// 验证依赖分支的笛卡尔积展开顺序与 Go 一致（x×y×z，且 z 随 x 变化）。
#[test]
fn run_test_enumerates_the_same_branch_dependent_cartesian_product_as_go() {
    let mut values = Vec::new();

    // RunTest 反复调用闭包，Pick/PickEnum 按栈深度推进组合；x==2 时 z 为数值，否则为文本。
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

/// 空候选列表与非法栈深度（pos 超出当前层）应返回与 Go 相同的错误文案。
#[test]
fn pick_stack_rejects_empty_values_and_illegal_depth() {
    let mut stack = newPickStack();
    assert_eq!(
        stack.PickValue(Vec::new()).unwrap_err().to_string(),
        "values should not be empty"
    );

    // 人为把 pos 推到越界，模拟非法状态 1 > 0。
    stack.pos = 1;
    assert_eq!(
        stack
            .PickValue(vec![AnyValue::new(1)])
            .unwrap_err()
            .to_string(),
        "illegal state 1 > 0"
    );
}

/// ValuesText 对字符串加引号、元素间空格分隔，格式对齐 Go 的 `%v` 风格诊断串。
#[test]
fn values_text_matches_go_string_quoting_and_spacing() {
    let mut stack = newPickStack();
    stack
        .PickValue(vec![AnyValue::new("a".to_owned())])
        .unwrap();
    stack.PickValue(vec![AnyValue::new(10)]).unwrap();

    assert_eq!(stack.ValuesText(), "[\"a\" 10]");
}

/// 某一组合 panic 后仍应继续访问其余组合，最终再把失败向上抛出。
#[test]
fn run_test_reports_failure_after_visiting_remaining_combinations() {
    let visits = Cell::new(0);
    // catch_unwind 捕获 RunTest 在全部组合跑完后汇总的 panic。
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        RunTest(|t| {
            let value = Pick(t, vec![1, 2]);
            visits.set(visits.get() + 1);
            if value == 1 {
                panic!("first combination failed");
            }
        });
    }));

    assert!(result.is_err());
    assert_eq!(visits.get(), 2);
}
