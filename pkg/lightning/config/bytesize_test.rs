// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// `ByteSize` 编解码单元测试。
//
// 覆盖 TOML 文本容量解析（合法后缀、负值、非法类型）以及 TOML/JSON 编码为纯数字。

use regex::Regex;
use std::str::FromStr;

use crate::ByteSize;

/// 单条 TOML 解码用例：输入片段、期望值或错误正则。
struct ByteSizeDecodeCase {
    input: &'static str,
    output: ByteSize,
    err: &'static str,
}

/// 仅含一个 `ByteSize` 字段的包装结构，便于 toml/json 往返。
#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct Wrapper {
    x: ByteSize,
}

// test_byte_size_toml_decode 对应 Go 的 TestByteSizeTOMLDecode。
/// 验证 TOML 中整数、下划线数字、带后缀字符串、浮点与非法类型的解码行为。
#[test]
fn test_byte_size_toml_decode() {
    let test_cases = [
        ByteSizeDecodeCase {
            input: "x = 10000",
            output: ByteSize(10000),
            err: "",
        },
        ByteSizeDecodeCase {
            input: "x = 107_374_182_400",
            output: ByteSize(107_374_182_400),
            err: "",
        },
        ByteSizeDecodeCase {
            input: "x = '10k'",
            output: ByteSize(10 * 1024),
            err: "",
        },
        ByteSizeDecodeCase {
            input: "x = '10PiB'",
            output: ByteSize(10 * 1024 * 1024 * 1024 * 1024 * 1024),
            err: "",
        },
        ByteSizeDecodeCase {
            input: "x = '10 KB'",
            output: ByteSize(10 * 1024),
            err: "",
        },
        ByteSizeDecodeCase {
            input: "x = '32768'",
            output: ByteSize(32768),
            err: "",
        },
        ByteSizeDecodeCase {
            input: "x = -1",
            output: ByteSize(0),
            err: "invalid size: '-1'",
        },
        ByteSizeDecodeCase {
            input: "x = 'invalid value'",
            output: ByteSize(0),
            err: r#"strconv.ParseFloat: parsing "invalid": invalid syntax"#,
        },
        ByteSizeDecodeCase {
            input: "x = true",
            output: ByteSize(0),
            err: "invalid size: 'true'",
        },
        ByteSizeDecodeCase {
            input: "x = 256.0",
            output: ByteSize(256),
            err: "",
        },
        ByteSizeDecodeCase {
            input: "x = 256.9",
            output: ByteSize(256),
            err: "",
        },
        ByteSizeDecodeCase {
            input: "x = 10e+9",
            output: ByteSize(10_000_000_000),
            err: "",
        },
        ByteSizeDecodeCase {
            input: "x = '2.5MB'",
            output: ByteSize(5 * 512 * 1024),
            err: "",
        },
        ByteSizeDecodeCase {
            input: "x = 2020-01-01T00:00:00Z",
            output: ByteSize(0),
            err: r#"strconv.ParseFloat: parsing "2020-01-01T00:00:00": invalid syntax"#,
        },
        ByteSizeDecodeCase {
            input: "x = ['100000']",
            output: ByteSize(0),
            err: r"(?i)toml:.*incompatible types",
        },
        ByteSizeDecodeCase {
            input: "x = { size = '100000' }",
            output: ByteSize(0),
            err: r"(?i)toml:.*incompatible types|invalid type: map",
        },
    ];

    for tc in test_cases {
        let comment = format!("input: `{}`", tc.input);
        let result = toml::from_str::<Wrapper>(tc.input);
        if !tc.err.is_empty() {
            // 期望失败：错误信息需匹配给定正则
            let err = result.expect_err(&comment);
            let re = Regex::new(tc.err).unwrap_or_else(|_| panic!("bad regexp {}", tc.err));
            assert!(
                re.is_match(&err.to_string()),
                "{comment}: expected /{}/ got {}",
                tc.err,
                err
            );
        } else {
            let output = result.expect(&comment);
            assert_eq!(tc.output, output.x, "{comment}");
        }
    }
}

// test_byte_size_toml_and_json_encode 对应 Go 的 TestByteSizeTOMLAndJSONEncode。
/// 验证编码侧始终输出纯数字，不带容量后缀。
#[test]
fn test_byte_size_toml_and_json_encode() {
    let input = Wrapper {
        x: ByteSize(1_048_576),
    };
    let encoded = toml::to_string(&input).expect("toml encode");
    assert_eq!("x = 1048576\n", encoded);

    let js = serde_json::to_string(&input).expect("json encode");
    assert_eq!(r#"{"x":1048576}"#, js);
}

// docker/go-units RAMInBytes rejects leading/trailing or misplaced spaces
// instead of normalizing all whitespace before parsing.
#[test]
fn test_byte_size_text_whitespace_and_suffix_parity() {
    for input in [" 32 ", "32m b", "32  B"] {
        assert!(ByteSize::from_str(input).is_err(), "input: {input:?}");
    }

    let error = ByteSize::from_str("32bm").expect_err("invalid suffix");
    assert_eq!("invalid suffix: 'bm'", error.to_string());
}
