// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 表过滤器单测：匹配、解析失败、`@file` 导入与 All。
//
// 对照 Go 表驱动用例，覆盖大小写敏感/不敏感两条路径。

// 这些测试只描述表过滤器的大小写敏感/不敏感匹配、解析失败、@file 导入和 All 过滤器语义；
//

use crate as filter;
use regex::RegexBuilder;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// 测试辅助：`&str` 切片转 `Vec<String>`。
fn to_strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// 用支持 `.` 跨行的正则断言错误文本。
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

/// 创建带纳秒后缀的临时目录，供 `@file` 导入测试使用。
fn tempdir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("table-filter-table-{nonce}"));
    std::fs::create_dir_all(&path).expect("create temporary directory");
    path
}

/// 测试用库表字面量。
struct Table {
    schema: &'static str,
    name: &'static str,
}

/// 表匹配用例：规则参数与 CS/CI 期望结果。
struct MatchTablesCase {
    args: &'static [&'static str],
    tables: &'static [Table],
    accepted_cs: &'static [bool],
    accepted_ci: &'static [bool],
}

/// schema 匹配用例：规则参数与 CS/CI 期望结果。
struct MatchSchemasCase {
    args: &'static [&'static str],
    schemas: &'static [&'static str],
    accepted_cs: &'static [bool],
    accepted_ci: &'static [bool],
}

/// 解析失败用例：非法参数与期望错误正则。
struct ParseFailureCase {
    arg: &'static str,
    msg: &'static str,
}

// TestMatchTables 对应 Go 的表规则匹配表驱动测试。
#[test]
fn test_match_tables() {
    let cases = vec![
        MatchTablesCase {
            args: &[],
            tables: &[Table {
                schema: "foo",
                name: "bar",
            }],
            accepted_cs: &[false],
            accepted_ci: &[false],
        },
        MatchTablesCase {
            args: &["*.*"],
            tables: &[Table {
                schema: "foo",
                name: "bar",
            }],
            accepted_cs: &[true],
            accepted_ci: &[true],
        },
        MatchTablesCase {
            args: &["foo.*"],
            tables: &[
                Table {
                    schema: "foo",
                    name: "bar",
                },
                Table {
                    schema: "foo1",
                    name: "bar",
                },
                Table {
                    schema: "foo2",
                    name: "bar",
                },
            ],
            accepted_cs: &[true, false, false],
            accepted_ci: &[true, false, false],
        },
        MatchTablesCase {
            args: &["*.*", "!foo1.*"],
            tables: &[
                Table {
                    schema: "foo",
                    name: "bar",
                },
                Table {
                    schema: "foo1",
                    name: "bar",
                },
                Table {
                    schema: "foo2",
                    name: "bar",
                },
            ],
            accepted_cs: &[true, false, true],
            accepted_ci: &[true, false, true],
        },
        MatchTablesCase {
            args: &["foo.bar1"],
            tables: &[
                Table {
                    schema: "foo",
                    name: "bar",
                },
                Table {
                    schema: "foo",
                    name: "bar1",
                },
                Table {
                    schema: "fff",
                    name: "bar1",
                },
            ],
            accepted_cs: &[false, true, false],
            accepted_ci: &[false, true, false],
        },
        MatchTablesCase {
            args: &["*.*", "!foo.bar"],
            tables: &[
                Table {
                    schema: "foo",
                    name: "bar",
                },
                Table {
                    schema: "foo",
                    name: "bar1",
                },
                Table {
                    schema: "fff",
                    name: "bar1",
                },
            ],
            accepted_cs: &[false, true, true],
            accepted_ci: &[false, true, true],
        },
        MatchTablesCase {
            args: &["/^foo/.*", r"!/^foo/./^sbtest-\d/"],
            tables: &[
                Table {
                    schema: "foo",
                    name: "sbtest",
                },
                Table {
                    schema: "foo1",
                    name: "sbtest-1",
                },
                Table {
                    schema: "fff",
                    name: "bar",
                },
            ],
            accepted_cs: &[true, false, false],
            accepted_ci: &[true, false, false],
        },
        MatchTablesCase {
            args: &["*.*", "!foo[bar].*", "!bar?.*", r"!special\\.*"],
            tables: &[
                Table {
                    schema: "foor",
                    name: "a",
                },
                Table {
                    schema: "foo[bar]",
                    name: "b",
                },
                Table {
                    schema: "ba",
                    name: "c",
                },
                Table {
                    schema: "bar?",
                    name: "d",
                },
                Table {
                    schema: r"special\",
                    name: "e",
                },
                Table {
                    schema: r"special\\",
                    name: "f",
                },
                Table {
                    schema: "bazzz",
                    name: "g",
                },
                Table {
                    schema: r"special\$",
                    name: "h",
                },
                Table {
                    schema: r"afooa",
                    name: "i",
                },
            ],
            accepted_cs: &[false, true, true, false, false, true, true, true, true],
            accepted_ci: &[false, true, true, false, false, true, true, true, true],
        },
        MatchTablesCase {
            args: &["*.*", "!/^FOO/.*", "!*./FoO$/"],
            tables: &[
                Table {
                    schema: "FOO1",
                    name: "a",
                },
                Table {
                    schema: "foo2",
                    name: "b",
                },
                Table {
                    schema: "BoO3",
                    name: "cFoO",
                },
                Table {
                    schema: "Foo4",
                    name: "dfoo",
                },
                Table {
                    schema: "5",
                    name: "5",
                },
            ],
            accepted_cs: &[false, true, false, true, true],
            accepted_ci: &[false, false, false, false, true],
        },
        MatchTablesCase {
            args: &["*.*", "!a?b?./f[0-9]/"],
            tables: &[
                Table {
                    schema: "abbd",
                    name: "f1",
                },
                Table {
                    schema: "aaaa",
                    name: "f2",
                },
                Table {
                    schema: "5",
                    name: "5",
                },
                Table {
                    schema: "abbc",
                    name: "fa",
                },
            ],
            accepted_cs: &[false, true, true, true],
            accepted_ci: &[false, true, true, true],
        },
        MatchTablesCase {
            args: &["*.*", "!/t[0-8]/.a??"],
            tables: &[
                Table {
                    schema: "t1",
                    name: "a01",
                },
                Table {
                    schema: "t9",
                    name: "a02",
                },
                Table {
                    schema: "5",
                    name: "5",
                },
                Table {
                    schema: "t8",
                    name: "a001",
                },
            ],
            accepted_cs: &[false, true, true, true],
            accepted_ci: &[false, true, true, true],
        },
        MatchTablesCase {
            args: &["*.*", "!a*.A*"],
            tables: &[
                Table {
                    schema: "aB",
                    name: "Ab",
                },
                Table {
                    schema: "AaB",
                    name: "aab",
                },
                Table {
                    schema: "acB",
                    name: "Afb",
                },
            ],
            accepted_cs: &[false, true, false],
            accepted_ci: &[false, false, false],
        },
        MatchTablesCase {
            args: &["BAR.*"],
            tables: &[
                Table {
                    schema: "bar",
                    name: "a",
                },
                Table {
                    schema: "BAR",
                    name: "a",
                },
            ],
            accepted_cs: &[false, true],
            accepted_ci: &[true, true],
        },
        MatchTablesCase {
            args: &["# comment", "x.y", "   \t"],
            tables: &[
                Table {
                    schema: "x",
                    name: "y",
                },
                Table {
                    schema: "y",
                    name: "y",
                },
            ],
            accepted_cs: &[true, false],
            accepted_ci: &[true, false],
        },
        MatchTablesCase {
            args: &["p_123$.45", "中文.表名"],
            tables: &[
                Table {
                    schema: "p_123",
                    name: "45",
                },
                Table {
                    schema: "p_123$",
                    name: "45",
                },
                Table {
                    schema: "英文",
                    name: "表名",
                },
                Table {
                    schema: "中文",
                    name: "表名",
                },
            ],
            accepted_cs: &[false, true, false, true],
            accepted_ci: &[false, true, false, true],
        },
        MatchTablesCase {
            args: &[r"\\\..*"],
            tables: &[
                Table {
                    schema: r"\.",
                    name: "a",
                },
                Table {
                    schema: r"\\\.",
                    name: "b",
                },
                Table {
                    schema: r"\a",
                    name: "c",
                },
            ],
            accepted_cs: &[true, false, false],
            accepted_ci: &[true, false, false],
        },
        MatchTablesCase {
            args: &["[!a-z].[^a-z]"],
            tables: &[
                Table {
                    schema: "!",
                    name: "z",
                },
                Table {
                    schema: "!",
                    name: "^",
                },
                Table {
                    schema: "!",
                    name: "9",
                },
                Table {
                    schema: "a",
                    name: "z",
                },
                Table {
                    schema: "a",
                    name: "^",
                },
                Table {
                    schema: "a",
                    name: "9",
                },
                Table {
                    schema: "1",
                    name: "z",
                },
                Table {
                    schema: "1",
                    name: "^",
                },
                Table {
                    schema: "1",
                    name: "9",
                },
            ],
            accepted_cs: &[true, true, false, false, false, false, true, true, false],
            accepted_ci: &[true, true, false, false, false, false, true, true, false],
        },
        MatchTablesCase {
            args: &[r#""some ""quoted""".`identifiers?`"#],
            tables: &[
                Table {
                    schema: r#"some "quoted""#,
                    name: "identifiers?",
                },
                Table {
                    schema: r#"some "quoted""#,
                    name: "identifiers!",
                },
                Table {
                    schema: r#"some ""quoted"""#,
                    name: "identifiers?",
                },
                Table {
                    schema: r#"SOME "QUOTED""#,
                    name: "IDENTIFIERS?",
                },
                Table {
                    schema: "some\t\"quoted\"",
                    name: "identifiers?",
                },
            ],
            accepted_cs: &[true, false, false, false, false],
            accepted_ci: &[true, false, false, true, false],
        },
        MatchTablesCase {
            args: &["db*.*", "!*.cfg*", "*.cfgsample"],
            tables: &[
                Table {
                    schema: "irrelevant",
                    name: "table",
                },
                Table {
                    schema: "db1",
                    name: "tbl1",
                },
                Table {
                    schema: "db1",
                    name: "cfg1",
                },
                Table {
                    schema: "db1",
                    name: "cfgsample",
                },
                Table {
                    schema: "else",
                    name: "cfgsample",
                },
            ],
            accepted_cs: &[false, true, false, true, true],
            accepted_ci: &[false, true, false, true, true],
        },
        MatchTablesCase {
            args: &["*.*", "!S.D[!a-d]"],
            tables: &[
                Table {
                    schema: "S",
                    name: "D1",
                },
                Table {
                    schema: "S",
                    name: "Da",
                },
                Table {
                    schema: "S",
                    name: "Db",
                },
                Table {
                    schema: "S",
                    name: "Daa",
                },
            ],
            accepted_cs: &[false, true, true, true],
            accepted_ci: &[false, true, true, true],
        },
        MatchTablesCase {
            args: &["*.*", "!S.D[a-d]"],
            tables: &[
                Table {
                    schema: "S",
                    name: "D1",
                },
                Table {
                    schema: "S",
                    name: "Da",
                },
                Table {
                    schema: "S",
                    name: "Db",
                },
                Table {
                    schema: "S",
                    name: "Daa",
                },
            ],
            accepted_cs: &[true, false, false, true],
            accepted_ci: &[true, false, false, true],
        },
    ];

    for tc in cases {
        // Go 先解析大小写敏感过滤器，再用 CaseInsensitive 包装出大小写不敏感过滤器。
        let fcs = filter::Parse(to_strings(tc.args)).expect("parse table filter");
        let fci = filter::CaseInsensitive(
            filter::Parse(to_strings(tc.args))
                .expect("parse table filter for case-insensitive wrapper"),
        );
        for (i, tbl) in tc.tables.iter().enumerate() {
            assert_eq!(
                tc.accepted_cs[i],
                fcs.MatchTable(tbl.schema, tbl.name),
                "cs tbl {}.{}",
                tbl.schema,
                tbl.name
            );
            assert_eq!(
                tc.accepted_ci[i],
                fci.MatchTable(tbl.schema, tbl.name),
                "ci tbl {}.{}",
                tbl.schema,
                tbl.name
            );
        }
    }
}

// TestMatchSchemas 对应 Go 的 schema 级匹配表驱动测试。
#[test]
fn test_match_schemas() {
    let cases = vec![
        MatchSchemasCase {
            args: &[],
            schemas: &["foo"],
            accepted_cs: &[false],
            accepted_ci: &[false],
        },
        MatchSchemasCase {
            args: &["*.*"],
            schemas: &["foo"],
            accepted_cs: &[true],
            accepted_ci: &[true],
        },
        MatchSchemasCase {
            args: &["foo.*"],
            schemas: &["foo", "foo1"],
            accepted_cs: &[true, false],
            accepted_ci: &[true, false],
        },
        MatchSchemasCase {
            args: &["*.*", "!foo1.*"],
            schemas: &["foo", "foo1"],
            accepted_cs: &[true, false],
            accepted_ci: &[true, false],
        },
        MatchSchemasCase {
            args: &["foo.bar1"],
            schemas: &["foo", "foo1"],
            accepted_cs: &[true, false],
            accepted_ci: &[true, false],
        },
        MatchSchemasCase {
            args: &["*.*", "!foo.bar"],
            schemas: &["foo", "foo1"],
            accepted_cs: &[true, true],
            accepted_ci: &[true, true],
        },
        MatchSchemasCase {
            args: &["/^foo/.*", r"!/^foo/./^sbtest-\d/"],
            schemas: &["foo", "foo2"],
            accepted_cs: &[true, true],
            accepted_ci: &[true, true],
        },
        MatchSchemasCase {
            args: &["*.*", "!FOO*.*", "!*.*FoO"],
            schemas: &["foo", "FOO", "foobar", "FOOBAR", "bar", "BAR"],
            accepted_cs: &[true, false, true, false, true, true],
            accepted_ci: &[false, false, false, false, true, true],
        },
    ];

    for tc in cases {
        let fcs = filter::Parse(to_strings(tc.args)).expect("parse table filter");
        let fci = filter::CaseInsensitive(
            filter::Parse(to_strings(tc.args))
                .expect("parse table filter for case-insensitive wrapper"),
        );
        for (i, schema) in tc.schemas.iter().enumerate() {
            // 关键迁移点：MatchSchema 只看库名，Go 中具体表级 negative 规则不一定排除整个 schema。
            assert_eq!(
                tc.accepted_cs[i],
                fcs.MatchSchema(schema),
                "cs schema {}",
                schema
            );
            assert_eq!(
                tc.accepted_ci[i],
                fci.MatchSchema(schema),
                "ci schema {}",
                schema
            );
        }
    }
}

// TestParseFailures2 对应 Go 中表规则解析失败的错误消息匹配。
#[test]
fn test_parse_failures2() {
    let cases = vec![
        ParseFailureCase {
            arg: "/^t[0-9]+((?!_copy).)*$/.*",
            msg: ".*: invalid pattern: error parsing regexp:.*",
        },
        ParseFailureCase {
            arg: "/^t[0-9]+sp(?=copy).*/.*",
            msg: ".*: invalid pattern: error parsing regexp:.*",
        },
        ParseFailureCase {
            arg: "a.b.c",
            msg: ".*: syntax error: stray characters after table pattern",
        },
        ParseFailureCase {
            arg: "a%b.c",
            msg: ".*: unexpected special character '%'",
        },
        ParseFailureCase {
            arg: r"a\tb.c",
            msg: r".*: cannot escape a letter or number \(\\t\), it is reserved for future extension",
        },
        ParseFailureCase {
            arg: "[].*",
            msg: ".*: syntax error: failed to parse character class",
        },
        ParseFailureCase {
            arg: "[!].*",
            msg: r".*: invalid pattern: error parsing regexp: missing closing \]:.*",
        },
        ParseFailureCase {
            arg: "[.*",
            msg: r".*: syntax error: failed to parse character class",
        },
        ParseFailureCase {
            arg: r"[\d\D].*",
            msg: r".*: syntax error: failed to parse character class",
        },
        ParseFailureCase {
            arg: "db",
            msg: r".*: wrong table pattern",
        },
        ParseFailureCase {
            arg: "db.",
            msg: r".*: syntax error: missing pattern",
        },
        ParseFailureCase {
            arg: "`db`*.*",
            msg: r".*: syntax error: missing '\.' between schema and table patterns",
        },
        ParseFailureCase {
            arg: "/db.*",
            msg: r".*: syntax error: incomplete regexp",
        },
        ParseFailureCase {
            arg: "`db.*",
            msg: r".*: syntax error: incomplete quoted identifier",
        },
        ParseFailureCase {
            arg: r#""db.*"#,
            msg: r".*: syntax error: incomplete quoted identifier",
        },
        ParseFailureCase {
            arg: r"db\",
            msg: r".*: syntax error: cannot place \\ at end of line",
        },
        ParseFailureCase {
            arg: "db.tbl#not comment",
            msg: r".*: unexpected special character '#'",
        },
    ];

    for tc in cases {
        let err = filter::Parse(to_strings(&[tc.arg])).expect_err("parse should fail");
        assert_regex_match(tc.msg, err.to_string().as_str(), tc.arg);
    }
}

// TestImport2 对应 Go 的表过滤 @file 导入测试。
#[test]
fn test_import2() {
    let dir = tempdir();
    let path1 = dir.join("1.txt");
    let path2 = dir.join("2.txt");
    std::fs::write(&path1, "\n\t\t\tdb?.tbl?\n\t\t\tdb02.tbl02\n\t\t").expect("write path1");
    std::fs::write(&path2, "\n\t\t\tdb03.tbl03\n\t\t\t!db4.tbl4\n\t\t").expect("write path2");

    let f = filter::Parse(vec![
        format!("@{}", path1.display()),
        format!("@{}", path2.display()),
        "db04.tbl04".to_string(),
    ])
    .expect("parse imported table filter");

    assert!(f.MatchTable("db1", "tbl1"));
    assert!(f.MatchTable("db2", "tbl2"));
    assert!(f.MatchTable("db3", "tbl3"));
    assert!(!f.MatchTable("db4", "tbl4"));
    assert!(!f.MatchTable("db01", "tbl01"));
    assert!(f.MatchTable("db02", "tbl02"));
    assert!(f.MatchTable("db03", "tbl03"));
    assert!(f.MatchTable("db04", "tbl04"));
}

// TestRecursiveImport2 对应 Go 的递归 @file 禁止和缺失文件错误。
#[test]
fn test_recursive_import2() {
    let dir = tempdir();
    let path3 = dir.join("3.txt");
    let path4 = dir.join("4.txt");
    std::fs::write(&path3, "db1.tbl1").expect("write path3");
    std::fs::write(&path4, format!("# comment\n\n@{}", path3.display())).expect("write path4");

    let err = filter::Parse(vec![format!("@{}", path4.display())])
        .expect_err("recursive import should fail");
    assert_regex_match(
        r".*4\.txt:3: importing filter files recursively is not allowed",
        err.to_string().as_str(),
        "recursive import",
    );

    let missing = dir.join("5.txt");
    let err = filter::Parse(vec![format!("@{}", missing.display())])
        .expect_err("missing import should fail");
    assert_regex_match(
        r".*: cannot open filter file: open .*5\.txt: .*",
        err.to_string().as_str(),
        "missing import",
    );
}

// TestAll 对应 Go 的 All 过滤器：大小写包装前后都匹配任意库表。
#[test]
fn test_all() {
    let f = filter::All();
    assert!(f.MatchTable("db1", "tbl1"));
    assert!(f.MatchSchema("db1"));

    let f = filter::CaseInsensitive(f);
    assert!(f.MatchTable("db1", "tbl1"));
    assert!(f.MatchSchema("db1"));
}

#[test]
fn case_insensitive_uses_go_simple_unicode_lowercase() {
    for (rule, upper, lower, other) in [("İ.İ", "İ", "i", "i\u{307}"), ("ΟΣ.ΟΣ", "ΟΣ", "οσ", "ος")]
    {
        let f = filter::CaseInsensitive(filter::Parse(to_strings(&[rule])).unwrap());
        assert!(f.MatchSchema(upper), "{rule}");
        assert!(f.MatchSchema(lower), "{rule}");
        assert!(f.MatchTable(upper, upper), "{rule}");
        assert!(f.MatchTable(lower, upper), "{rule}");
        assert!(!f.MatchSchema(other), "{rule}");
        assert!(!f.MatchTable(lower, other), "{rule}");
    }
}

#[derive(Debug)]
struct LowerOnceFilter(bool);

impl filter::Filter for LowerOnceFilter {
    fn MatchTable(&self, schema: &str, table: &str) -> bool {
        self.0 && schema == "db" && table == "tbl"
    }

    fn MatchSchema(&self, schema: &str) -> bool {
        self.0 && schema == "db"
    }

    fn toLower(&self) -> Box<dyn filter::Filter> {
        assert!(
            !self.0,
            "Go loweredFilter.toLower returns itself without reconverting"
        );
        Box::new(Self(true))
    }
}

#[test]
fn case_insensitive_repeated_wrapper_preserves_lowered_filter() {
    let mut f = filter::CaseInsensitive(Box::new(LowerOnceFilter(false)));
    for _ in 0..3 {
        f = filter::CaseInsensitive(f);
        assert!(f.MatchSchema("DB"));
        assert!(f.MatchTable("DB", "TBL"));
        assert!(!f.MatchTable("DB", "other"));
    }
}
