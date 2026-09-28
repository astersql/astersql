// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// fix-control 解析与类型化读取的单元测试。
//
// fix-control 是优化器按 issue 编号开关行为的会话变量；此处覆盖空值解析
// 以及字符串 / 布尔 / 整数 / 浮点回放语义，与 Go 侧行为对齐。

use super::*;
use std::collections::HashMap;

#[derive(Debug, PartialEq)]
struct resultForSingleFix {
    ValueInMap: String,
    GetStr: String,
    GetBool: bool,
    GetInt: i64,
    GetFloat: f64,
}

impl resultForSingleFix {
    fn expected(value: &str, GetBool: bool, GetInt: i64, GetFloat: f64) -> Self {
        Self {
            ValueInMap: value.to_owned(),
            GetStr: value.to_owned(),
            GetBool,
            GetInt,
            GetFloat,
        }
    }
}

fn getTestResultForSingleFix(fixControlMap: &HashMap<u64, String>, key: u64) -> resultForSingleFix {
    resultForSingleFix {
        ValueInMap: fixControlMap.get(&key).cloned().unwrap_or_default(),
        GetStr: GetStrWithDefault(fixControlMap, key, "default"),
        GetBool: GetBoolWithDefault(fixControlMap, key, false),
        GetInt: GetIntWithDefault(fixControlMap, key, 12345),
        GetFloat: GetFloatWithDefault(fixControlMap, key, 1234.5),
    }
}

/// 值为空（`123:`）时应解析为空字符串且无警告。
#[test]
fn TestParseToMapEmptyValue() {
    let (values, warnings) = ParseToMap("123:").expect("empty values are valid");
    assert!(warnings.is_empty());
    assert_eq!(Some(""), values.get(&123).map(String::as_str));
}

/// 逐项回放 Go `fix_control_suite` 的成功场景、警告和类型化读取结果。
#[test]
fn TestFixControl() {
    let successfulCases = vec![
        (
            "1000:'on', 10000:1",
            HashMap::from([
                (1000, resultForSingleFix::expected("on", true, 12345, 1234.5)),
                (10000, resultForSingleFix::expected("1", true, 1, 1.0)),
            ]),
            Vec::<String>::new(),
        ),
        (
            "100:'on', 100:1",
            HashMap::from([(100, resultForSingleFix::expected("1", true, 1, 1.0))]),
            vec![
                "repeated assignment for fix control: 100. existing value: \"on\". new value: \"1\"."
                    .to_owned(),
            ],
        ),
        (
            "100:2,100:2",
            HashMap::from([(100, resultForSingleFix::expected("2", false, 2, 2.0))]),
            Vec::new(),
        ),
        (
            "  111 : 'test1'  ,",
            HashMap::from([(
                111,
                resultForSingleFix::expected("test1", false, 12345, 1234.5),
            )]),
            Vec::new(),
        ),
        (
            "  4321 : '55.5'  , ",
            HashMap::from([(
                4321,
                resultForSingleFix::expected("55.5", false, 12345, 55.5),
            )]),
            Vec::new(),
        ),
        (
            "  4321 : '55.5'  ",
            HashMap::from([(
                4321,
                resultForSingleFix::expected("55.5", false, 12345, 55.5),
            )]),
            Vec::new(),
        ),
        (
            "  5000 : 55.5  , ",
            HashMap::from([(
                5000,
                resultForSingleFix::expected("55.5", false, 12345, 55.5),
            )]),
            Vec::new(),
        ),
        (
            "  5000 : 55.5  , 2000: '-10',100:5000 ,",
            HashMap::from([
                (
                    5000,
                    resultForSingleFix::expected("55.5", false, 12345, 55.5),
                ),
                (2000, resultForSingleFix::expected("-10", false, -10, -10.0)),
                (100, resultForSingleFix::expected("5000", false, 5000, 5000.0)),
            ]),
            Vec::new(),
        ),
        (
            "  2000 : 'test1'  ,",
            HashMap::from([(
                2000,
                resultForSingleFix::expected("test1", false, 12345, 1234.5),
            )]),
            Vec::new(),
        ),
    ];

    for (input, expectedFixControl, expectedWarnings) in successfulCases {
        let (fixControlMap, warnings) = ParseToMap(input).expect("Go fixture expects success");
        assert_eq!(expectedWarnings, warnings, "warnings for {input:?}");
        assert_eq!(
            expectedFixControl.len(),
            fixControlMap.len(),
            "map size for {input:?}"
        );
        for (key, expected) in expectedFixControl {
            assert_eq!(
                expected,
                getTestResultForSingleFix(&fixControlMap, key),
                "typed result for key {key} in {input:?}"
            );
        }
    }

    // Go 测试每个 SQL case 前会清空会话 map；独立解析同样不得泄漏前一 case 状态。
    let (isolated, _) = ParseToMap("9:ON").expect("first isolated case");
    assert!(isolated.contains_key(&9));
    let (isolated, _) = ParseToMap("10:OFF").expect("second isolated case");
    assert!(!isolated.contains_key(&9));
    assert_eq!(Some("OFF"), isolated.get(&10).map(String::as_str));
}

/// 回放 Go 数据集中的全部失败路径；Rust 以同类解析错误表达 Go `ParseUint` 失败。
#[test]
fn TestFixControlErrors() {
    for input in ["  1.5 : 'test1'", "-1: 'test1' "] {
        let err = ParseToMap(input).expect_err("non-u64 keys must fail");
        assert!(
            err.downcast_ref::<std::num::ParseIntError>().is_some(),
            "expected unsigned-integer parse error for {input:?}, got {err:#}"
        );
    }

    for (input, expectedError) in [
        (
            "  2000 : 'test1",
            "invalid fix control: expected quote not found",
        ),
        ("100, 100", "invalid fix control: expected colon not found"),
    ] {
        let err = ParseToMap(input).expect_err("invalid Go fixture must fail");
        assert_eq!(expectedError, err.to_string(), "error for {input:?}");
    }
}
