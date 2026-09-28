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

// table-router 迁移对照单测：路由生命周期、冲突与扩展列抽取。
//
// 与 Go 对齐：规则优先级、大小写敏感、非法值拒绝，以及正则捕获拼接。

use crate::router::*;
use std::sync::Arc;

/// 构造仅含模式与目标名的简易 `TableRule`。
fn rule(schema: &str, table: &str, target_schema: &str, target_table: &str) -> TableRule {
    TableRule {
        SchemaPattern: schema.into(),
        TablePattern: table.into(),
        TargetSchema: target_schema.into(),
        TargetTable: target_table.into(),
        ..Default::default()
    }
}

#[test]
/// 增删改规则与匹配优先级对照 Go。
fn route_rule_lifecycle_and_priority_matches_go() {
    let mut rules = vec![
        rule("Test_1_*", "abc*", "t1", "abc"),
        rule("test_1_*", "test*", "t2", "test"),
        rule("test_1_*", "", "test", ""),
        rule("test_2_*", "abc*", "t1", "abc"),
        rule("test_2_*", "test*", "t2", "test"),
    ];
    let mut router = NewTableRouter(false, rules.clone()).unwrap();

    let cases = [
        ("test_1_a", "abc1", "t1", "abc"),
        ("test_2_a", "abc2", "t1", "abc"),
        ("test_1_a", "test1", "t2", "test"),
        ("test_2_a", "test2", "t2", "test"),
        ("test_1_a", "xyz", "test", "xyz"),
    ];
    for existing_rule in &rules {
        assert!(router.AddRule(existing_rule.clone()).is_err());
    }
    for (schema, table, target_schema, target_table) in cases {
        assert_eq!(
            router.Route(schema, table).unwrap(),
            (target_schema.into(), target_table.into())
        );
    }

    let mut updated = rules[0].clone();
    updated.TargetTable = "xxx".into();
    router.UpdateRule(updated.clone()).unwrap();
    assert_eq!(router.Route("test_1_a", "abc1").unwrap().1, "xxx");

    router.RemoveRule(updated.clone()).unwrap();
    assert!(router.RemoveRule(updated).is_err());
    assert_eq!(
        router.Route("test_1_a", "abc1").unwrap(),
        ("test".into(), "abc1".into())
    );

    rules.remove(0);
    assert_eq!(
        router.Route("test_3_a", "").unwrap(),
        ("test_3_a".into(), "".into())
    );

    router.AddRule(rule("test_*", "", "error", "")).unwrap();
    assert!(router.Route("test_1_a", "").is_err());

    router
        .AddRule(rule("test_1_*", "tes*", "error", "error"))
        .unwrap();
    assert!(router.Route("test_1_a", "test").is_err());

    router
        .Selector
        .Insert(
            "test_1_*",
            "abc*",
            Some(Arc::new("error".to_string())),
            crate::selector::Insert,
        )
        .unwrap();
    assert!(router.Route("test_1_a", "abc").is_err());

    let invalid_rule = rule("test*", "abc*", "", "");
    assert!(router.AddRule(invalid_rule.clone()).is_err());
    assert!(router.UpdateRule(invalid_rule).is_err());
}

#[test]
/// 大小写敏感冲突、空模式/目标及非法正则应失败。
fn conflicts_case_sensitivity_and_invalid_values_match_go() {
    let rules = vec![
        rule("Test_1_*", "abc*", "t1", "abc"),
        rule("test_1_*", "test*", "t2", "test"),
        rule("test_1_*", "", "test", ""),
        rule("test_2_*", "abc*", "t1", "abc"),
        rule("test_2_*", "test*", "t2", "test"),
    ];
    let mut sensitive = NewTableRouter(true, rules.clone()).unwrap();
    let cases = [
        ("test_1_a", "abc1", "test", "abc1"),
        ("test_2_a", "abc2", "t1", "abc"),
        ("test_1_a", "test1", "t2", "test"),
        ("test_2_a", "test2", "t2", "test"),
        ("test_1_a", "xyz", "test", "xyz"),
    ];
    for existing_rule in &rules {
        assert!(sensitive.AddRule(existing_rule.clone()).is_err());
    }
    for (schema, table, target_schema, target_table) in cases {
        assert_eq!(
            sensitive.Route(schema, table).unwrap(),
            (target_schema.into(), target_table.into())
        );
    }

    sensitive.AddRule(rule("test_*", "", "other", "")).unwrap();
    assert!(sensitive.Route("test_1_a", "").is_err());
    assert!(NewTableRouter(false, vec![rule("", "x", "target", "")]).is_err());
    assert!(NewTableRouter(false, vec![rule("schema", "x", "", "")]).is_err());

    let mut bad_regex = rule("schema", "x", "target", "");
    bad_regex.TableExtractor = Some(Box::new(TableExtractor {
        TargetColumn: "c".into(),
        TableRegexp: "[".into(),
        ..Default::default()
    }));
    assert!(NewTableRouter(false, vec![bad_regex]).is_err());
}

#[test]
/// FetchExtendColumn 将多捕获组拼接为扩展列值。
fn fetch_extend_columns_matches_go_capture_concatenation() {
    let mut table_rule = rule("schema*", "t*", "test", "t");
    table_rule.TableExtractor = Some(Box::new(TableExtractor {
        TargetColumn: "table_name".into(),
        TableRegexp: "table_(.*)".into(),
        ..Default::default()
    }));
    table_rule.SchemaExtractor = Some(Box::new(SchemaExtractor {
        TargetColumn: "schema_name".into(),
        SchemaRegexp: "schema_(.*)".into(),
        ..Default::default()
    }));
    table_rule.SourceExtractor = Some(Box::new(SourceExtractor {
        TargetColumn: "source_name".into(),
        SourceRegexp: "source_(.*)_(.*)".into(),
        ..Default::default()
    }));
    let mut schema_rule = rule("schema*", "", "test", "t2");
    schema_rule.SchemaExtractor = Some(Box::new(SchemaExtractor {
        TargetColumn: "schema_name".into(),
        SchemaRegexp: "(.*)".into(),
        ..Default::default()
    }));
    schema_rule.SourceExtractor = Some(Box::new(SourceExtractor {
        TargetColumn: "source_name".into(),
        SourceRegexp: "(.*)".into(),
        ..Default::default()
    }));
    let router = NewTableRouter(false, vec![table_rule, schema_rule]).unwrap();

    let (columns, values) = router.FetchExtendColumn("schema_s1", "table_t1", "source_s1_s1");
    assert_eq!(columns, ["table_name", "schema_name", "source_name"]);
    assert_eq!(values, ["t1", "s1", "s1s1"]);

    let (columns, values) = router.FetchExtendColumn("schema_s2", "a_table_t2", "source_s2");
    assert_eq!(columns, ["schema_name", "source_name"]);
    assert_eq!(values, ["schema_s2", "source_s2"]);
}
