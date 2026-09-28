// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 列过滤器单测：匹配表驱动、解析失败消息与 `@file` 导入/递归导入错误。
//
// 对应 Go `column_filter_test.go` 中的 MatchColumns、ParseFailures、Import 等用例。

// 这些测试只描述列过滤规则的解析、匹配和 @file 导入错误语义；

use crate as filter;
use regex::RegexBuilder;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// 将静态字符串切片转为 `Vec<String>`，供 `ParseColumnFilter` 使用。
fn to_strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// 用正则断言错误消息（对应 Go `require.Regexpf`）。
fn assert_regex_match(pattern: &str, actual: &str, context: &str) {
    let regex = RegexBuilder::new(pattern)
        .dot_matches_new_line(true)
        .build()
        .expect("valid assertion regex");
    assert!(
        regex.is_match(actual),
        "{context}: {actual:?} does not match {pattern:?}"
    );
}

/// 创建带纳秒 nonce 的临时目录，避免并行测试冲突。
fn tempdir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("table-filter-column-{nonce}"));
    std::fs::create_dir_all(&path).expect("create temporary directory");
    path
}

/// 表驱动匹配用例：规则参数、待测列名与期望接受结果。
struct MatchColumnsCase {
    args: &'static [&'static str],
    columns: &'static [&'static str],
    accepted: &'static [bool],
}

/// 解析失败用例：非法规则与期望错误消息正则。
struct ParseFailureCase {
    arg: &'static str,
    msg: &'static str,
}

// TestMatchColumns 对应 Go 的列过滤匹配表驱动测试。
/// 覆盖通配、否定、正则、注释、中文、转义与大小写等列匹配语义。
#[test]
fn test_match_columns() {
    let cases = vec![
        MatchColumnsCase {
            args: &[],
            columns: &["foo"],
            accepted: &[false],
        },
        MatchColumnsCase {
            args: &["*"],
            columns: &["foo"],
            accepted: &[true],
        },
        MatchColumnsCase {
            args: &["foo*"],
            columns: &["foo", "foo1", "foo2"],
            accepted: &[true, true, true],
        },
        MatchColumnsCase {
            args: &["*", "!foo1*"],
            columns: &["foo", "foo1", "foo2"],
            accepted: &[true, false, true],
        },
        MatchColumnsCase {
            args: &["/^foo/"],
            columns: &["foo", "foo1", "fff"],
            accepted: &[true, true, false],
        },
        MatchColumnsCase {
            args: &["*", "!foo[bar]", "!bar?", r"!special\\"],
            columns: &[
                "food",
                "foor",
                "foo[bar]",
                "ba",
                "bar?",
                r"special\",
                r"special\\",
                "bazzz",
                r"special\$",
                r"afooa",
            ],
            accepted: &[
                true, false, true, true, false, false, true, true, true, true,
            ],
        },
        MatchColumnsCase {
            args: &["*", "!/a?b?f[0-9]/"],
            columns: &["abbdf1", "aaaaf2", "55", "abbcfa"],
            accepted: &[false, false, true, true],
        },
        MatchColumnsCase {
            args: &["BAR"],
            columns: &["bar", "BAR"],
            accepted: &[true, true],
        },
        MatchColumnsCase {
            args: &["i"],
            columns: &["İ"],
            accepted: &[true],
        },
        MatchColumnsCase {
            args: &["# comment", "x", "   \t"],
            columns: &["x", "y"],
            accepted: &[true, false],
        },
        MatchColumnsCase {
            args: &["p_123$", "中文"],
            columns: &["p_123", "p_123$", "英文", "中文"],
            accepted: &[false, true, false, true],
        },
        MatchColumnsCase {
            args: &[r"\\\."],
            columns: &[r"\.", r"\\\.", r"\a"],
            accepted: &[true, false, false],
        },
        MatchColumnsCase {
            args: &["[!a-z]"],
            columns: &["!", "a", "1"],
            accepted: &[true, false, true],
        },
        MatchColumnsCase {
            args: &[r#""some ""quoted""""#],
            columns: &[
                r#"some "quoted""#,
                r#"some ""quoted"""#,
                r#"SOME "QUOTED""#,
                "some\t\"quoted\"",
            ],
            accepted: &[true, false, true, false],
        },
        MatchColumnsCase {
            args: &["db*", "!cfg*", "cfgsample", r"a\.b\.c"],
            columns: &["irrelevant", "db1", "cfg1", "cfgsample", "a.b.c"],
            accepted: &[false, true, false, true, true],
        },
        MatchColumnsCase {
            args: &["*", "!D[!a-d]"],
            columns: &["S", "Da", "Db", "Daa", "dD", "de"],
            accepted: &[true, true, true, true, true, false],
        },
        MatchColumnsCase {
            args: &[r"?\.?"],
            columns: &["a", "a.b", "中文.英文", "我.你"],
            accepted: &[false, true, false, true],
        },
        MatchColumnsCase {
            args: &["*", r"!?\.?"],
            columns: &["a", ".b", ".英文", "我.你", "我.你.他"],
            accepted: &[true, true, true, false, true],
        },
    ];

    for tc in cases {
        // Go 在每组 case 中调用 filter.ParseColumnFilter(tc.args)，并要求没有解析错误。
        let column_filter =
            filter::ParseColumnFilter(to_strings(tc.args)).expect("parse column filter");
        for (i, column) in tc.columns.iter().enumerate() {
            // 关键断言：列名大小写不敏感，且后写规则优先；期望值完全保留 Go 表驱动数据。
            assert_eq!(
                tc.accepted[i],
                column_filter.MatchColumn(column),
                "col {}",
                column
            );
        }
    }
}

// TestParseFailures 对应 Go 中列规则解析失败的错误消息匹配。
/// 非法规则应解析失败，且错误消息匹配 Go 侧正则期望。
#[test]
fn test_parse_failures() {
    let cases = vec![
        ParseFailureCase {
            arg: "/^t[0-9]+((?!_copy))*$/",
            msg: ".*: invalid pattern: error parsing regexp:.*",
        },
        ParseFailureCase {
            arg: r"a%b\.c",
            msg: ".*: unexpected special character '%'",
        },
        ParseFailureCase {
            arg: r"a\tb\.c",
            msg: r".*: cannot escape a letter or number \(\\t\), it is reserved for future extension",
        },
        ParseFailureCase {
            arg: r"[]\.*",
            msg: ".*: syntax error: failed to parse character class",
        },
        ParseFailureCase {
            arg: r"[!]\.*",
            msg: r".*: invalid pattern: error parsing regexp: missing closing \]:.*",
        },
        ParseFailureCase {
            arg: r"[.*",
            msg: r".*: syntax error: failed to parse character class",
        },
        ParseFailureCase {
            arg: r"[\d\D].*",
            msg: r".*: syntax error: failed to parse character class",
        },
        ParseFailureCase {
            arg: "db.",
            msg: ".*: unexpected special character '.'",
        },
        ParseFailureCase {
            arg: r"/db\.*",
            msg: r".*: syntax error: incomplete regexp",
        },
        ParseFailureCase {
            arg: r"`db\.*",
            msg: r".*: syntax error: incomplete quoted identifier",
        },
        ParseFailureCase {
            arg: r#""db\.*"#,
            msg: r".*: syntax error: incomplete quoted identifier",
        },
        ParseFailureCase {
            arg: r"db\",
            msg: r".*: syntax error: cannot place \\ at end of line",
        },
        ParseFailureCase {
            arg: r"db\.tbl#not comment",
            msg: r".*: unexpected special character '#'",
        },
    ];

    for tc in cases {
        let err = filter::ParseColumnFilter(to_strings(&[tc.arg])).expect_err("parse should fail");
        // Go 使用 require.Regexpf 对 err.Error() 做正则匹配；这里保留同一错误断言形状。
        assert_regex_match(tc.msg, err.to_string().as_str(), tc.arg);
    }
}

// TestImport 对应 Go 的 @file 导入测试：两个临时文件和一个命令行规则合并解析。
/// 从两个 `@file` 与一条命令行规则合并解析，校验列匹配结果。
#[test]
fn test_import() {
    let dir = tempdir();
    let path1 = dir.join("1.txt");
    let path2 = dir.join("2.txt");
    std::fs::write(
        &path1,
        "\n\t\t\tcol?tql?\n\t\t\tcol?\\.tql?\n\t\t\tcol02\\.tql02\n\t\t",
    )
    .expect("write path1");
    std::fs::write(&path2, "\n\t\t\tcol03\\.tql03\n\t\t\t!col4\\.tql4\n\t\t").expect("write path2");

    // 关键迁移点：@file 规则来自真实临时文件 IO；保留路径拼接和导入顺序。
    let f = filter::ParseColumnFilter(vec![
        format!("@{}", path1.display()),
        format!("@{}", path2.display()),
        r"col04\.tql04".to_string(),
    ])
    .expect("parse imported column filter");

    assert!(f.MatchColumn("col1tql1"));
    assert!(f.MatchColumn("col2.tql2"));
    assert!(f.MatchColumn("col3.tql3"));
    assert!(!f.MatchColumn("col4.tql4"));
    assert!(!f.MatchColumn("col01tql01"));
    assert!(!f.MatchColumn("col01.tql01"));
    assert!(f.MatchColumn("col02.tql02"));
    assert!(f.MatchColumn("col03.tql03"));
    assert!(f.MatchColumn("col04.tql04"));
}

// TestRecursiveImport 对应 Go 的递归 @file 禁止和缺失文件错误。
/// 递归 `@file` 与缺失文件均应失败，错误消息与 Go 一致。
#[test]
fn test_recursive_import() {
    let dir = tempdir();
    let path3 = dir.join("3.txt");
    let path4 = dir.join("4.txt");
    std::fs::write(&path3, "col1").expect("write path3");
    std::fs::write(&path4, format!("# comment\n\n@{}", path3.display())).expect("write path4");

    let err = filter::ParseColumnFilter(vec![format!("@{}", path4.display())])
        .expect_err("recursive import should fail");
    assert_regex_match(
        r".*4\.txt:3: importing filter files recursively is not allowed",
        err.to_string().as_str(),
        "recursive import",
    );

    let missing = dir.join("5.txt");
    let err = filter::ParseColumnFilter(vec![format!("@{}", missing.display())])
        .expect_err("missing import should fail");
    assert_regex_match(
        r".*: cannot open filter file: open .*5\.txt: .*",
        err.to_string().as_str(),
        "missing import",
    );
}
