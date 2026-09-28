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

// table-rule-selector 迁移补充单元测试。
//
// 对齐 Go：Insert/Replace/Append、通配与 range 匹配、非法输入、
// Remove 与缓存失效、Match 返回独立切片，以及 Selector 的 Send+Sync。

use std::sync::Arc;

use super::{Append, Insert, NewTrieSelector, Replace, Rule, RuleSet, Selector};

/// 用描述字符串构造测试规则（`Arc<String>`）。
fn rule(description: &str) -> Rule {
    Arc::new(description.to_owned())
}

/// 提取规则描述并排序，便于与 Go 期望列表对照。
fn descriptions(rules: &RuleSet) -> Vec<String> {
    let mut descriptions: Vec<_> = rules
        .0
        .iter()
        .map(|rule| {
            rule.downcast_ref::<String>()
                .expect("test rules are strings")
                .clone()
        })
        .collect();
    descriptions.sort();
    descriptions
}

/// 按 `schema.table` 描述插入一条规则（Insert 模式）。
fn insert_rule(selector: &dyn Selector, schema: &str, table: &str) {
    selector
        .Insert(
            schema,
            table,
            Some(rule(&format!("{schema}.{table}"))),
            Insert,
        )
        .unwrap();
}

/// 覆盖重复 Insert 拒绝、Replace/Append 与 AllRules 枚举。
#[test]
fn migration_insert_replace_append_and_all_rules_match_go() {
    let selector = NewTrieSelector();
    insert_rule(selector.as_ref(), "schema*", "");
    assert!(
        selector
            .Insert("schema*", "", Some(rule("duplicate")), Insert)
            .is_err()
    );

    selector
        .Insert("schema*", "", Some(rule("replacement")), Replace)
        .unwrap();
    selector
        .Insert("schema*", "", Some(rule("append")), Append)
        .unwrap();
    insert_rule(selector.as_ref(), "schema*", "test*");

    let (schemas, tables) = selector.AllRules();
    assert_eq!(
        descriptions(schemas.get("schema*").unwrap()),
        vec!["append", "replacement"]
    );
    assert_eq!(
        descriptions(tables.get("schema*").unwrap().get("test*").unwrap()),
        vec!["schema*.test*"]
    );
}

/// 通配符、字符类 range 与 schema 级规则的 Match 结果对照 Go。
#[test]
fn migration_wildcards_ranges_and_schema_rules_match_go() {
    let selector = NewTrieSelector();
    let fixtures = [
        ("?bc", "t1_ab?"),
        ("?bc", "t1_abc"),
        ("ab*", "t4_abc"),
        ("ab*", "t4_abc*"),
        ("schema*", ""),
        ("schema*", "test*"),
        ("ik[hjkl]", "ik[!zxc]"),
        ("ik[f-h]", "ik[!a-ce-g]"),
        ("i[x-z][1-3]", "i?[x-z]"),
        ("i[x-z][1-3]", "ix*"),
        ("[!]", "[a-]"),
        ("[!]", "[a-c-f]"),
        ("[!a-c!f-g]", "*"),
    ];
    for (schema, table) in fixtures {
        insert_rule(selector.as_ref(), schema, table);
    }

    assert_eq!(
        descriptions(&selector.Match("dbc", "t1_abc")),
        vec!["?bc.t1_ab?", "?bc.t1_abc"]
    );
    assert_eq!(
        descriptions(&selector.Match("abc", "t4_abc")),
        vec!["ab*.t4_abc", "ab*.t4_abc*"]
    );
    assert_eq!(
        descriptions(&selector.Match("schema1", "test1")),
        vec!["schema*.", "schema*.test*"]
    );
    assert_eq!(
        descriptions(&selector.Match("ikh", "iky")),
        vec!["ik[f-h].ik[!a-ce-g]", "ik[hjkl].ik[!zxc]"]
    );
    assert_eq!(
        descriptions(&selector.Match("iz3", "ixz")),
        vec!["i[x-z][1-3].i?[x-z]", "i[x-z][1-3].ix*"]
    );
    assert_eq!(
        descriptions(&selector.Match("!", "-")),
        vec!["[!].[a-]", "[!].[a-c-f]"]
    );
    assert_eq!(
        descriptions(&selector.Match("d", "zxcv")),
        vec!["[!a-c!f-g].*"]
    );
}

/// 空 schema、空 rule、以及 `**`（星号非末位）应被拒绝。
#[test]
fn migration_invalid_inputs_and_star_position_match_go() {
    let selector = NewTrieSelector();
    assert!(selector.Insert("", "", Some(rule("x")), Insert).is_err());
    assert!(selector.Insert("schema", "", None, Replace).is_err());
    assert!(
        selector
            .Insert("ab**", "", Some(rule("x")), Replace)
            .is_err()
    );
    assert!(
        selector
            .Insert("abcd", "ab**", Some(rule("x")), Replace)
            .is_err()
    );
}

/// Remove 后 Match 结果变化，且重复 Remove 报错；缓存随之失效。
#[test]
fn migration_remove_and_cache_invalidation_match_go() {
    let selector = NewTrieSelector();
    insert_rule(selector.as_ref(), "schema*", "");
    insert_rule(selector.as_ref(), "schema*", "test*");

    assert_eq!(descriptions(&selector.Match("schema1", "test1")).len(), 2);
    selector.Remove("schema*", "test*").unwrap();
    assert_eq!(
        descriptions(&selector.Match("schema1", "test1")),
        vec!["schema*."]
    );
    assert!(selector.Remove("schema*", "test*").is_err());
    selector.Remove("schema*", "").unwrap();
    assert!(selector.Match("schema1", "test1").0.is_empty());
    assert!(selector.Remove("schema*", "").is_err());
}

/// Match 返回独立 RuleSet：清空调用方副本不影响后续 Match。
#[test]
fn migration_match_returns_an_independent_rule_slice() {
    let selector = NewTrieSelector();
    insert_rule(selector.as_ref(), "schema*", "");

    let mut first = selector.Match("schema1", "");
    first.0.clear();
    assert_eq!(
        descriptions(&selector.Match("schema1", "")),
        vec!["schema*."]
    );
}

/// Selector 实现 Send+Sync，多线程并发 Insert/Match 安全。
#[test]
fn migration_selector_is_send_and_sync_like_go_mutex_selector() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Box<dyn Selector>>();

    let selector = Arc::new(NewTrieSelector());
    let mut threads = Vec::new();
    for i in 0..8 {
        let selector = Arc::clone(&selector);
        threads.push(std::thread::spawn(move || {
            let schema = format!("schema{i}");
            selector
                .Insert(&schema, "", Some(rule(&schema)), Insert)
                .unwrap();
            assert_eq!(descriptions(&selector.Match(&schema, "")), vec![schema]);
        }));
    }
    for thread in threads {
        thread.join().unwrap();
    }

    let (schemas, tables) = selector.AllRules();
    assert_eq!(schemas.len(), 8);
    assert!(tables.is_empty());
}
