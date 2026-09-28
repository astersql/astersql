// Copyright 2022 PingCAP, Inc.
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

// table-router 单测：路由、规则增删改、大小写敏感与扩展列提取。
//
// 对应 Go `router_test.go`；验证 trie 选择器冲突、非法规则拒绝及提取器优先级。

// 这些测试只描述 table router 的路由、规则增删改、大小写敏感和扩展列提取语义；
//

use std::sync::Arc;

/// 单条路由期望：输入 schema/table 与目标 schema/table。
struct RouteCase {
    schema: &'static str,
    table: &'static str,
    target_schema: &'static str,
    target_table: &'static str,
}

// TestRoute 对应 Go 的大小写不敏感路由主流程测试。
/// 大小写不敏感：增删改、冲突、非法规则与无匹配回退。
#[test]
fn test_route() {
    let mut rules = vec![
        TableRule {
            SchemaPattern: "Test_1_*".to_string(),
            TablePattern: "abc*".to_string(),
            TargetSchema: "t1".to_string(),
            TargetTable: "abc".to_string(),
            ..Default::default()
        },
        TableRule {
            SchemaPattern: "test_1_*".to_string(),
            TablePattern: "test*".to_string(),
            TargetSchema: "t2".to_string(),
            TargetTable: "test".to_string(),
            ..Default::default()
        },
        TableRule {
            SchemaPattern: "test_1_*".to_string(),
            TablePattern: "".to_string(),
            TargetSchema: "test".to_string(),
            TargetTable: "".to_string(),
            ..Default::default()
        },
        TableRule {
            SchemaPattern: "test_2_*".to_string(),
            TablePattern: "abc*".to_string(),
            TargetSchema: "t1".to_string(),
            TargetTable: "abc".to_string(),
            ..Default::default()
        },
        TableRule {
            SchemaPattern: "test_2_*".to_string(),
            TablePattern: "test*".to_string(),
            TargetSchema: "t2".to_string(),
            TargetTable: "test".to_string(),
            ..Default::default()
        },
    ];

    let mut cases = vec![
        RouteCase {
            schema: "test_1_a",
            table: "abc1",
            target_schema: "t1",
            target_table: "abc",
        },
        RouteCase {
            schema: "test_2_a",
            table: "abc2",
            target_schema: "t1",
            target_table: "abc",
        },
        RouteCase {
            schema: "test_1_a",
            table: "test1",
            target_schema: "t2",
            target_table: "test",
        },
        RouteCase {
            schema: "test_2_a",
            table: "test2",
            target_schema: "t2",
            target_table: "test",
        },
        RouteCase {
            schema: "test_1_a",
            table: "xyz",
            target_schema: "test",
            target_table: "xyz",
        },
    ];

    // initial table router: Go 中 false 表示大小写不敏感。
    let mut router = NewTableRouter(false, rules.clone()).expect("new table router");

    // insert duplicate rules: 重复插入同一批规则应报错。
    for rule in &rules {
        assert!(router.AddRule(rule.clone()).is_err());
    }
    assert_route_cases(&router, &cases);

    // update rules: 修改第一条规则的目标表后更新，第一条 case 的结果随之变化。
    rules[0].TargetTable = "xxx".to_string();
    cases[0].target_table = "xxx";
    router.UpdateRule(rules[0].clone()).expect("update rule");
    assert_route_cases(&router, &cases);

    // remove rule: 删除后再删一次应报错，并回落到 schema 级规则。
    router.RemoveRule(rules[0].clone()).expect("remove rule");
    assert!(router.RemoveRule(rules[0].clone()).is_err());
    let (schema, table) = router
        .Route(cases[0].schema, cases[0].table)
        .expect("route after remove");
    assert_eq!("test", schema);
    assert_eq!("abc1", table);
    rules.remove(0);
    cases.remove(0);

    // mismatched: 没有匹配路由时保持原 schema。
    let (schema, _) = router.Route("test_3_a", "").expect("route mismatched");
    assert_eq!("test_3_a", schema);

    // test multiple schema level rules: 多条 schema 级规则命中时，Go 期望返回冲突错误。
    router
        .AddRule(TableRule {
            SchemaPattern: "test_*".to_string(),
            TablePattern: "".to_string(),
            TargetSchema: "error".to_string(),
            TargetTable: "".to_string(),
            ..Default::default()
        })
        .expect("add schema conflict");
    assert!(router.Route("test_1_a", "").is_err());

    // test multiple table level rules: 多条 table 级规则命中时也应报错。
    router
        .AddRule(TableRule {
            SchemaPattern: "test_1_*".to_string(),
            TablePattern: "tes*".to_string(),
            TargetSchema: "error".to_string(),
            TargetTable: "error".to_string(),
            ..Default::default()
        })
        .expect("add table conflict");
    assert!(router.Route("test_1_a", "test").is_err());

    // invalid rule: 直接向 Selector 插入非法目标，Route 应暴露错误。
    router
        .Selector
        .Insert(
            "test_1_*",
            "abc*",
            Some(Arc::new("error".to_string())),
            selector::Insert,
        )
        .expect("insert selector");
    assert!(router.Route("test_1_a", "abc").is_err());

    // Add/Update invalid table route rule: 目标 schema 缺失，Valid 阶段会拒绝。
    let invalid_rule = TableRule {
        SchemaPattern: "test*".to_string(),
        TablePattern: "abc*".to_string(),
        ..Default::default()
    };
    assert!(router.AddRule(invalid_rule.clone()).is_err());
    assert!(router.UpdateRule(invalid_rule).is_err());
}

// TestCaseSensitive 对应 Go 的大小写敏感路由测试。
/// 大小写敏感：`Test_1_*` 不匹配 `test_1_a` 的表级 abc 规则。
#[test]
fn test_case_sensitive() {
    let rules = vec![
        TableRule {
            SchemaPattern: "Test_1_*".to_string(),
            TablePattern: "abc*".to_string(),
            TargetSchema: "t1".to_string(),
            TargetTable: "abc".to_string(),
            ..Default::default()
        },
        TableRule {
            SchemaPattern: "test_1_*".to_string(),
            TablePattern: "test*".to_string(),
            TargetSchema: "t2".to_string(),
            TargetTable: "test".to_string(),
            ..Default::default()
        },
        TableRule {
            SchemaPattern: "test_1_*".to_string(),
            TablePattern: "".to_string(),
            TargetSchema: "test".to_string(),
            TargetTable: "".to_string(),
            ..Default::default()
        },
        TableRule {
            SchemaPattern: "test_2_*".to_string(),
            TablePattern: "abc*".to_string(),
            TargetSchema: "t1".to_string(),
            TargetTable: "abc".to_string(),
            ..Default::default()
        },
        TableRule {
            SchemaPattern: "test_2_*".to_string(),
            TablePattern: "test*".to_string(),
            TargetSchema: "t2".to_string(),
            TargetTable: "test".to_string(),
            ..Default::default()
        },
    ];

    let cases = vec![
        RouteCase {
            schema: "test_1_a",
            table: "abc1",
            target_schema: "test",
            target_table: "abc1",
        },
        RouteCase {
            schema: "test_2_a",
            table: "abc2",
            target_schema: "t1",
            target_table: "abc",
        },
        RouteCase {
            schema: "test_1_a",
            table: "test1",
            target_schema: "t2",
            target_table: "test",
        },
        RouteCase {
            schema: "test_2_a",
            table: "test2",
            target_schema: "t2",
            target_table: "test",
        },
        RouteCase {
            schema: "test_1_a",
            table: "xyz",
            target_schema: "test",
            target_table: "xyz",
        },
    ];

    // Go 中 true 表示大小写敏感，因此 Test_1_* 不会匹配 test_1_a 的 abc1。
    let mut router = NewTableRouter(true, rules.clone()).expect("new case-sensitive router");
    for rule in &rules {
        assert!(router.AddRule(rule.clone()).is_err());
    }
    assert_route_cases(&router, &cases);
}

// TestFetchExtendColumn 对应 Go 的扩展列提取测试。
/// 扩展列：表级规则优先，否则退回 schema 级提取器。
#[test]
fn test_fetch_extend_column() {
    let rules = vec![
        TableRule {
            SchemaPattern: "schema*".to_string(),
            TablePattern: "t*".to_string(),
            TargetSchema: "test".to_string(),
            TargetTable: "t".to_string(),
            TableExtractor: Some(Box::new(TableExtractor {
                TargetColumn: "table_name".to_string(),
                TableRegexp: "table_(.*)".to_string(),
                ..Default::default()
            })),
            SchemaExtractor: Some(Box::new(SchemaExtractor {
                TargetColumn: "schema_name".to_string(),
                SchemaRegexp: "schema_(.*)".to_string(),
                ..Default::default()
            })),
            SourceExtractor: Some(Box::new(SourceExtractor {
                TargetColumn: "source_name".to_string(),
                SourceRegexp: "source_(.*)_(.*)".to_string(),
                ..Default::default()
            })),
        },
        TableRule {
            SchemaPattern: "schema*".to_string(),
            TargetSchema: "test".to_string(),
            TargetTable: "t2".to_string(),
            SchemaExtractor: Some(Box::new(SchemaExtractor {
                TargetColumn: "schema_name".to_string(),
                SchemaRegexp: "(.*)".to_string(),
                ..Default::default()
            })),
            SourceExtractor: Some(Box::new(SourceExtractor {
                TargetColumn: "source_name".to_string(),
                SourceRegexp: "(.*)".to_string(),
                ..Default::default()
            })),
            ..Default::default()
        },
    ];
    let r = NewTableRouter(false, rules).expect("new router with extractors");
    let expected = vec![
        vec!["table_name", "schema_name", "source_name"],
        vec!["t1", "s1", "s1s1"],
        vec!["schema_name", "source_name"],
        vec!["schema_s2", "source_s2"],
    ];

    // table level rules have highest priority: 表级规则优先，所以同时返回 table/schema/source 三个扩展列。
    let (extend_col, extend_val) = r.FetchExtendColumn("schema_s1", "table_t1", "source_s1_s1");
    assert_eq!(expected[0], extend_col);
    assert_eq!(expected[1], extend_val);

    // only schema rules: 没有表级命中时，退回 schema 级规则。
    let (extend_col2, extend_val2) = r.FetchExtendColumn("schema_s2", "a_table_t2", "source_s2");
    assert_eq!(expected[2], extend_col2);
    assert_eq!(expected[3], extend_val2);
}

/// 逐条断言 Route 返回的目标 schema/table。
fn assert_route_cases(router: &TableRouter, cases: &[RouteCase]) {
    for cs in cases {
        // Go 循环中每个 case 都 require.NoError 后校验 schema/table 两个返回值。
        let (schema, table) = router.Route(cs.schema, cs.table).expect("route case");
        assert_eq!(cs.target_schema, schema);
        assert_eq!(cs.target_table, table);
    }
}

#[test]
fn test_go_simple_lowercase_lifecycle() {
    let mut lowered = TableRule {
        SchemaPattern: "İΟΣ".into(),
        TablePattern: "İΟΣ".into(),
        ..Default::default()
    };
    lowered.ToLower();
    assert_eq!(lowered.SchemaPattern, "iοσ");
    assert_eq!(lowered.TablePattern, "iοσ");
    // Use ASCII after folding here: Go's selector itself has byte/rune quirks
    // for literal multibyte patterns, independent of the router's case mapping.
    let mut rule = TableRule {
        SchemaPattern: "İ".into(),
        TablePattern: "İ".into(),
        TargetSchema: "target".into(),
        ..Default::default()
    };
    let mut router = NewTableRouter(false, vec![rule.clone()]).unwrap();
    assert_eq!(
        router.Route("i", "i").unwrap(),
        ("target".into(), "i".into())
    );
    assert_eq!(
        router.Route("İ", "İ").unwrap(),
        ("target".into(), "İ".into())
    );
    rule.TargetSchema = "updated".into();
    router.UpdateRule(rule.clone()).unwrap();
    assert_eq!(router.Route("i", "i").unwrap().0, "updated");
    router.RemoveRule(rule).unwrap();
    assert_eq!(router.Route("İ", "İ").unwrap().0, "İ");
}

#[test]
fn test_go_regexp_ascii_character_classes() {
    for (pattern, source, expected) in [
        (r"(\d+)", "１２3", "3"),
        (r"(\w+)", "中文abc", "abc"),
        (r"(\s+)", "\u{00a0} ", " "),
        (r"(\D+)", "１２3", "１２"),
        (r"(\W+)", "中文abc", "中文"),
        (r"(\S+)", "\u{00a0} ", "\u{00a0}"),
        (r"([\d]+)", "１２3", "3"),
        (r"(\\d)", r"\d", r"\d"),
    ] {
        let router = NewTableRouter(
            true,
            vec![TableRule {
                SchemaPattern: "s".into(),
                TargetSchema: "target".into(),
                SourceExtractor: Some(Box::new(SourceExtractor {
                    SourceRegexp: pattern.into(),
                    TargetColumn: "source".into(),
                    ..Default::default()
                })),
                ..Default::default()
            }],
        )
        .unwrap();
        assert_eq!(
            router.FetchExtendColumn("s", "t", source).1,
            vec![expected],
            "{pattern}"
        );
    }
}

#[test]
fn test_go_regexp_dialect_validation() {
    // Oracle: Go regexp.Compile; these differ from Rust regex's syntax contract.
    let mut differences = Vec::new();
    for (pattern, valid) in [
        (r"(?x)(a)", false),
        (r"(\141)", true),
        (r"(a{1001})", false),
        (r"((a{100}){100})", false),
        (r"a**", false),
        (r"a++", false),
        (r"a?*", false),
        (r"a{2}{3}", false),
        (r"a*(?i)*", true),
        (r"(?i:a)*(?i)??", true),
        (r"[[:foo:]]", false),
        (r"[a-\d]", false),
        (r"(?x:a)", false),
        (r"(?u:a)", false),
        (r"(?R:a)", false),
        (r"(a{10}){100}", true),
        (r"(a{1000}){0}", true),
    ] {
        let mut rule = TableRule {
            SchemaPattern: "s".into(),
            TargetSchema: "t".into(),
            SourceExtractor: Some(Box::new(SourceExtractor {
                SourceRegexp: pattern.into(),
                TargetColumn: "source".into(),
                ..Default::default()
            })),
            ..Default::default()
        };
        if rule.Valid().is_ok() != valid {
            differences.push((pattern, valid));
        }
    }
    assert!(
        differences.is_empty(),
        "Go regexp validity mismatches (pattern, expected valid): {differences:?}"
    );
}

#[test]
fn test_go_route_error_context() {
    let rule = TableRule {
        SchemaPattern: "S".into(),
        TargetSchema: "dst".into(),
        ..Default::default()
    };
    let text = "&{TableExtractor:<nil> SchemaExtractor:<nil> SourceExtractor:<nil> SchemaPattern:s TablePattern: TargetSchema:dst TargetTable:}";
    let mut router = NewTableRouter(false, vec![rule.clone()]).unwrap();
    let expected = format!(
        "add rule {text} into table router: insert into schema selector: pattern s already exists"
    );
    assert_eq!(router.AddRule(rule.clone()).unwrap_err(), expected);
    let error = NewTableRouter(false, vec![rule.clone(), rule])
        .err()
        .unwrap();
    assert_eq!(
        error,
        format!("initial rule {text} in table router: {expected}")
    );
    router
        .AddRule(TableRule {
            SchemaPattern: "*".into(),
            TargetSchema: "other".into(),
            ..Default::default()
        })
        .unwrap();
    assert!(
        router
            .Route("s", "")
            .unwrap_err()
            .ends_with("It's not supported")
    );
    router
        .Selector
        .Insert(
            "bad",
            "",
            Some(Arc::new("error".to_owned())),
            selector::Insert,
        )
        .unwrap();
    assert_eq!(
        router.Route("bad", "").unwrap_err(),
        "table route rule error not valid"
    );
}

#[test]
fn test_go_extractor_syntax_and_capture_oracle() {
    // Expected values obtained from Go 1.25.10 regexp.FindStringSubmatch.
    for (pattern, source, value) in [
        (r"(\12)", "\n", "\n"),
        (r"(\141)", "a", "a"),
        (r"(\Q(a)\E)", "(a)", "(a)"),
        (r"([a-z--b]+)", "3", "3"),
        (r"([a&&b]+)", "a&b", "a&b"),
        (r"([[a])", "[", "["),
        (r"([]a]+)", "]a", "]a"),
        (r"([\d-a]+)", "3-a", "3-a"),
        (r"(\b\w+\b)", "中文abc", "abc"),
        (r"(?ii)(a)", "A", "A"),
        (r"(?i-i)(a)", "A", ""),
        (r"(?)(a)", "a", "a"),
        (r"(?P<x>a)(?<x>b)", "ab", "ab"),
        (r"(a{01})", "a{01}", "a{01}"),
        (r"(a{,2})", "a{,2}", "a{,2}"),
        (r"(\x{D800})", "a", ""),
        (r"([\x{D800}-\x{DFFF}])", "a", ""),
        (r"(\p{Cs})", "a", ""),
        (r"(\p{Assigned}+)", "中文abc", "中文abc"),
        (r"(\p{upper-case letter})", "A", "A"),
        (r"(\p{LC}+)", "Ab", "Ab"),
        (r"(a)?(b)(c)?", "b", "b"),
        (r"(?U)(a+)", "aaa", "a"),
        (r"(?U)(a+?)", "aaa", "aaa"),
        (r"(?i)(a)(?-i)(b)", "Ab", "Ab"),
        (r"(?i)(a)(?-i)(b)", "AB", ""),
        (r"(?i:(a))b", "Ab", "A"),
        (r"((a|ab)*)", "abab", "aa"),
    ] {
        // Exercise all three extractor compilation paths and output ordering.
        let rule = TableRule {
            SchemaPattern: "*".into(),
            TargetSchema: "dst".into(),
            TableExtractor: Some(Box::new(TableExtractor {
                TableRegexp: pattern.into(),
                TargetColumn: "table".into(),
                ..Default::default()
            })),
            SchemaExtractor: Some(Box::new(SchemaExtractor {
                SchemaRegexp: pattern.into(),
                TargetColumn: "schema".into(),
                ..Default::default()
            })),
            SourceExtractor: Some(Box::new(SourceExtractor {
                SourceRegexp: pattern.into(),
                TargetColumn: "source".into(),
                ..Default::default()
            })),
            ..Default::default()
        };
        let router = NewTableRouter(true, vec![rule]).unwrap_or_else(|e| panic!("{pattern}: {e}"));
        let (columns, values) = router.FetchExtendColumn(source, source, source);
        assert_eq!(columns, ["table", "schema", "source"]);
        assert_eq!(values, [value, value, value], "{pattern}");
    }
}

#[test]
fn test_go_valid_partial_state_and_update_atomicity() {
    let mut rule = TableRule {
        SchemaPattern: "s".into(),
        TargetSchema: "dst".into(),
        TableExtractor: Some(Box::new(TableExtractor {
            TableRegexp: "(a)".into(),
            TargetColumn: "t".into(),
            ..Default::default()
        })),
        SchemaExtractor: Some(Box::new(SchemaExtractor {
            SchemaRegexp: "(".into(),
            TargetColumn: "s".into(),
            ..Default::default()
        })),
        SourceExtractor: Some(Box::new(SourceExtractor {
            SourceRegexp: "(c)".into(),
            TargetColumn: "c".into(),
            ..Default::default()
        })),
        ..Default::default()
    };
    assert_eq!(
        rule.Valid().unwrap_err(),
        "schema extractor schema regexp illegal ("
    );
    assert!(rule.TableExtractor.as_ref().unwrap().regexp.is_some());
    assert!(rule.SchemaExtractor.as_ref().unwrap().regexp.is_none());
    assert!(rule.SourceExtractor.as_ref().unwrap().regexp.is_none());
    rule.SchemaExtractor.as_mut().unwrap().SchemaRegexp = "(b)".into();
    rule.Valid().unwrap();
    let old_source = rule
        .SourceExtractor
        .as_ref()
        .unwrap()
        .regexp
        .as_ref()
        .unwrap()
        .as_str()
        .to_owned();
    let mut router = NewTableRouter(true, vec![rule.clone()]).unwrap();
    rule.SourceExtractor.as_mut().unwrap().SourceRegexp = "(".into();
    assert_eq!(
        rule.Valid().unwrap_err(),
        "source extractor source regexp illegal ("
    );
    assert_eq!(
        rule.SourceExtractor
            .as_ref()
            .unwrap()
            .regexp
            .as_ref()
            .unwrap()
            .as_str(),
        old_source
    );
    rule.TargetSchema = "changed".into();
    assert!(router.UpdateRule(rule).is_err());
    assert_eq!(router.Route("s", "t").unwrap().0, "dst");
}

#[test]
#[should_panic(expected = "extractor regexp must be initialized")]
fn test_go_unvalidated_extractor_panics() {
    let router = NewTableRouter(true, vec![]).unwrap();
    router
        .Selector
        .Insert(
            "s",
            "",
            Some(Arc::new(TableRule {
                SchemaPattern: "s".into(),
                TargetSchema: "t".into(),
                SourceExtractor: Some(Box::new(SourceExtractor::default())),
                ..Default::default()
            })),
            selector::Insert,
        )
        .unwrap();
    router.FetchExtendColumn("s", "t", "source");
}
