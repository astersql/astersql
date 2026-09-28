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

// 表规则选择器主流程单测：插入、匹配、追加、替换与删除。
//
// 对应 Go `selector_test.go`。用固定 fixture 覆盖 `*`/`?`/`[range]`，
// 并校验 Match 缓存与 AllRules 快照。

use super::*;
use std::collections::HashMap;
use std::sync::Arc;

/// 选择器测试套件：fixture 表、匹配用例、删除用例与期望规则图。
struct test_selector_suite {
    /// schema -> 其下 table pattern 列表（空串表示 schema 级）。
    tables: HashMap<&'static str, Vec<&'static str>>,
    /// Match 期望用例列表。
    match_case: Vec<match_case>,
    /// 成对 (schema, table) 删除序列。
    remove_cases: Vec<&'static str>,
    /// 期望的 schema 级规则图。
    expected_schema_rules: HashMap<String, RuleSet>,
    /// 期望的 table 级规则图：schema -> table -> rules。
    expected_table_rules: HashMap<String, HashMap<String, RuleSet>>,
}

/// 单条 Match 期望：输入、命中条数与交替的 schema/table pattern 列表。
struct match_case {
    schema: &'static str,
    table: &'static str,
    matched_num: usize,
    matched_rules: Vec<&'static str>,
}

impl match_case {
    /// 构造匹配用例；`matched_rules` 按 (schema, table) 交替存放。
    fn new(
        schema: &'static str,
        table: &'static str,
        matched_num: usize,
        matched_rules: &[&'static str],
    ) -> Self {
        Self {
            schema,
            table,
            matched_num,
            matched_rules: matched_rules.to_vec(),
        }
    }
}

impl test_selector_suite {
    /// 填充与 Go 测试一致的通配/range fixture 与匹配/删除用例。
    fn new() -> Self {
        Self {
            tables: HashMap::from([
                ("t*", vec!["test*"]),
                ("schema*", vec!["", "test*", "abc*", "xyz"]),
                ("?bc", vec!["t1_abc", "t1_ab?", "abc*"]),
                ("a?c", vec!["t2_abc", "t2_ab*", "a?b"]),
                ("ab?", vec!["t3_ab?", "t3_ab*", "ab?"]),
                ("ab*", vec!["t4_abc", "t4_abc*", "ab*"]),
                ("abc", vec!["abc"]),
                ("abd", vec!["abc"]),
                ("ik[hjkl]", vec!["ik[!zxc]"]),
                ("ik[f-h]", vec!["ik[!a-ce-g]"]),
                ("i[x-z][1-3]", vec!["i?[x-z]", "ix*"]),
                // [\!\-\!], [a-a\--\-], [a-c\--\-f-f].
                ("[!]", vec!["[a-]", "[a-c-f]"]),
                // [!a-c\!\-\!f-g]
                ("[!a-c!f-g]", vec!["*"]),
                // [] match nothing.
                ("[]*", vec!["*"]),
            ]),
            match_case: vec![
                match_case::new("dbc", "t1_abc", 2, &["?bc", "t1_ab?", "?bc", "t1_abc"]),
                match_case::new("adc", "t2_abc", 2, &["a?c", "t2_ab*", "a?c", "t2_abc"]),
                match_case::new("abd", "t3_abc", 2, &["ab?", "t3_ab*", "ab?", "t3_ab?"]),
                match_case::new("abc", "t4_abc", 2, &["ab*", "t4_abc", "ab*", "t4_abc*"]),
                match_case::new(
                    "abc",
                    "abc",
                    4,
                    &["?bc", "abc*", "ab*", "ab*", "ab?", "ab?", "abc", "abc"],
                ),
                match_case::new("schema1", "xxx", 1, &["schema*", ""]),
                match_case::new("schema1", "", 1, &["schema*", ""]),
                match_case::new("schema1", "test1", 2, &["schema*", "", "schema*", "test*"]),
                match_case::new("t1", "test1", 1, &["t*", "test*"]),
                match_case::new("schema1", "abc1", 2, &["schema*", "", "schema*", "abc*"]),
                match_case::new("ikj", "ikb", 1, &["ik[hjkl]", "ik[!zxc]"]),
                match_case::new(
                    "ikh",
                    "iky",
                    2,
                    &["ik[hjkl]", "ik[!zxc]", "ik[f-h]", "ik[!a-ce-g]"],
                ),
                match_case::new(
                    "iz3",
                    "ixz",
                    2,
                    &["i[x-z][1-3]", "i?[x-z]", "i[x-z][1-3]", "ix*"],
                ),
                match_case::new("!", "-", 2, &["[!]", "[a-]", "[!]", "[a-c-f]"]),
                match_case::new("!", "c", 1, &["[!]", "[a-c-f]"]),
                match_case::new("d", "zxcv", 1, &["[!a-c!f-g]", "*"]),
            ],
            remove_cases: vec![
                "schema*",
                "",
                "a?c",
                "t2_ab*",
                "i[x-z][1-3]",
                "i?[x-z]",
                "[!]",
                "[a-c-f]",
            ],
            expected_schema_rules: HashMap::new(),
            expected_table_rules: HashMap::new(),
        }
    }
}

#[derive(Debug)]
/// 测试用规则载荷，仅携带可读 description。
struct dummyRule {
    description: String,
}

/// 构造带 description 的 dummyRule 规则。
fn rule(description: impl Into<String>) -> Rule {
    Arc::new(dummyRule {
        description: description.into(),
    })
}

/// 提取并排序规则描述，便于断言。
fn descriptions(rules: &RuleSet) -> Vec<String> {
    let mut descriptions: Vec<_> = rules
        .0
        .iter()
        .map(|rule| {
            rule.downcast_ref::<dummyRule>()
                .expect("all selector test rules are dummyRule")
                .description
                .clone()
        })
        .collect();
    descriptions.sort();
    descriptions
}

/// 比较两张 schema/table pattern -> RuleSet 图是否描述一致。
fn assert_rule_maps_equal(actual: &HashMap<String, RuleSet>, expected: &HashMap<String, RuleSet>) {
    assert_eq!(actual.len(), expected.len());
    for (pattern, expected_rules) in expected {
        let actual_rules = actual
            .get(pattern)
            .unwrap_or_else(|| panic!("missing rule pattern {pattern}"));
        assert_eq!(descriptions(actual_rules), descriptions(expected_rules));
    }
}

/// 比较嵌套的 schema -> table -> RuleSet 图。
fn assert_table_rule_maps_equal(
    actual: &HashMap<String, HashMap<String, RuleSet>>,
    expected: &HashMap<String, HashMap<String, RuleSet>>,
) {
    assert_eq!(actual.len(), expected.len());
    for (schema, expected_tables) in expected {
        let actual_tables = actual
            .get(schema)
            .unwrap_or_else(|| panic!("missing schema pattern {schema}"));
        assert_rule_maps_equal(actual_tables, expected_tables);
    }
}

/// 主入口：依次跑 Insert → Match → Append → Replace → Remove。
#[test]
fn TestSelector() {
    let mut ts = test_selector_suite::new();
    let s = trieSelector::new_empty();
    (ts.expected_schema_rules, ts.expected_table_rules) = testGenerateExpectedRules(&ts);

    testInsert(&ts, &s);
    testMatch(&ts, &s);
    testAppend(&mut ts, &s);
    testReplace(&mut ts, &s);
    testRemove(&mut ts, &s);
}

/// 插入期望规则：重复 Insert 失败、Replace 成功，并校验 AllRules。
fn testInsert(ts: &test_selector_suite, s: &trieSelector) {
    for (schema, rules) in &ts.expected_schema_rules {
        s.Insert(schema, "", Some(rules.0[0].clone()), Insert)
            .unwrap();
        assert!(
            s.Insert(schema, "", Some(rules.0[0].clone()), Insert)
                .is_err()
        );
        s.Insert(schema, "", Some(rules.0[0].clone()), Replace)
            .unwrap();
    }

    for (schema, tables) in &ts.expected_table_rules {
        for (table, rules) in tables {
            s.Insert(schema, table, Some(rules.0[0].clone()), Insert)
                .unwrap();
            assert!(
                s.Insert(schema, table, Some(rules.0[0].clone()), Insert)
                    .is_err()
            );
            s.Insert(schema, table, Some(rules.0[0].clone()), Replace)
                .unwrap();
        }
    }

    assert!(s.Insert("schema", "", None, Replace).is_err());
    assert!(s.Insert("ab**", "", Some(rule("error")), Replace).is_err());
    assert!(
        s.Insert("abcd", "ab**", Some(rule("error")), Replace)
            .is_err()
    );

    let (schemas, tables) = s.AllRules();
    assert_rule_maps_equal(&schemas, &ts.expected_schema_rules);
    assert_table_rule_maps_equal(&tables, &ts.expected_table_rules);
}

/// 按 remove_cases 成对删除，并同步更新期望规则图。
fn testRemove(ts: &mut test_selector_suite, s: &trieSelector) {
    for pair in ts.remove_cases.chunks_exact(2) {
        let (schema, table) = (pair[0], pair[1]);
        s.Remove(schema, table).unwrap();
        assert!(s.Remove(schema, table).is_err());

        if table.is_empty() {
            ts.expected_schema_rules.remove(schema);
        } else {
            ts.expected_table_rules
                .get_mut(schema)
                .expect("schema exists")
                .remove(table);
        }
    }

    let (schemas, tables) = s.AllRules();
    assert_rule_maps_equal(&schemas, &ts.expected_schema_rules);
    assert_table_rule_maps_equal(&tables, &ts.expected_table_rules);
}

/// 对所有 schema 级规则 Append 一条描述为 "append" 的规则。
fn testAppend(ts: &mut test_selector_suite, s: &trieSelector) {
    let appended_rule = rule("append");
    for (schema, rules) in &mut ts.expected_schema_rules {
        rules.0.push(appended_rule.clone());
        s.Insert(schema, "", Some(appended_rule.clone()), Append)
            .unwrap();
    }

    let (schemas, tables) = s.AllRules();
    assert_rule_maps_equal(&schemas, &ts.expected_schema_rules);
    assert_table_rule_maps_equal(&tables, &ts.expected_table_rules);
}

/// 用 Replace 覆盖 schema 级规则为单条 "replace"。
fn testReplace(ts: &mut test_selector_suite, s: &trieSelector) {
    let replaced_rule = rule("replace");
    for (schema, rules) in &mut ts.expected_schema_rules {
        *rules = RuleSet(vec![replaced_rule.clone()]);
        s.Insert(schema, "", Some(replaced_rule.clone()), Replace)
            .unwrap();
        s.Insert(schema, "", Some(replaced_rule.clone()), Replace)
            .unwrap();
        assert!(
            s.Insert(schema, "", Some(replaced_rule.clone()), Insert)
                .is_err()
        );
    }

    let (schemas, tables) = s.AllRules();
    assert_rule_maps_equal(&schemas, &ts.expected_schema_rules);
    assert_table_rule_maps_equal(&tables, &ts.expected_table_rules);
}

/// 按 match_case 断言 Match，并核对内部 cache 与未命中条目。
fn testMatch(ts: &test_selector_suite, s: &trieSelector) {
    let mut expected_cache = HashMap::new();
    for mc in &ts.match_case {
        let actual = s.Match(mc.schema, mc.table);
        let expected = RuleSet(
            (0..mc.matched_num)
                .map(|i| {
                    rule(quoteSchemaTable(
                        mc.matched_rules[2 * i],
                        mc.matched_rules[2 * i + 1],
                    ))
                })
                .collect(),
        );
        assert_eq!(descriptions(&actual), descriptions(&expected));
        expected_cache.insert(quoteSchemaTable(mc.schema, mc.table), expected);
    }

    let cache = s.cache.read().expect("cache read lock");
    assert_rule_maps_equal(&cache, &expected_cache);
    drop(cache);

    for (schema, table) in [("t1", ""), ("t1", "abc"), ("xxx", "abc")] {
        let actual = s.Match(schema, table);
        assert!(actual.0.is_empty());
        expected_cache.insert(quoteSchemaTable(schema, table), actual);
    }

    let cache = s.cache.read().expect("cache read lock");
    assert_rule_maps_equal(&cache, &expected_cache);
}

/// 由 fixture `tables` 生成期望的 schema/table 规则图。
fn testGenerateExpectedRules(
    ts: &test_selector_suite,
) -> (
    HashMap<String, RuleSet>,
    HashMap<String, HashMap<String, RuleSet>>,
) {
    let mut schema_rules = HashMap::new();
    let mut table_rules: HashMap<String, HashMap<String, RuleSet>> = HashMap::new();
    for (schema, tables) in &ts.tables {
        table_rules.entry((*schema).to_owned()).or_default();
        for table in tables {
            if table.is_empty() {
                schema_rules.insert(
                    (*schema).to_owned(),
                    RuleSet(vec![rule(quoteSchemaTable(schema, ""))]),
                );
            } else {
                table_rules.entry((*schema).to_owned()).or_default().insert(
                    (*table).to_owned(),
                    RuleSet(vec![rule(quoteSchemaTable(schema, table))]),
                );
            }
        }
    }
    (schema_rules, table_rules)
}
