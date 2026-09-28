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

// Filter 表驱动单元测试：覆盖 ApplyOn/Apply、大小写与非法正则。
//
// 对应 Go `filter_test.go`。用例构造 Do/Ignore 的 DB/Table 规则（含 `~` 正则），
// 校验过滤结果、输入不被改写，以及前瞻正则在 New 时失败。

use super::{New, Rules, Table};

// ExpectedTables 用来保留 Go 里 nil slice 和空 slice 的差异；require.Equal 会区分两者。
/// 期望输出：区分 Go 的 nil slice 与非空列表。
#[derive(Clone, Debug, PartialEq)]
enum ExpectedTables {
    /// 对应 Go `nil` slice。
    Nil,
    /// 对应非空（或空但非 nil）表列表。
    List(Vec<Box<Table>>),
}

// filterCase 对应 Go 表驱动用例中的匿名 struct。
/// 单条表驱动用例：规则、输入、期望输出与大小写开关。
#[derive(Debug)]
struct filterCase {
    /// 过滤规则。
    rules: Rules,
    /// 输入表；`None` 表示 Go 侧 nil。
    input: Option<Vec<Box<Table>>>,
    /// 期望过滤结果。
    output: ExpectedTables,
    /// 是否大小写敏感。
    caseSensitive: bool,
}

// tb 对应 Go 字面量 &Table{Schema: "...", Name: "..."}。
/// 构造 `Box<Table>` 测试辅助。
fn tb(schema: &str, name: &str) -> Box<Table> {
    Box::new(Table {
        Schema: schema.to_string(),
        Name: name.to_string(),
    })
}

// rules 保留 Go Rules 字面量的四类规则；未传入的字段保持空列表，等价于 Go 测试里的 nil/省略字段。
/// 构造四类规则的 `Rules` 测试辅助。
fn rules(
    ignore_dbs: &[&str],
    do_dbs: &[&str],
    ignore_tables: Vec<Box<Table>>,
    do_tables: Vec<Box<Table>>,
) -> Rules {
    Rules {
        IgnoreDBs: ignore_dbs.iter().map(|s| s.to_string()).collect(),
        DoDBs: do_dbs.iter().map(|s| s.to_string()).collect(),
        IgnoreTables: ignore_tables,
        DoTables: do_tables,
        ..Default::default()
    }
}

/// 包装为 `Some(tables)`，对应 Go 非 nil 输入切片。
fn some_tables(tables: Vec<Box<Table>>) -> Option<Vec<Box<Table>>> {
    Some(tables)
}

/// 包装为 `ExpectedTables::List`。
fn expected(tables: Vec<Box<Table>>) -> ExpectedTables {
    ExpectedTables::List(tables)
}

// cloneTables 对应 Go helper：nil 输入直接返回 nil，否则逐个调用 Table.Clone，避免过滤器改写原输入。
/// 深拷贝表列表，用于断言过滤器未原地修改输入。
fn cloneTables(tbs: &[Box<Table>]) -> Vec<Box<Table>> {
    tbs.iter().map(|tb| tb.Clone()).collect()
}

/// 按 `ExpectedTables` 变体断言实际输出。
fn assert_expected(expected: ExpectedTables, actual: Vec<Box<Table>>) {
    match expected {
        // Rust 的生产 API 用 Vec 表示 Go 的 nil/empty slice；nil 对照必须为空。
        ExpectedTables::Nil => assert!(actual.is_empty()),
        ExpectedTables::List(expected) => assert_eq!(expected, actual),
    }
}

/// 表驱动测试 `ApplyOn`：空规则、库/表规则、正则与大小写组合。
#[test]
fn TestFilterOnSchema() {
    let cases = vec![
        // empty rules
        filterCase {
            rules: rules(&[], &[], vec![], vec![]),
            input: None,
            output: ExpectedTables::Nil,
            caseSensitive: false,
        },
        filterCase {
            rules: rules(&[], &[], vec![], vec![]),
            input: some_tables(vec![tb("foo", "bar"), tb("foo", "")]),
            output: expected(vec![tb("foo", "bar"), tb("foo", "")]),
            caseSensitive: false,
        },
        // schema-only rules
        filterCase {
            rules: rules(&["foo"], &["foo"], vec![], vec![]),
            input: some_tables(vec![
                tb("foo", "bar"),
                tb("foo", ""),
                tb("foo1", "bar"),
                tb("foo1", ""),
            ]),
            output: expected(vec![tb("foo", "bar"), tb("foo", "")]),
            caseSensitive: false,
        },
        filterCase {
            rules: rules(&["foo1"], &[], vec![], vec![]),
            input: some_tables(vec![
                tb("foo", "bar"),
                tb("foo", ""),
                tb("foo1", "bar"),
                tb("foo1", ""),
            ]),
            output: expected(vec![tb("foo", "bar"), tb("foo", "")]),
            caseSensitive: false,
        },
        // DoTable rules(Without regex)
        filterCase {
            rules: rules(&[], &[], vec![], vec![tb("foo", "bar1")]),
            input: some_tables(vec![
                tb("foo", "bar"),
                tb("foo", "bar1"),
                tb("foo", ""),
                tb("fff", "bar1"),
            ]),
            output: expected(vec![tb("foo", "bar1"), tb("foo", "")]),
            caseSensitive: false,
        },
        // ignoreTable rules(Without regex)
        filterCase {
            rules: rules(&[], &[], vec![tb("foo", "bar")], vec![]),
            input: some_tables(vec![
                tb("foo", "bar"),
                tb("foo", "bar1"),
                tb("foo", ""),
                tb("fff", "bar1"),
            ]),
            output: expected(vec![tb("foo", "bar1"), tb("foo", ""), tb("fff", "bar1")]),
            caseSensitive: false,
        },
        filterCase {
            // all regexp
            rules: rules(&[], &["~^foo"], vec![tb("~^foo", "~^sbtest-\\d")], vec![]),
            input: some_tables(vec![
                tb("foo", "sbtest"),
                tb("foo1", "sbtest-1"),
                tb("foo2", ""),
                tb("fff", "bar"),
            ]),
            output: expected(vec![tb("foo", "sbtest"), tb("foo2", "")]),
            caseSensitive: false,
        },
        // test rule with * or ?
        filterCase {
            rules: rules(&["foo[bar]", "foo?", "special\\"], &[], vec![], vec![]),
            input: some_tables(vec![
                tb("foor", "a"),
                tb("foo[bar]", "b"),
                tb("fo", "c"),
                tb("foo?", "d"),
                tb("special\\", "e"),
            ]),
            output: expected(vec![tb("foo[bar]", "b"), tb("fo", "c")]),
            caseSensitive: false,
        },
        // ensure non case-insensitive
        filterCase {
            rules: rules(&["~^FOO"], &[], vec![tb("~.*", "~FoO$")], vec![]),
            input: some_tables(vec![
                tb("FOO1", "a"),
                tb("foo2", "b"),
                tb("BoO3", "cFoO"),
                tb("Foo4", "dfoo"),
                tb("5", "5"),
            ]),
            output: expected(vec![tb("5", "5")]),
            caseSensitive: false,
        },
        // ensure case-insensitive
        filterCase {
            rules: rules(&["~^FOO"], &[], vec![tb("~.*", "~FoO$")], vec![]),
            input: some_tables(vec![
                tb("FOO1", "a"),
                tb("foo2", "b"),
                tb("BoO3", "cFoo"),
                tb("Foo4", "dfoo"),
                tb("5", "5"),
            ]),
            output: expected(vec![
                tb("foo2", "b"),
                tb("BoO3", "cFoo"),
                tb("Foo4", "dfoo"),
                tb("5", "5"),
            ]),
            caseSensitive: true,
        },
        // test the rule whose schema part is not regex and the table part is regex.
        filterCase {
            rules: rules(&[], &[], vec![tb("a?b?", "~f[0-9]")], vec![]),
            input: some_tables(vec![
                tb("abbd", "f1"),
                tb("aaaa", "f2"),
                tb("5", "5"),
                tb("abbc", "fa"),
            ]),
            output: expected(vec![tb("aaaa", "f2"), tb("5", "5"), tb("abbc", "fa")]),
            caseSensitive: false,
        },
        // test the rule whose schema part is regex and the table part is not regex.
        filterCase {
            rules: rules(&[], &[], vec![tb("~t[0-8]", "a??")], vec![]),
            input: some_tables(vec![
                tb("t1", "a01"),
                tb("t9", "a02"),
                tb("5", "5"),
                tb("t9", "a001"),
            ]),
            output: expected(vec![tb("t9", "a02"), tb("5", "5"), tb("t9", "a001")]),
            caseSensitive: false,
        },
        filterCase {
            rules: rules(&[], &[], vec![tb("a*", "A*")], vec![]),
            input: some_tables(vec![tb("aB", "Ab"), tb("AaB", "aab"), tb("acB", "Afb")]),
            output: expected(vec![tb("AaB", "aab")]),
            caseSensitive: true,
        },
        filterCase {
            rules: rules(&[], &[], vec![tb("a*", "A*")], vec![]),
            input: some_tables(vec![tb("aB", "Ab"), tb("AaB", "aab"), tb("acB", "Afb")]),
            output: ExpectedTables::Nil,
            caseSensitive: false,
        },
    ];

    for tt in cases {
        // Go 这里使用 require.NoError；用 expect 表达构造过滤器必须成功。
        let ft = New(tt.caseSensitive, Some(Box::new(tt.rules))).expect("New should succeed");
        let input = tt.input.unwrap_or_default();
        let originInput = cloneTables(&input);
        let got = ft.ApplyOn(input.clone());
        // 原测试会打印 got/expected 并检查 ApplyOn 不改写输入。
        assert_eq!(input, originInput);
        assert_expected(tt.output, got);
    }
}

/// 表驱动测试 `Apply`：大小写与规则组合，且输入不被原地修改。
#[test]
fn TestCaseSensitiveApply() {
    let cases = vec![
        filterCase {
            rules: rules(&["foo"], &["foo"], vec![], vec![]),
            input: some_tables(vec![
                tb("foo", "bar"),
                tb("foo", ""),
                tb("foo1", "bar"),
                tb("foo1", ""),
            ]),
            output: expected(vec![tb("foo", "bar"), tb("foo", "")]),
            caseSensitive: false,
        },
        filterCase {
            rules: rules(&["foo1"], &[], vec![], vec![]),
            input: some_tables(vec![
                tb("foo", "bar"),
                tb("foo", ""),
                tb("foo1", "bar"),
                tb("foo1", ""),
            ]),
            output: expected(vec![tb("foo", "bar"), tb("foo", "")]),
            caseSensitive: false,
        },
        // ignoreTable rules(Without regex)
        filterCase {
            rules: rules(&[], &[], vec![tb("Foo", "bAr")], vec![]),
            input: some_tables(vec![
                tb("foo", "bar"),
                tb("foo", "bar1"),
                tb("foo", ""),
                tb("fff", "bar1"),
            ]),
            output: expected(vec![tb("foo", "bar1"), tb("foo", ""), tb("fff", "bar1")]),
            caseSensitive: false,
        },
        filterCase {
            // all regexp
            rules: rules(&[], &["~^foo"], vec![tb("~^foo", "~^sbtest-\\d")], vec![]),
            input: some_tables(vec![
                tb("foo", "sbtest"),
                tb("foo1", "sbtest-1"),
                tb("foo2", ""),
                tb("fff", "bar"),
            ]),
            output: expected(vec![tb("foo", "sbtest"), tb("foo2", "")]),
            caseSensitive: false,
        },
        // test rule with * or ?
        filterCase {
            rules: rules(&["foo[bar]", "foo?", "special\\"], &[], vec![], vec![]),
            input: some_tables(vec![
                tb("foor", "a"),
                tb("foo[bar]", "b"),
                tb("Fo", "c"),
                tb("foo?", "d"),
                tb("special\\", "e"),
            ]),
            output: expected(vec![tb("foo[bar]", "b"), tb("Fo", "c")]),
            caseSensitive: false,
        },
        // ensure non case-insensitive
        filterCase {
            rules: rules(&["~^FOO"], &[], vec![tb("~.*", "~FoO$")], vec![]),
            input: some_tables(vec![
                tb("FOO1", "a"),
                tb("foo2", "b"),
                tb("BoO3", "cFoO"),
                tb("Foo4", "dfoo"),
                tb("5", "5"),
            ]),
            output: expected(vec![tb("5", "5")]),
            caseSensitive: false,
        },
        // ensure case-insensitive
        filterCase {
            rules: rules(&["~^FOO"], &[], vec![tb("~.*", "~FoO$")], vec![]),
            input: some_tables(vec![
                tb("FOO1", "a"),
                tb("foo2", "b"),
                tb("BoO3", "cFoo"),
                tb("Foo4", "dfoo"),
                tb("5", "5"),
            ]),
            output: expected(vec![
                tb("foo2", "b"),
                tb("BoO3", "cFoo"),
                tb("Foo4", "dfoo"),
                tb("5", "5"),
            ]),
            caseSensitive: true,
        },
        // test the rule whose schema part is not regex and the table part is regex.
        filterCase {
            rules: rules(&[], &[], vec![tb("a?b?", "~f[0-9]")], vec![]),
            input: some_tables(vec![
                tb("abBd", "f1"),
                tb("aAAa", "f2"),
                tb("5", "5"),
                tb("abbc", "FA"),
            ]),
            output: expected(vec![tb("aAAa", "f2"), tb("5", "5"), tb("abbc", "FA")]),
            caseSensitive: false,
        },
        // test the rule whose schema part is regex and the table part is not regex.
        filterCase {
            rules: rules(&[], &[], vec![tb("~t[0-8]", "A??")], vec![]),
            input: some_tables(vec![
                tb("t1", "a01"),
                tb("t9", "A02"),
                tb("5", "5"),
                tb("T9", "a001"),
            ]),
            output: expected(vec![tb("t9", "A02"), tb("5", "5"), tb("T9", "a001")]),
            caseSensitive: false,
        },
        filterCase {
            rules: rules(&[], &[], vec![tb("a*", "A*")], vec![]),
            input: some_tables(vec![tb("aB", "Ab"), tb("AaB", "aab"), tb("acB", "Afb")]),
            output: expected(vec![tb("AaB", "aab")]),
            caseSensitive: true,
        },
        filterCase {
            rules: rules(&[], &[], vec![tb("a*", "A*")], vec![]),
            input: some_tables(vec![tb("aB", "Ab"), tb("AaB", "aab"), tb("acB", "Afb")]),
            output: expected(vec![]),
            caseSensitive: false,
        },
    ];

    for tt in cases {
        let ft = New(tt.caseSensitive, Some(Box::new(tt.rules))).expect("New should succeed");
        let input = tt.input.unwrap_or_default();
        let originInput = cloneTables(&input);
        let got = ft.Apply(input.clone());
        // Apply 与 ApplyOn 都应保持传入表对象不被原地修改。
        assert_eq!(input, originInput);
        assert_expected(tt.output, got);
    }
}

/// 仅有 schema、无 table name 的输入应被保留（schema DDL 场景）。
#[test]
fn TestMaxBox() {
    let rules = rules(&[], &[], vec![tb("test1", "t2")], vec![tb("test1", "t1")]);

    let r = New(false, Some(Box::new(rules))).expect("New should succeed");
    let x = tb("test1", "");
    let res = r.ApplyOn(vec![x.clone()]);
    // Go require.Len/Equal 校验只有 schema 但无 table name 的输入能保留。
    assert_eq!(1, res.len());
    assert_eq!(x, res[0]);
}

/// 校验大小写敏感开关对 Match/ApplyOn 的真实影响，且 Match 不改写输入。
#[test]
fn TestCaseSensitive() {
    // ensure case-sensitive rules are really case-sensitive
    let case_rules = rules(&["~^FOO"], &[], vec![tb("~.*", "~FoO$")], vec![]);
    let r = New(true, Some(Box::new(case_rules))).expect("New should succeed");

    let input = vec![
        tb("FOO1", "a"),
        tb("foo2", "b"),
        tb("BoO3", "cFoO"),
        tb("Foo4", "dfoo"),
        tb("5", "5"),
    ];
    let actual = r.ApplyOn(input);
    let expected = vec![tb("foo2", "b"), tb("Foo4", "dfoo"), tb("5", "5")];
    assert_eq!(expected, actual);

    let inputTable = tb("FOO", "a");
    assert!(!r.Match(&inputTable));

    let bar_rules = rules(&[], &["BAR"], vec![], vec![]);
    let r = New(false, Some(Box::new(bar_rules))).expect("New should succeed");
    let inputTable = tb("bar", "a");
    assert!(r.Match(&inputTable));

    let inputTable = tb("BAR", "a");
    let originInputTable = inputTable.Clone();
    assert!(r.Match(&inputTable));
    // Match 只读缓存与规则，不应改写传入 Table。
    assert_eq!(inputTable, originInputTable);
}

/// 大小写不敏感的非 ASCII 字面量应按 Go `strings.ToLower` 后精确命中。
#[test]
fn TestCaseInsensitiveUnicodeLiteral() {
    let unicode_rules = rules(&[], &[], vec![], vec![tb("ÄBC", "TÖBL")]);
    let filter = New(false, Some(Box::new(unicode_rules))).expect("New should succeed");

    assert!(filter.Match(&tb("ÄBC", "TÖBL")));
    assert!(filter.Match(&tb("äbc", "töbl")));
}

/// 含正/负向前瞻的非法正则应使 `New` 返回错误。
#[test]
fn TestInvalidRegex() {
    let cases = vec![
        rules(&[], &["~^t[0-9]+((?!_copy).)*$"], vec![], vec![]),
        rules(&[], &["~^t[0-9]+sp(?=copy).*"], vec![], vec![]),
    ];

    for rules in cases {
        // Go regexp 不支持负向/正向前瞻；New 应把这些规则编译错误返回给调用者。
        let err = New(true, Some(Box::new(rules))).err();
        assert!(err.is_some());
    }
}

/// 校验 `Match` 返回 bool；`rules=None` 时不过滤、恒为 true。
#[test]
fn TestMatchReturnsBool() {
    let rules = rules(&[], &["sns"], vec![], vec![]);
    let f = New(true, Some(Box::new(rules))).expect("New should succeed");
    assert!(f.Match(&tb("sns", "")));
    assert!(!f.Match(&tb("other", "")));

    // nil rules 在 Go 中代表不过滤；Match 应直接返回 true。
    let f = New(true, None).expect("New should succeed");
    assert!(f.Match(&tb("other", "")));
}
