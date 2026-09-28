// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 字符集 / 排序规则格的 Compare 与 Join 单测，对齐 Go 同名用例。

use super::*;

/// 从格元素 `Unwrap` 出字符串（charset/collation 的规范表示）。
fn unwrap_string(value: &dyn Lattice) -> String {
    value
        .Unwrap()
        .downcast_ref::<String>()
        .expect("charset/collation Unwrap must return String")
        .clone()
}

/// 断言比较失败且错误信息包含给定子串。
fn assert_error_contains(result: Result<i32, IncompatibleError>, expected: &str) {
    let err = result.expect_err("comparison should fail");
    assert!(
        err.to_string().contains(expected),
        "expected error to contain {expected}, got {err}"
    );
}

// TestCharsetCompare 对应 Go 的同名测试：验证大小写归一、utf8mb3 兼容和不可比较错误。
/// 字符集比较：大小写归一、utf8mb3↔utf8、utf8mb4 超集与不相容错误。
#[test]
fn test_charset_compare() {
    assert_eq!(Charset("UTF8").Compare(&Charset("utf8")).unwrap(), 0);
    assert_eq!(Charset("UTF8MB3").Compare(&Charset("utf8")).unwrap(), 0);
    assert_error_contains(
        Charset("uTF8").Compare(&Charset("GBK")),
        "incompatible charset (utf8 vs gbk)",
    );
    assert_eq!(Charset("latin1").Compare(&Charset("utf8mb4")).unwrap(), -1);
    assert_eq!(Charset("utf8mb4").Compare(&Charset("utf8mb3")).unwrap(), 1);
    assert_error_contains(
        Charset("other1").Compare(&Charset("other2")),
        "incompatible charset (other1 vs other2)",
    );
    assert_error_contains(
        Charset("other1").Compare(&Charset("utf8")),
        "incompatible charset (other1 vs utf8)",
    );
}

// TestCollationCompare 对应 Go 的 collation 比较测试。
/// 排序规则比较：后缀必须一致，字符集偏序与 charset 格一致。
#[test]
fn test_collation_compare() {
    assert_eq!(
        Collation("UTF8_BIN")
            .Compare(&Collation("utf8_bin"))
            .unwrap(),
        0
    );
    assert_eq!(
        Collation("binary").Compare(&Collation("BINARY")).unwrap(),
        0
    );
    assert_error_contains(
        Collation("binary").Compare(&Collation("utf8mb4_bin")),
        "incompatible collation (binary vs utf8mb4_bin)",
    );
    assert_eq!(
        Collation("UTF8MB3_BIN")
            .Compare(&Collation("utf8_bin"))
            .unwrap(),
        0
    );
    assert_eq!(
        Collation("LATIN1_BIN")
            .Compare(&Collation("utf8mb4_bin"))
            .unwrap(),
        -1
    );
    assert_error_contains(
        Collation("UTF8_BIN").Compare(&Collation("GBK_BIN")),
        "incompatible charset (utf8 vs gbk)",
    );
    assert_eq!(
        Collation("utf8mb4_general_ci")
            .Compare(&Collation("utf8_general_ci"))
            .unwrap(),
        1
    );
    assert_error_contains(
        Collation("utf8mb4_general_ci").Compare(&Collation("utf8mb4_0900_ai_ci")),
        "incompatible collation (utf8mb4_general_ci vs utf8mb4_0900_ai_ci)",
    );
    assert_error_contains(
        Collation("other_cs_bin").Compare(&Collation("other_cs_ci")),
        "incompatible collation (other_cs_bin vs other_cs_ci)",
    );
    assert_error_contains(
        Collation("unknowCS").Compare(&Collation("unknowCS2")),
        "incompatible charset (unknowcs vs unknowcs2)",
    );
    assert_eq!(
        Collation("unknowCS")
            .Compare(&Collation("unknowCS"))
            .unwrap(),
        0
    );
}

// TestCharsetJoin 对应 Go 的字符集合并测试。
/// 字符集 Join：utf8↔latin1 上确界为 utf8mb4。
#[test]
fn test_charset_join() {
    assert_eq!(
        unwrap_string(Charset("utf8").Join(&Charset("latin1")).unwrap().as_ref()),
        "utf8mb4"
    );
    assert_eq!(
        unwrap_string(
            Charset("latin1")
                .Join(&Charset("utf8mb3"))
                .unwrap()
                .as_ref()
        ),
        "utf8mb4"
    );
}

// TestCollationJoin 对应 Go 的排序规则合并测试。
/// 排序规则 Join：保留相同后缀，字符集提升到共同上界。
#[test]
fn test_collation_join() {
    assert_eq!(
        unwrap_string(
            Collation("utf8_bin")
                .Join(&Collation("latin1_bin"))
                .unwrap()
                .as_ref()
        ),
        "utf8mb4_bin"
    );
    assert_eq!(
        unwrap_string(
            Collation("latin1_general_cs")
                .Join(&Collation("utf8_general_cs"))
                .unwrap()
                .as_ref()
        ),
        "utf8mb4_general_cs"
    );
}
