// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// ENUM 名称与序号解析的单元测试。
//
// 覆盖默认/unicode/general_ci 校对下的匹配、中文成员、
// 数字字符串回退，以及 ParseEnumValue 边界。

use crate::metadata::{ParseEnum, ParseEnumValue, mysql};

#[test]
/// 多校对规则下 ParseEnum/ParseEnumValue 的成功与失败用例。
fn test_enum() {
    let name_cases = [
        (vec!["a", "b"], "a", 1),
        (vec!["a"], "b", 0),
        (vec!["a"], "1", 1),
    ];
    // 默认与 unicode_ci：按名匹配或数字回退；expected=0 表示应失败
    for collation in [mysql::DefaultCollationName, "utf8_unicode_ci"] {
        for (elements, name, expected) in &name_cases {
            let elements: Vec<String> = elements.iter().map(|value| (*value).to_owned()).collect();
            let result = ParseEnum(&elements, name, collation);
            if *expected == 0 {
                assert!(result.is_err());
            } else {
                let value = result.unwrap();
                assert_eq!(value.String(), elements[*expected as usize - 1]);
                assert_eq!(value.ToNumber(), *expected as f64);
            }
        }
    }

    // general_ci：大小写/尾空格不敏感，并覆盖中文成员名
    for (elements, name, expected) in [
        (vec!["a", "b"], "A     ", 1),
        (vec!["a"], "A", 1),
        (vec!["a"], "b", 0),
        (vec!["啊"], "啊", 1),
        (vec!["a"], "1", 1),
    ] {
        let elements: Vec<String> = elements.into_iter().map(str::to_owned).collect();
        let result = ParseEnum(&elements, name, "utf8_general_ci");
        if expected == 0 {
            assert!(result.is_err());
        } else {
            let value = result.unwrap();
            assert_eq!(value.String(), elements[expected as usize - 1]);
            assert_eq!(value.ToNumber(), expected as f64);
        }
    }

    let elements = vec!["a".to_owned()];
    assert_eq!(ParseEnumValue(&elements, 1).unwrap().ToNumber(), 1.0);
    assert!(ParseEnumValue(&elements, 0).is_err());

    // Go's strconv.ParseUint with base 0 accepts an underscore immediately
    // after a base prefix, following Go integer-literal syntax.
    let elements = vec!["a".to_owned(), "b".to_owned()];
    assert_eq!(
        ParseEnum(&elements, "0x_2", mysql::DefaultCollationName)
            .unwrap()
            .ToNumber(),
        2.0
    );
    assert_eq!(
        ParseEnum(&elements, "0_2", mysql::DefaultCollationName)
            .unwrap()
            .ToNumber(),
        2.0
    );
    assert!(ParseEnum(&elements, "+1", mysql::DefaultCollationName).is_err());
}
