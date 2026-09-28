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

// Filter 迁移回归单测。
//
// 对应 Go `pkg/util/filter` 的规则优先级、正则大小写、Apply/ApplyOn 大小写保留、
// 非法规则拒绝、空规则与系统库判定等行为，确保 Rust 迁移稿与 Go 语义一致。

use util_filter::{Filter, IsSystemSchema, New, Rules, Table};

/// 构造 `Box<Table>`，便于在规则与输入列表中复用。
fn table(schema: &str, name: &str) -> Box<Table> {
    Box::new(Table::new(schema, name))
}

/// 抽出 `(Schema, Name)` 便于断言过滤结果顺序与内容。
fn names(tables: &[Box<Table>]) -> Vec<(&str, &str)> {
    tables
        .iter()
        .map(|table| (table.Schema.as_str(), table.Name.as_str()))
        .collect()
}

/// 校验库/表 Do/Ignore 规则的优先级与通配匹配是否与 Go 一致。
#[test]
fn migration_schema_and_table_precedence_matches_go() {
    // DoDBs/IgnoreDBs 与 DoTables/IgnoreTables 交叉时，按 Go 的过滤优先级取舍。
    let rules = Rules {
        DoDBs: vec!["foo*".into()],
        IgnoreDBs: vec!["foo1".into()],
        DoTables: vec![table("foo*", "bar?")],
        IgnoreTables: vec![table("foo1", "bar1")],
    };
    let filter = New(false, Some(Box::new(rules))).unwrap();
    let input = vec![
        table("FOO1", "BAR1"),
        table("foo2", "bar2"),
        table("foo2", "other"),
        table("else", "bar2"),
        table("foo2", ""),
    ];

    let output = filter.Apply(input);
    assert_eq!(
        names(&output),
        vec![("FOO1", "BAR1"), ("foo2", "bar2"), ("foo2", "")]
    );
}

/// 校验 `~` 正则规则组合，以及大小写敏感开关对 Match 的影响。
#[test]
fn migration_regex_combinations_and_case_sensitivity_match_go() {
    let rules = Rules {
        IgnoreTables: vec![
            table("~^prod[0-8]$", "event??"),
            table("archive", "~^tmp[0-9]+$"),
            table("~^raw", "~^drop_[0-9]+$"),
        ],
        ..Default::default()
    };
    // case_sensitive=true：大小写不同则不命中 Ignore，从而 Match 为 true。
    let filter = New(true, Some(Box::new(rules))).unwrap();

    assert!(!filter.Match(&Table::new("prod1", "event01")));
    assert!(filter.Match(&Table::new("PROD1", "event01")));
    assert!(!filter.Match(&Table::new("archive", "tmp42")));
    assert!(!filter.Match(&Table::new("raw_data", "drop_7")));
    assert!(filter.Match(&Table::new("raw_data", "keep_7")));
}

/// 校验 Apply 保留原始大小写，ApplyOn 会规范化为小写（与 Go 一致）。
#[test]
fn migration_apply_on_clones_and_apply_preserves_original_case() {
    let rules = Rules {
        DoDBs: vec!["allowed".into()],
        ..Default::default()
    };
    let filter = New(false, Some(Box::new(rules))).unwrap();

    let applied = filter.Apply(vec![table("ALLOWED", "Mixed")]);
    assert_eq!(names(&applied), vec![("ALLOWED", "Mixed")]);

    let applied_on = filter.ApplyOn(vec![table("ALLOWED", "Mixed")]);
    assert_eq!(names(&applied_on), vec![("allowed", "mixed")]);
}

/// 校验空规则与含 look-around 的非法正则会被 New 拒绝。
#[test]
fn migration_invalid_rules_and_regex_are_rejected() {
    let empty = Rules {
        DoDBs: vec![String::new()],
        ..Default::default()
    };
    assert!(New(true, Some(Box::new(empty))).is_err());

    // Go 侧同样拒绝带前瞻/后瞻的正则，避免不可移植的匹配语义。
    let look_around = Rules {
        DoDBs: vec!["~^t[0-9]+(?=copy)$".into()],
        ..Default::default()
    };
    assert!(New(true, Some(Box::new(look_around))).is_err());
}

/// 校验空规则全通过、结果缓存命中，以及系统 schema（含 DM/巡检库）判定。
#[test]
fn migration_nil_rules_cache_and_system_schemas_match_go() {
    // rules=None 时等价于不过滤，任意表均 Match。
    let filter = New(true, None).unwrap();
    assert!(filter.Match(&Table::new("anything", "table")));

    let rules = Rules {
        DoDBs: vec!["sns".into()],
        ..Default::default()
    };
    let filter: Box<Filter> = New(true, Some(Box::new(rules))).unwrap();
    // 连续两次 Match 同一表，覆盖缓存命中路径。
    assert!(filter.Match(&Table::new("sns", "first")));
    assert!(filter.Match(&Table::new("sns", "first")));
    assert!(!filter.Match(&Table::new("other", "first")));

    // 系统库：MySQL/TiDB 内置库 + DM 心跳库 + 巡检库。
    for schema in [
        "information_schema",
        "performance_schema",
        "metrics_schema",
        "mysql",
        "sys",
        "dm_heartbeat",
        "inspection_schema",
    ] {
        assert!(IsSystemSchema(schema), "{schema}");
    }
    assert!(!IsSystemSchema("not_system_schema"));
}

/// Go `strings.ToLower` lowercases Unicode identifiers, not only ASCII.
#[test]
fn migration_case_insensitive_matching_lowercases_unicode() {
    let rules = Rules {
        DoTables: vec![table("ÄBC", "TÖBL")],
        ..Default::default()
    };
    let filter = New(false, Some(Box::new(rules))).unwrap();

    assert!(filter.Match(&Table::new("ÄBC", "TÖBL")));
    assert!(filter.Match(&Table::new("äbc", "töbl")));
}
