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

// 轻量匹配器单元测试：Space / Digit / Number / Char / AnyChar。
//
// 对照 Go `pkg/util/parser` 测试；失败路径保留 Go「不消费输入」的断言意图。

fn bytes(value: &str) -> Vec<u8> {
    value.as_bytes().to_vec()
}

/// space_ok_case 对应 TestSpace 中成功表的匿名 struct。
// space_ok_case 对应 TestSpace 中成功表的匿名 struct。
struct SpaceOkCase {
    /// 要求的最少空白数。
    times: isize,
    /// 输入串。
    input: &'static str,
    /// 成功匹配后的剩余串。
    expected: &'static str,
}

/// space_err_case 对应 TestSpace 中失败表，只需记录最小空格数和原始输入。
// space_err_case 对应 TestSpace 中失败表，只需记录最小空格数和原始输入。
struct SpaceErrCase {
    /// 要求的最少空白数。
    times: isize,
    /// 输入串。
    input: &'static str,
}

/// TestSpace 对应 Go 的 Space 测试：成功时返回剩余字符串，失败时返回原输入和错误。
// TestSpace 对应 Go 的 Space 测试：成功时返回剩余字符串，失败时返回原输入和错误。
#[test]
fn test_space() {
    let ok_table = vec![
        SpaceOkCase {
            times: 0,
            input: " 1",
            expected: "1",
        },
        SpaceOkCase {
            times: 0,
            input: "1",
            expected: "1",
        },
        SpaceOkCase {
            times: 1,
            input: "     1",
            expected: "1",
        },
        SpaceOkCase {
            times: 2,
            input: "  1",
            expected: "1",
        },
    ];
    for test in ok_table {
        let (rest, error) = utilparser::Space(test.input, test.times);
        assert_eq!(None, error);
        assert_eq!(bytes(test.expected), rest);
    }

    let err_table = vec![
        SpaceErrCase {
            times: 1,
            input: "1",
        },
        SpaceErrCase {
            times: 2,
            input: " 1",
        },
    ];
    for test in err_table {
        let (rest, error) = utilparser::Space(test.input, test.times);
        assert_eq!(Some(utilparser::ErrPatternNotMatch), error);
        assert_eq!(bytes(test.input), rest);
    }
}

/// digit_ok_case 对应 TestDigit 的成功用例，保留 digits/rest 双返回值断言。
// digit_ok_case 对应 TestDigit 的成功用例，保留 digits/rest 双返回值断言。
struct DigitOkCase {
    /// 要求的最少数字个数。
    times: isize,
    /// 输入串。
    input: &'static str,
    /// 期望匹配到的数字前缀。
    expected_digits: &'static str,
    /// 期望剩余串。
    expected_rest: &'static str,
}

/// digit_err_case 对应 TestDigit 的失败用例，失败时 digits 为空且 rest 为原输入。
// digit_err_case 对应 TestDigit 的失败用例，失败时 digits 为空且 rest 为原输入。
struct DigitErrCase {
    /// 要求的最少数字个数。
    times: isize,
    /// 输入串。
    input: &'static str,
}

/// TestDigit 对应 Go 的 Digit 测试，验证至少 times 个数字的匹配规则。
// TestDigit 对应 Go 的 Digit 测试，验证至少 times 个数字的匹配规则。
#[test]
fn test_digit() {
    let ok_table = vec![
        DigitOkCase {
            times: 0,
            input: "123abc",
            expected_digits: "123",
            expected_rest: "abc",
        },
        DigitOkCase {
            times: 1,
            input: "123abc",
            expected_digits: "123",
            expected_rest: "abc",
        },
        DigitOkCase {
            times: 2,
            input: "123 @)@)",
            expected_digits: "123",
            expected_rest: " @)@)",
        },
        DigitOkCase {
            times: 3,
            input: "456 121",
            expected_digits: "456",
            expected_rest: " 121",
        },
    ];
    for test in ok_table {
        let (digits, rest, error) = utilparser::Digit(test.input, test.times);
        assert_eq!(None, error);
        assert_eq!(bytes(test.expected_digits), digits);
        assert_eq!(bytes(test.expected_rest), rest);
    }

    let err_table = vec![
        DigitErrCase {
            times: 1,
            input: "int",
        },
        DigitErrCase {
            times: 2,
            input: "1int",
        },
        DigitErrCase {
            times: 3,
            input: "12 int",
        },
    ];
    for test in err_table {
        let (digits, rest, error) = utilparser::Digit(test.input, test.times);
        assert_eq!(Some(utilparser::ErrPatternNotMatch), error);
        assert_eq!(Vec::<u8>::new(), digits);
        assert_eq!(bytes(test.input), rest);
    }
}

/// number_ok_case 对应 TestNumber 的成功用例，数字前缀会转换为 int。
// number_ok_case 对应 TestNumber 的成功用例，数字前缀会转换为 int。
struct NumberOkCase {
    /// 输入串。
    input: &'static str,
    /// 期望解析出的整数。
    expected_num: isize,
    /// 期望剩余串。
    expected_rest: &'static str,
}

/// number_err_case 对应 TestNumber 的失败用例。
// number_err_case 对应 TestNumber 的失败用例。
struct NumberErrCase {
    /// 输入串。
    input: &'static str,
}

/// TestNumber 对应 Go 的 Number 测试，覆盖数字前缀解析和非数字输入失败路径。
// TestNumber 对应 Go 的 Number 测试，覆盖数字前缀解析和非数字输入失败路径。
#[test]
fn test_number() {
    let ok_table = vec![
        NumberOkCase {
            input: "123abc",
            expected_num: 123,
            expected_rest: "abc",
        },
        NumberOkCase {
            input: "123abc",
            expected_num: 123,
            expected_rest: "abc",
        },
        NumberOkCase {
            input: "123 @)@)",
            expected_num: 123,
            expected_rest: " @)@)",
        },
        NumberOkCase {
            input: "456 121",
            expected_num: 456,
            expected_rest: " 121",
        },
    ];
    for test in ok_table {
        let (number, rest, error) = utilparser::Number(test.input);
        assert_eq!(None, error);
        assert_eq!(test.expected_num, number);
        assert_eq!(bytes(test.expected_rest), rest);
    }

    let err_table = vec![
        NumberErrCase { input: "int" },
        NumberErrCase { input: "abcint" },
        NumberErrCase { input: "@)@)int" },
    ];
    for test in err_table {
        let (number, rest, error) = utilparser::Number(test.input);
        assert_eq!(Some(utilparser::ErrPatternNotMatch), error);
        assert_eq!(0, number);
        assert_eq!(bytes(test.input), rest);
    }
}

/// char_ok_case 对应 TestCharAndAnyChar 的成功表。
// char_ok_case 对应 TestCharAndAnyChar 的成功表。
struct CharOkCase {
    /// Char 期望匹配的字节。
    ch: u8,
    /// 输入串。
    input: &'static str,
    /// 期望剩余串。
    expected: &'static str,
}

/// char_err_case 对应 TestCharAndAnyChar 的失败表。
// char_err_case 对应 TestCharAndAnyChar 的失败表。
struct CharErrCase {
    /// Char 期望匹配的字节。
    ch: u8,
    /// 输入串。
    input: &'static str,
}

/// TestCharAndAnyChar 对应 Go 的 Char 与 AnyChar 组合测试。
// TestCharAndAnyChar 对应 Go 的 Char 与 AnyChar 组合测试。
#[test]
fn test_char_and_any_char() {
    let ok_table = vec![
        CharOkCase {
            ch: b'i',
            input: "int",
            expected: "nt",
        },
        CharOkCase {
            ch: b'1',
            input: "1int",
            expected: "int",
        },
        CharOkCase {
            ch: b'1',
            input: "12 int",
            expected: "2 int",
        },
    ];
    for test in ok_table {
        let (rest, error) = utilparser::Char(test.input, test.ch);
        assert_eq!(None, error);
        assert_eq!(bytes(test.expected), rest);

        let (rest, error) = utilparser::AnyChar(test.input);
        assert_eq!(None, error);
        assert_eq!(bytes(test.expected), rest);
    }

    let err_table = vec![
        CharErrCase {
            ch: b'i',
            input: "xint",
        },
        CharErrCase {
            ch: b'1',
            input: "x1int",
        },
        CharErrCase {
            ch: b'1',
            input: "x12 int",
        },
    ];
    for test in err_table {
        let (rest, error) = utilparser::Char(test.input, test.ch);
        assert_eq!(Some(utilparser::ErrPatternNotMatch), error);
        assert_eq!(bytes(test.input), rest);
    }
}

#[test]
fn any_char_consumes_the_first_go_string_byte() {
    let (rest, error) = utilparser::AnyChar("é");
    assert_eq!(None, error);
    assert_eq!("é".as_bytes()[1..], rest);
}
