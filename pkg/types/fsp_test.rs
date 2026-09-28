// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// FSP（小数秒精度）相关单元测试。
//
// 对照 Go `pkg/types/fsp_test.go`，覆盖 `CheckFsp` / `ParseFrac` /
// `alignFrac` 的边界与舍入进位行为。

// 对照 pkg/types/fsp_test.go，覆盖 FSP 检查、解析和小数补齐。
//

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_variables
)]

use crate::field::errors;

// 直接编译生产实现，使测试能覆盖包内私有 alignFrac，不复制生产逻辑。
include!("fsp.rs");

/// 校验未指定、过小、过大及合法中间值的 FSP 规范化结果。
#[test]
fn TestCheckFsp() {
    let cases = [
        (i64::from(UnspecifiedFsp), DefaultFsp, None),
        (-2019, DefaultFsp, Some("Invalid fsp -2019".to_owned())),
        (
            i64::from(MinFsp) - 4_294_967_296,
            DefaultFsp,
            Some(format!("Invalid fsp {}", i64::from(MinFsp) - 4_294_967_296)),
        ),
        (-1, DefaultFsp, None),
        (i64::from(MaxFsp) + 1, MaxFsp, None),
        (i64::from(MaxFsp) + 2019, MaxFsp, None),
        (i64::from(MaxFsp) + 4_294_967_296, MaxFsp, None),
        (
            i64::from((MaxFsp + MinFsp) / 2),
            (MaxFsp + MinFsp) / 2,
            None,
        ),
        (5, 5, None),
    ];
    for (input, expected, expected_error) in cases {
        let (obtained, error) = CheckFsp(input);
        assert_eq!(expected, obtained, "input {input}");
        assert_eq!(
            expected_error,
            error.map(|error| error.to_string()),
            "input {input}"
        );
    }
}

/// 空串、非法 FSP、非数字及多组舍入/进位用例。
#[test]
fn TestParseFrac() {
    let (obtained, overflow, error) = ParseFrac("", 5);
    assert_eq!(0, obtained);
    assert!(!overflow);
    assert!(error.is_none());

    let (obtained, overflow, error) = ParseFrac("999", 200_u8 as i8 as i32);
    assert_eq!((0, false), (obtained, overflow));
    assert!(error.unwrap().to_string().starts_with("Invalid fsp "));

    let (obtained, overflow, error) = ParseFrac("NotNum", MaxFsp);
    assert_eq!((0, false), (obtained, overflow));
    assert!(error.unwrap().to_string().starts_with("strconv.ParseInt:"));

    // Go slices the input by bytes and reports an integer parsing error even
    // when the requested prefix splits a UTF-8 code point; Rust must not panic.
    let (obtained, overflow, error) = ParseFrac("é9", 0);
    assert_eq!((0, false), (obtained, overflow));
    assert!(error.unwrap().to_string().starts_with("strconv.ParseInt:"));

    for (input, fsp, expected, expected_overflow) in [
        ("1235", 6, 123_500, false),
        ("123456", 4, 123_500, false),
        ("1234567", 6, 123_457, false),
        ("1234567", 4, 123_500, false),
        ("1236", 3, 124_000, false),
        ("0312", 2, 30_000, false),
        ("999", 2, 0, true),
    ] {
        let (obtained, overflow, error) = ParseFrac(input, fsp);
        assert_eq!(expected, obtained, "input {input}");
        assert_eq!(expected_overflow, overflow, "input {input}");
        assert!(error.is_none(), "input {input}: {error:?}");
    }
}

/// 正负小数串按 fsp 右侧补零，已足够长则原样返回。
#[test]
fn TestAlignFrac() {
    assert_eq!("100000", alignFrac("100", 6));
    assert_eq!("10000000000", alignFrac("10000000000", 6));
    assert_eq!("-100000", alignFrac("-100", 6));
    assert_eq!("-10000000000", alignFrac("-10000000000", 6));
}
