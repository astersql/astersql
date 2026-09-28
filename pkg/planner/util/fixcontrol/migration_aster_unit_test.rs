// Copyright 2023 PingCAP, Inc.
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

// Aster 迁移期 fix-control 语义对齐单测。
//
// 覆盖 Get* 在缺失键 / 布尔 ON·1 / 非法数字回退，以及 ParseToMap
// 的引号、重复键警告与语法错误边界，确保与 Go 行为一致。

use super::{
    GetBool, GetBoolWithDefault, GetFloat, GetFloatWithDefault, GetInt, GetIntWithDefault, GetStr,
    GetStrWithDefault, ParseToMap,
};
use std::collections::HashMap;

/// 校验 Get* 对 None map、布尔语义、极值整数与解析失败默认值。
#[test]
fn getters_match_go_missing_and_conversion_semantics() {
    // map 为 None：全部返回「不存在」且无解析错误。
    assert_eq!((String::new(), false), GetStr(None, 1));
    assert_eq!((false, false), GetBool(None, 1));
    assert_eq!((0, false, Ok(())), GetInt(None, 1));
    assert_eq!((0.0, false, Ok(())), GetFloat(None, 1));

    let values = HashMap::from([
        (1, "oN".to_owned()),
        (2, "1".to_owned()),
        (3, "true".to_owned()),
        (4, "-9223372036854775808".to_owned()),
        (5, "6.25".to_owned()),
        (6, "not-a-number".to_owned()),
    ]);

    // ON（忽略大小写）与 "1" 为 true；"true" 字符串本身不为 true。
    assert_eq!((true, true), GetBool(Some(&values), 1));
    assert_eq!(("oN".to_owned(), true), GetStr(&values, 1));
    assert_eq!((true, true), GetBool(Some(&values), 2));
    assert_eq!((false, true), GetBool(Some(&values), 3));
    assert_eq!(
        (-9_223_372_036_854_775_808, true, Ok(())),
        GetInt(Some(&values), 4)
    );
    assert_eq!((6.25, true, Ok(())), GetFloat(Some(&values), 5));
    assert!(GetInt(Some(&values), 6).2.is_err());
    assert!(GetFloat(Some(&values), 6).2.is_err());

    // 缺失键或解析失败时走 WithDefault 回退。
    assert_eq!("fallback", GetStrWithDefault(&values, 99, "fallback"));
    assert!(GetBoolWithDefault(Some(&values), 99, true));
    assert_eq!(42, GetIntWithDefault(Some(&values), 6, 42));
    assert_eq!(4.5, GetFloatWithDefault(Some(&values), 6, 4.5));
}

/// 校验引号内逗号、重复键覆盖并告警、空值与空白键值。
#[test]
fn parse_to_map_matches_go_quotes_duplicates_and_empty_values() {
    let (values, warnings) =
        ParseToMap(" 52592: ON, 44823:'12,34', 52592:off, 7:\"spaced value\", 8:")
            .expect("valid fix-control input");

    // 重复 52592 后以新值 off 覆盖，并产生一条警告。
    assert_eq!(Some("off"), values.get(&52592).map(String::as_str));
    assert_eq!(Some("12,34"), values.get(&44823).map(String::as_str));
    assert_eq!(Some("spaced value"), values.get(&7).map(String::as_str));
    assert_eq!(Some(""), values.get(&8).map(String::as_str));
    assert_eq!(
        vec![
            "repeated assignment for fix control: 52592. existing value: \"ON\". new value: \"off\"."
                .to_owned()
        ],
        warnings
    );
}

/// 校验缺冒号、非法键、未闭合引号报错，以及空值合法。
#[test]
fn parse_to_map_matches_go_error_boundaries() {
    assert!(ParseToMap("123").is_err());
    assert!(ParseToMap("not-a-number:value").is_err());
    assert!(ParseToMap("123:'unterminated").is_err());

    let (values, warnings) = ParseToMap("123:").expect("Go accepts an empty value");
    assert!(warnings.is_empty());
    assert_eq!(Some(""), values.get(&123).map(String::as_str));
}
