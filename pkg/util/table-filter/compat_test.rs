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

// MySQL 复制规则兼容层单测：Schemas/Tables 过滤器与旧规则解析。
//
// 对照 Go 用例验证白/黑名单、通配/正则及解析失败消息。

// 这些测试只描述旧版 MySQL replication 规则兼容层的匹配与错误语义；
//

use crate as filter;
use regex::Regex;

/// 用正则断言错误字符串，便于对齐 Go 的模糊错误匹配。
fn assert_regex_match(pattern: &str, actual: &str, context: &str) {
    let regex = Regex::new(pattern).expect("valid assertion regex");
    assert!(
        regex.is_match(actual),
        "{context}: {actual:?} does not match {pattern:?}"
    );
}

/// 测试用库表字面量。
struct Table {
    schema: &'static str,
    name: &'static str,
}

/// 旧版四元组规则的静态测试夹具。
struct LegacyRules {
    ignore_dbs: &'static [&'static str],
    do_dbs: &'static [&'static str],
    do_tables: &'static [Table],
    ignore_tables: &'static [Table],
}

/// 旧规则用例：规则 + 应接受/拒绝的库表。
struct LegacyCase {
    rules: LegacyRules,
    accepted: &'static [Table],
    rejected: &'static [Table],
}

/// 解析失败用例：非法模式与期望错误正则。
struct ParseFailureCase {
    arg: &'static str,
    msg: &'static str,
}

/// 将静态夹具转为 `MySQLReplicationRules`。
fn to_mysql_rules(rules: LegacyRules) -> filter::MySQLReplicationRules {
    filter::MySQLReplicationRules {
        DoTables: rules
            .do_tables
            .iter()
            .map(|table| Box::new(filter::Table::new(table.schema, table.name)))
            .collect(),
        DoDBs: rules
            .do_dbs
            .iter()
            .map(|schema| (*schema).to_owned())
            .collect(),
        IgnoreTables: rules
            .ignore_tables
            .iter()
            .map(|table| Box::new(filter::Table::new(table.schema, table.name)))
            .collect(),
        IgnoreDBs: rules
            .ignore_dbs
            .iter()
            .map(|schema| (*schema).to_owned())
            .collect(),
    }
}

// TestSchemaFilter 对应 Go 中只按 schema 过滤的兼容测试。
#[test]
fn test_schema_filter() {
    let sf0 = filter::CaseInsensitive(filter::NewSchemasFilter(vec![
        "foo?".to_string(),
        "bar".to_string(),
    ]));
    assert!(sf0.MatchTable("foo?", "a"));
    assert!(!sf0.MatchTable("food", "a"));
    assert!(sf0.MatchTable("bar", "b"));
    assert!(sf0.MatchTable("BAR", "b"));

    let sf1 = filter::NewSchemasFilter(vec![r"\baz".to_string()]);
    assert!(!sf1.MatchSchema("baz"));
    assert!(!sf1.MatchSchema("Baz"));
    assert!(sf1.MatchSchema(r"\baz"));
    assert!(!sf1.MatchSchema(r"\Baz"));

    // Go 的空 schema filter 不匹配任意表。
    let sf2 = filter::NewSchemasFilter(vec![]);
    assert!(!sf2.MatchTable("aaa", "bbb"));
}

// TestTableFilter 对应 Go 中直接构造 table 列表的兼容测试。
#[test]
fn test_table_filter() {
    let tf0 = filter::CaseInsensitive(filter::NewTablesFilter(vec![
        filter::Table {
            Schema: "foo?".to_string(),
            Name: "bar*".to_string(),
        },
        filter::Table {
            Schema: "BAR?".to_string(),
            Name: "FOO*".to_string(),
        },
    ]));
    assert!(tf0.MatchTable("foo?", "bar*"));
    assert!(tf0.MatchTable("bar?", "foo*"));
    assert!(tf0.MatchTable("FOO?", "BAR*"));
    assert!(!tf0.MatchTable("foo?", "bar"));
    assert!(!tf0.MatchTable("BARD", "FOO*"));

    let tf1 = filter::NewTablesFilter(vec![filter::Table {
        Schema: r"\baz".to_string(),
        Name: "BAR".to_string(),
    }]);
    assert!(!tf1.MatchSchema("baz"));
    assert!(!tf1.MatchSchema("Baz"));
    assert!(tf1.MatchSchema(r"\baz"));
    assert!(!tf1.MatchSchema(r"\Baz"));

    let tf2 = filter::NewTablesFilter(vec![]);
    assert!(!tf2.MatchTable("aaa", "bbb"));
}

// Go strings.ToLower applies Unicode's simple one-rune lowercase mapping.
#[test]
fn test_mysql_replication_rules_to_lower_uses_go_simple_case_mapping() {
    let mut rules = filter::MySQLReplicationRules {
        DoTables: vec![Box::new(filter::Table::new("\u{130}DB", "\u{130}TABLE"))],
        DoDBs: vec!["\u{130}DO".to_string()],
        IgnoreTables: vec![Box::new(filter::Table::new("\u{130}IGN", "\u{130}TABLE"))],
        IgnoreDBs: vec!["\u{130}IGNORE".to_string()],
    };

    filter::MySQLReplicationRules::ToLower(Some(&mut rules));

    assert_eq!(rules.DoTables[0].Schema, "idb");
    assert_eq!(rules.DoTables[0].Name, "itable");
    assert_eq!(rules.DoDBs, ["ido"]);
    assert_eq!(rules.IgnoreTables[0].Schema, "iign");
    assert_eq!(rules.IgnoreTables[0].Name, "itable");
    assert_eq!(rules.IgnoreDBs, ["iignore"]);
}

// TestLegacyFilter 对应 Go 的 MySQLReplicationRules 表驱动兼容测试。
#[test]
fn test_legacy_filter() {
    let cases = vec![
        LegacyCase {
            rules: LegacyRules {
                ignore_dbs: &[],
                do_dbs: &[],
                do_tables: &[],
                ignore_tables: &[],
            },
            accepted: &[Table {
                schema: "foo",
                name: "bar",
            }],
            rejected: &[],
        },
        LegacyCase {
            rules: LegacyRules {
                ignore_dbs: &["foo"],
                do_dbs: &["foo"],
                do_tables: &[],
                ignore_tables: &[],
            },
            accepted: &[Table {
                schema: "foo",
                name: "bar",
            }],
            rejected: &[Table {
                schema: "foo1",
                name: "bar",
            }],
        },
        LegacyCase {
            rules: LegacyRules {
                ignore_dbs: &["foo1"],
                do_dbs: &[],
                do_tables: &[],
                ignore_tables: &[],
            },
            accepted: &[Table {
                schema: "foo",
                name: "bar",
            }],
            rejected: &[Table {
                schema: "foo1",
                name: "bar",
            }],
        },
        LegacyCase {
            rules: LegacyRules {
                ignore_dbs: &[],
                do_dbs: &[],
                do_tables: &[Table {
                    schema: "foo",
                    name: "bar1",
                }],
                ignore_tables: &[],
            },
            accepted: &[Table {
                schema: "foo",
                name: "bar1",
            }],
            rejected: &[
                Table {
                    schema: "foo",
                    name: "bar",
                },
                Table {
                    schema: "foo1",
                    name: "bar",
                },
                Table {
                    schema: "foo1",
                    name: "bar1",
                },
            ],
        },
        LegacyCase {
            rules: LegacyRules {
                ignore_dbs: &[],
                do_dbs: &[],
                do_tables: &[],
                ignore_tables: &[Table {
                    schema: "foo",
                    name: "bar",
                }],
            },
            accepted: &[
                Table {
                    schema: "foo",
                    name: "bar1",
                },
                Table {
                    schema: "foo1",
                    name: "bar",
                },
                Table {
                    schema: "foo1",
                    name: "bar1",
                },
            ],
            rejected: &[Table {
                schema: "foo",
                name: "bar",
            }],
        },
        LegacyCase {
            rules: LegacyRules {
                ignore_dbs: &[],
                do_dbs: &["~^foo"],
                do_tables: &[],
                ignore_tables: &[Table {
                    schema: "~^foo",
                    name: r"~^sbtest-\d",
                }],
            },
            accepted: &[
                Table {
                    schema: "foo",
                    name: "sbtest",
                },
                Table {
                    schema: "foo",
                    name: r"sbtest-\d",
                },
            ],
            rejected: &[
                Table {
                    schema: "fff",
                    name: "bar",
                },
                Table {
                    schema: "foo1",
                    name: "sbtest-1",
                },
            ],
        },
        LegacyCase {
            rules: LegacyRules {
                ignore_dbs: &["foo[bar]", "baz?", r"special\"],
                do_dbs: &[],
                do_tables: &[],
                ignore_tables: &[],
            },
            accepted: &[
                Table {
                    schema: "foo[bar]",
                    name: "1",
                },
                Table {
                    schema: "food",
                    name: "2",
                },
                Table {
                    schema: "fo",
                    name: "3",
                },
                Table {
                    schema: r"special\\",
                    name: "4",
                },
                Table {
                    schema: "bazzz",
                    name: "9",
                },
                Table {
                    schema: r"special\$",
                    name: "10",
                },
                Table {
                    schema: r"afooa",
                    name: "11",
                },
            ],
            rejected: &[
                Table {
                    schema: "foor",
                    name: "5",
                },
                Table {
                    schema: "baz?",
                    name: "6",
                },
                Table {
                    schema: "baza",
                    name: "7",
                },
                Table {
                    schema: r"special\",
                    name: "8",
                },
            ],
        },
        LegacyCase {
            rules: LegacyRules {
                ignore_dbs: &[],
                do_dbs: &[r"!@#$%^&*\?"],
                do_tables: &[],
                ignore_tables: &[],
            },
            accepted: &[Table {
                schema: r"!@#$%^&abcdef\g",
                name: "1",
            }],
            rejected: &[Table {
                schema: "abcdef",
                name: "2",
            }],
        },
        LegacyCase {
            rules: LegacyRules {
                ignore_dbs: &[],
                do_dbs: &[r"1[!abc]", r"2[^abc]", r"3[\d]"],
                do_tables: &[],
                ignore_tables: &[],
            },
            accepted: &[
                Table {
                    schema: "1!",
                    name: "1",
                },
                Table {
                    schema: "1z",
                    name: "4",
                },
                Table {
                    schema: "2^",
                    name: "3",
                },
                Table {
                    schema: "2a",
                    name: "5",
                },
                Table {
                    schema: "3d",
                    name: "6",
                },
                Table {
                    schema: r"3\",
                    name: "8",
                },
            ],
            rejected: &[
                Table {
                    schema: "1a",
                    name: "2",
                },
                Table {
                    schema: "30",
                    name: "7",
                },
            ],
        },
        LegacyCase {
            rules: LegacyRules {
                ignore_dbs: &[],
                do_dbs: &["foo", "bar"],
                do_tables: &[
                    Table {
                        schema: "*",
                        name: "a",
                    },
                    Table {
                        schema: "*",
                        name: "b",
                    },
                ],
                ignore_tables: &[],
            },
            accepted: &[
                Table {
                    schema: "foo",
                    name: "a",
                },
                Table {
                    schema: "foo",
                    name: "b",
                },
                Table {
                    schema: "bar",
                    name: "a",
                },
                Table {
                    schema: "bar",
                    name: "b",
                },
            ],
            rejected: &[
                Table {
                    schema: "foo",
                    name: "c",
                },
                Table {
                    schema: "baz",
                    name: "a",
                },
            ],
        },
    ];

    // Go 首先验证 nil 规则会解析为全量匹配。
    let f = filter::ParseMySQLReplicationRules(None).expect("parse nil legacy rules");
    assert!(f.MatchTable("foo", "bar"));

    for tc in cases {
        // 关键迁移点：旧规则先解析，再统一包一层 CaseInsensitive，与 Go 循环内顺序一致。
        let rules = to_mysql_rules(tc.rules);
        let f = filter::CaseInsensitive(
            filter::ParseMySQLReplicationRules(Some(&rules)).expect("parse legacy rules"),
        );
        for tbl in tc.accepted {
            assert!(
                f.MatchTable(tbl.schema, tbl.name),
                "accept case {}.{}",
                tbl.schema,
                tbl.name
            );
        }
        for tbl in tc.rejected {
            assert!(
                !f.MatchTable(tbl.schema, tbl.name),
                "reject case {}.{}",
                tbl.schema,
                tbl.name
            );
        }
    }
}

// TestParseLegacyFailures 对应 Go 中旧规则解析错误的正则断言。
#[test]
fn test_parse_legacy_failures() {
    let cases = vec![
        ParseFailureCase {
            arg: "[a",
            msg: r"error parsing regexp: missing closing \]:.*",
        },
        ParseFailureCase {
            arg: "",
            msg: "pattern cannot be empty",
        },
    ];

    for tc in cases {
        let rules = filter::MySQLReplicationRules {
            DoDBs: vec![tc.arg.to_string()],
            ..Default::default()
        };
        let err =
            filter::ParseMySQLReplicationRules(Some(&rules)).expect_err("legacy parse should fail");
        assert_regex_match(tc.msg, err.to_string().as_str(), tc.arg);
    }
}
