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

// regexpr-router 与旧 table-router 对照的单元测试。
//
// 覆盖创建、AddRule、库级/表级/正则路由、扩展列提取、AllRules 分类，
// 以及重复匹配报错；部分用例同时跑旧 NewTableRouter 与 NewRegExprRouter。

use regexpr_router::NewRegExprRouter;
use router_crate::router::{self, SchemaExtractor, SourceExtractor, TableExtractor, TableRule};

/// 构造简易 TableRule。
fn rule(schema: &str, table: &str, target_schema: &str, target_table: &str) -> TableRule {
    TableRule {
        SchemaPattern: schema.into(),
        TablePattern: table.into(),
        TargetSchema: target_schema.into(),
        TargetTable: target_table.into(),
        ..Default::default()
    }
}

/// 构造表名提取器。
fn table_extractor(column: &str, regexp: &str) -> TableExtractor {
    let mut extractor = TableExtractor::default();
    extractor.TargetColumn = column.into();
    extractor.TableRegexp = regexp.into();
    extractor
}

/// 构造库名提取器。
fn schema_extractor(column: &str, regexp: &str) -> SchemaExtractor {
    let mut extractor = SchemaExtractor::default();
    extractor.TargetColumn = column.into();
    extractor.SchemaRegexp = regexp.into();
    extractor
}

/// 构造 source 提取器。
fn source_extractor(column: &str, regexp: &str) -> SourceExtractor {
    let mut extractor = SourceExtractor::default();
    extractor.TargetColumn = column.into();
    extractor.SourceRegexp = regexp.into();
    extractor
}

// TestCreateRouter
/// 验证空规则列表可成功创建大小写敏感/不敏感路由表。
#[test]
fn test_create_router() {
    assert!(NewRegExprRouter(true, vec![]).is_ok());
    assert!(NewRegExprRouter(false, vec![]).is_ok());
}

// TestAddRule
/// 验证向空路由表追加库级与表级规则成功。
#[test]
fn test_add_rule() {
    let rules = vec![
        rule("test1", "", "dtest1", ""),
        rule("test2", "table2", "dtest2", "dtable2"),
    ];

    let mut case_sensitive = NewRegExprRouter(true, vec![]).unwrap();
    for route_rule in &rules {
        assert!(case_sensitive.AddRule(route_rule.clone()).is_ok());
    }

    let mut case_insensitive = NewRegExprRouter(false, vec![]).unwrap();
    for route_rule in &rules {
        assert!(case_insensitive.AddRule(route_rule.clone()).is_ok());
    }
}

// TestSchemaRoute
/// 验证库级路由结果与旧 NewTableRouter 一致。
#[test]
fn test_schema_route() {
    let rules = vec![
        rule("test1", "", "dtest1", ""),
        rule("gtest*", "", "dtest", ""),
    ];
    let old_router = router::NewTableRouter(true, rules.clone()).unwrap();
    let new_router = NewRegExprRouter(true, rules).unwrap();
    let cases = [
        (("test1", "table1"), ("dtest1", "table1")),
        (("gtesttest", "atable"), ("dtest", "atable")),
        (("ptest", "atableg"), ("ptest", "atableg")),
    ];

    for ((schema, table), (expected_schema, expected_table)) in cases {
        let old_result = old_router.Route(schema, table).unwrap();
        let new_result = new_router.Route(schema, table).unwrap();
        assert_eq!(old_result, (expected_schema.into(), expected_table.into()));
        assert_eq!(new_result, (expected_schema.into(), expected_table.into()));
    }
}

// TestTableRoute
/// 验证表级路由结果与旧 NewTableRouter 一致。
#[test]
fn test_table_route() {
    let rules = vec![
        rule("test1", "table1", "dtest1", "dtable1"),
        rule("test*", "table2", "dtest2", "dtable2"),
        rule("test3", "table*", "dtest3", "dtable3"),
    ];
    let old_router = router::NewTableRouter(true, rules.clone()).unwrap();
    let new_router = NewRegExprRouter(true, rules).unwrap();

    for i in 1..=3 {
        let schema = format!("test{i}");
        let table = format!("table{i}");
        let expected = (format!("dtest{i}"), format!("dtable{i}"));
        assert_eq!(old_router.Route(&schema, &table).unwrap(), expected);
        assert_eq!(new_router.Route(&schema, &table).unwrap(), expected);
    }
}

// TestRegExprRoute
/// 验证正则与 glob 混用时的目标库表映射。
#[test]
fn test_reg_expr_route() {
    let rules = vec![
        rule("~test.[0-9]+", "", "dtest1", ""),
        rule(
            "~test2?[animal|human]",
            "~tbl.*[cat|dog]+",
            "dtest2",
            "dtable2",
        ),
        rule("~test3_(schema)?.*", "test3_*", "dtest3", "dtable3"),
        rule(
            "test4s_*",
            "~testtable_[donot_delete]?",
            "dtest4",
            "dtable4",
        ),
    ];
    let input = [
        ("tests100", "table1"),
        ("test2animal", "tbl_animal_dogcat"),
        ("test3_schema_meta", "test3_tail"),
        ("test4s_2022", "testtable_donot_delete"),
        ("mytst5566", "gtable"),
    ];
    let expected = [
        ("dtest1", "table1"),
        ("dtest2", "dtable2"),
        ("dtest3", "dtable3"),
        ("dtest4", "dtable4"),
        ("mytst5566", "gtable"),
    ];
    let new_router = NewRegExprRouter(true, rules).unwrap();

    for ((schema, table), (expected_schema, expected_table)) in input.into_iter().zip(expected) {
        assert_eq!(
            new_router.Route(schema, table).unwrap(),
            (expected_schema.into(), expected_table.into())
        );
    }
}

// TestFetchExtendColumn
/// 验证扩展列提取：表级命中优先，否则回退库级规则。
#[test]
fn test_fetch_extend_column() {
    let rules = vec![
        TableRule {
            TableExtractor: Some(Box::new(table_extractor("table_name", "table_(.*)"))),
            SchemaExtractor: Some(Box::new(schema_extractor("schema_name", "schema_(.*)"))),
            SourceExtractor: Some(Box::new(source_extractor(
                "source_name",
                "source_(.*)_(.*)",
            ))),
            ..rule("schema*", "t*", "test", "t")
        },
        TableRule {
            SchemaExtractor: Some(Box::new(schema_extractor("schema_name", "(.*)"))),
            SourceExtractor: Some(Box::new(source_extractor("source_name", "(.*)"))),
            ..rule("~s?chema.*", "", "test", "t2")
        },
    ];
    let router = NewRegExprRouter(false, rules).unwrap();

    assert_eq!(
        router.FetchExtendColumn("schema_s1", "table_t1", "source_s1_s1"),
        (
            vec![
                "table_name".into(),
                "schema_name".into(),
                "source_name".into()
            ],
            vec!["t1".into(), "s1".into(), "s1s1".into()],
        )
    );
    assert_eq!(
        router.FetchExtendColumn("schema_s2", "a_table_t2", "source_s2"),
        (
            vec!["schema_name".into(), "source_name".into()],
            vec!["schema_s2".into(), "source_s2".into()],
        )
    );
}

// TestAllRule
/// 验证 AllRules 按库级/表级拆分且顺序与输入一致。
#[test]
fn test_all_rule() {
    let rules = vec![
        rule("~test.[0-9]+", "", "dtest1", ""),
        rule(
            "~test2?[animal|human]",
            "~tbl.*[cat|dog]+",
            "dtest2",
            "dtable2",
        ),
        rule("~test3_(schema)?.*", "test3_*", "dtest3", "dtable3"),
        rule(
            "test4s_*",
            "~testtable_[donot_delete]?",
            "dtest4",
            "dtable4",
        ),
    ];
    let router = NewRegExprRouter(true, rules.clone()).unwrap();
    let (schema_rules, table_rules) = router.AllRules();

    assert_eq!(schema_rules.len(), 1);
    assert_eq!(table_rules.len(), 3);
    assert_eq!(schema_rules[0].SchemaPattern, rules[0].SchemaPattern);
    for i in 0..3 {
        assert_eq!(table_rules[i].SchemaPattern, rules[i + 1].SchemaPattern);
        assert_eq!(table_rules[i].TablePattern, rules[i + 1].TablePattern);
    }
}

// TestDupMatch
/// 验证同一对象命中多条规则时返回冲突错误。
#[test]
fn test_dup_match() {
    let rules = vec![
        rule("~test[0-9]+.*", "~.*", "dtest1", ""),
        rule("~test2?[a|b]", "~tbl2", "dtest2", "dtable2"),
        rule("mytest*", "", "mytest", ""),
        rule("~mytest(_meta)?_schema", "", "test", ""),
    ];
    let router = NewRegExprRouter(true, rules).unwrap();

    for (schema, table) in [("test2a", "tbl2"), ("mytest_meta_schema", "")] {
        match router.Route(schema, table) {
            Ok(target) => panic!("expected duplicate-match error, got target {target:?}"),
            Err(error) => assert!(error.contains("matches more than one rule")),
        }
    }
}
