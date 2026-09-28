// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// split 单元测试：最长公共前缀与切分步长计算。
//
// 对齐 Go 侧空串/前缀/无公共前缀，以及 lower/upper 补 pad 后的 uint64 差值步长。

#![allow(dead_code)]
#![allow(non_snake_case)]

use crate::split::{getStepValue, longestCommonPrefixLen};

/// 最长公共前缀表驱动用例：两字符串与期望公共前缀字节长度。
struct LongestCommonPrefixCase {
    s1: &'static str,
    s2: &'static str,
    l: usize,
}

/// 覆盖空串、单边空、相等、真前缀与首字节不同等前缀长度情形。
#[test]
fn TestLongestCommonPrefixLen() {
    let cases = [
        LongestCommonPrefixCase {
            s1: "",
            s2: "",
            l: 0,
        },
        LongestCommonPrefixCase {
            s1: "",
            s2: "a",
            l: 0,
        },
        LongestCommonPrefixCase {
            s1: "a",
            s2: "",
            l: 0,
        },
        LongestCommonPrefixCase {
            s1: "a",
            s2: "a",
            l: 1,
        },
        LongestCommonPrefixCase {
            s1: "ab",
            s2: "a",
            l: 1,
        },
        LongestCommonPrefixCase {
            s1: "a",
            s2: "ab",
            l: 1,
        },
        LongestCommonPrefixCase {
            s1: "b",
            s2: "ab",
            l: 0,
        },
        LongestCommonPrefixCase {
            s1: "ba",
            s2: "ab",
            l: 0,
        },
    ];

    for ca in cases {
        let re = longestCommonPrefixLen(ca.s1.as_bytes(), ca.s2.as_bytes());
        assert_eq!(ca.l, re);
    }
}

/// 步长表驱动用例：lower/upper、期望公共前缀长与 `num=1` 时的步长。
struct StepValueCase {
    lower: Vec<u8>,
    upper: Vec<u8>,
    l: usize,
    v: u64,
}

/// 先校验公共前缀，再对去前缀后缀以 `num=1` 计算步长，对齐 Go 补 pad 语义。
#[test]
fn TestGetStepValue() {
    let cases = [
        StepValueCase {
            lower: vec![],
            upper: vec![],
            l: 0,
            v: u64::MAX,
        },
        StepValueCase {
            lower: vec![0],
            upper: vec![128],
            l: 0,
            v: u64::from_be_bytes([128, 255, 255, 255, 255, 255, 255, 255]),
        },
        StepValueCase {
            lower: vec![b'a'],
            upper: vec![b'z'],
            l: 0,
            v: u64::from_be_bytes([b'z' - b'a', 255, 255, 255, 255, 255, 255, 255]),
        },
        StepValueCase {
            lower: b"abc".to_vec(),
            upper: vec![b'z'],
            l: 0,
            v: u64::from_be_bytes([
                b'z' - b'a',
                255_u8 - b'b',
                255_u8 - b'c',
                255,
                255,
                255,
                255,
                255,
            ]),
        },
        StepValueCase {
            lower: b"abc".to_vec(),
            upper: b"xyz".to_vec(),
            l: 0,
            v: u64::from_be_bytes([
                b'x' - b'a',
                b'y' - b'b',
                b'z' - b'c',
                255,
                255,
                255,
                255,
                255,
            ]),
        },
        StepValueCase {
            lower: b"abc".to_vec(),
            upper: b"axyz".to_vec(),
            l: 1,
            v: u64::from_be_bytes([b'x' - b'b', b'y' - b'c', b'z', 255, 255, 255, 255, 255]),
        },
        StepValueCase {
            lower: b"abc0123456".to_vec(),
            upper: b"xyz01234".to_vec(),
            l: 0,
            v: u64::from_be_bytes([b'x' - b'a', b'y' - b'b', b'z' - b'c', 0, 0, 0, 0, 0]),
        },
    ];

    for ca in cases {
        let l = longestCommonPrefixLen(&ca.lower, &ca.upper);
        assert_eq!(ca.l, l);
        let v0 = getStepValue(&ca.lower[l..], &ca.upper[l..], 1);
        assert_eq!(v0, ca.v);
    }
}
