// Copyright 2026 AsterSQL.

// regexpr-router 迁移对齐单元测试。
//
// 对照 Go 语义验证：库级通配路由、表级优先于库级、正则与 glob 混用、
// 大小写不敏感归一化、扩展列提取顺序、AllRules 分类，以及冲突/非法规则报错。

use crate::router::{SchemaExtractor, SourceExtractor, TableExtractor, TableRule};
use crate::*;

/// 构造简易 TableRule（仅填 pattern 与目标库表）。
fn rule(schema: &str, table: &str, target_schema: &str, target_table: &str) -> TableRule {
    TableRule {
        SchemaPattern: schema.into(),
        TablePattern: table.into(),
        TargetSchema: target_schema.into(),
        TargetTable: target_table.into(),
        ..Default::default()
    }
}

/// 验证库级 glob 路由与未命中时回退到输入库表名。
#[test]
fn migration_routes_schema_glob_and_unmatched_table_like_go() {
    let router = NewRegExprRouter(
        true,
        vec![
            rule("test1", "", "dtest1", ""),
            rule("gtest*", "", "dtest", ""),
        ],
    )
    .unwrap();

    assert_eq!(
        router.Route("test1", "table1").unwrap(),
        ("dtest1".into(), "table1".into())
    );
    assert_eq!(
        router.Route("gtesttest", "atable").unwrap(),
        ("dtest".into(), "atable".into())
    );
    assert_eq!(
        router.Route("ptest", "atableg").unwrap(),
        ("ptest".into(), "atableg".into())
    );
}

/// 验证表级规则优先于库级规则。
#[test]
fn migration_table_rules_override_schema_rules_like_go() {
    let router = NewRegExprRouter(
        true,
        vec![
            rule("test*", "", "schema_target", ""),
            rule("test1", "table1", "dtest1", "dtable1"),
        ],
    )
    .unwrap();

    assert_eq!(
        router.Route("test1", "table1").unwrap(),
        ("dtest1".into(), "dtable1".into())
    );
}

/// 验证正则与 glob 规则混用时的匹配结果。
#[test]
fn migration_supports_mixed_regular_expression_and_glob_rules() {
    let router = NewRegExprRouter(
        true,
        vec![
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
        ],
    )
    .unwrap();

    assert_eq!(router.Route("tests100", "table1").unwrap().0, "dtest1");
    assert_eq!(
        router.Route("test2animal", "tbl_animal_dogcat").unwrap().0,
        "dtest2"
    );
    assert_eq!(
        router.Route("test3_schema_meta", "test3_tail").unwrap().0,
        "dtest3"
    );
    assert_eq!(
        router
            .Route("test4s_2022", "testtable_donot_delete")
            .unwrap()
            .0,
        "dtest4"
    );
}

/// 验证大小写不敏感时规则归一化，未命中仍保留原始输入大小写。
#[test]
fn migration_case_insensitive_router_normalizes_rules_but_preserves_input_fallback() {
    let router =
        NewRegExprRouter(false, vec![rule("TEST*", "TABLE*", "target", "routed")]).unwrap();
    assert_eq!(
        router.Route("TestDB", "TableOne").unwrap(),
        ("target".into(), "routed".into())
    );
    assert_eq!(
        router.Route("Other", "MixedCase").unwrap(),
        ("Other".into(), "MixedCase".into())
    );
}

/// 验证 FetchExtendColumn 按 table → schema → source 顺序提取扩展列。
#[test]
fn migration_fetches_extractors_in_table_schema_source_order() {
    let mut table_extractor = TableExtractor::default();
    table_extractor.TargetColumn = "table_name".into();
    table_extractor.TableRegexp = "table_(.*)".into();
    let mut table_schema_extractor = SchemaExtractor::default();
    table_schema_extractor.TargetColumn = "schema_name".into();
    table_schema_extractor.SchemaRegexp = "schema_(.*)".into();
    let mut table_source_extractor = SourceExtractor::default();
    table_source_extractor.TargetColumn = "source_name".into();
    table_source_extractor.SourceRegexp = "source_(.*)_(.*)".into();
    let table_rule = TableRule {
        TableExtractor: Some(Box::new(table_extractor)),
        SchemaExtractor: Some(Box::new(table_schema_extractor)),
        SourceExtractor: Some(Box::new(table_source_extractor)),
        ..rule("schema*", "t*", "test", "t")
    };
    let mut schema_extractor = SchemaExtractor::default();
    schema_extractor.TargetColumn = "schema_name".into();
    schema_extractor.SchemaRegexp = "(.*)".into();
    let mut source_extractor = SourceExtractor::default();
    source_extractor.TargetColumn = "source_name".into();
    source_extractor.SourceRegexp = "(.*)".into();
    let schema_rule = TableRule {
        SchemaExtractor: Some(Box::new(schema_extractor)),
        SourceExtractor: Some(Box::new(source_extractor)),
        ..rule("~s?chema.*", "", "test", "t2")
    };
    let router = NewRegExprRouter(false, vec![table_rule, schema_rule]).unwrap();

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

/// 验证 AllRules 按添加顺序拆分为库级与表级，且类型正确。
#[test]
fn migration_all_rules_preserves_order_and_kind() {
    let rules = vec![
        rule("~test.[0-9]+", "", "dtest1", ""),
        rule("schema2", "table2", "dtest2", "dtable2"),
        rule("schema3", "table3", "dtest3", "dtable3"),
    ];
    let router = NewRegExprRouter(true, rules.clone()).unwrap();
    let (schemas, tables) = router.AllRules();
    assert_eq!(schemas.len(), 1);
    assert_eq!(tables.len(), 2);
    assert_eq!(schemas[0].SchemaPattern, rules[0].SchemaPattern);
    assert_eq!(tables[0].SchemaPattern, rules[1].SchemaPattern);
    assert_eq!(tables[1].TablePattern, rules[2].TablePattern);
}

/// 验证多规则命中同一表时返回 Go 风格冲突错误且无目标。
#[test]
fn migration_duplicate_matches_return_go_error_and_no_target() {
    let router = NewRegExprRouter(
        true,
        vec![
            rule("~test[0-9]+.*", "~.*", "dtest1", ""),
            rule("~test2?[a|b]", "~tbl2", "dtest2", "dtable2"),
        ],
    )
    .unwrap();
    let error = router.Route("test2a", "tbl2").unwrap_err();
    assert!(error.contains("table test2a.tbl2 matches more than one rule"));
}

/// 验证非法空规则在构造阶段被拒绝。
#[test]
fn migration_rejects_invalid_rules() {
    let error = NewRegExprRouter(true, vec![TableRule::default()])
        .err()
        .unwrap();
    assert!(error.contains("schema pattern"));
}
