// Copyright 2024 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// 视图导入（view import）解析与拓扑排序单元测试。
//
// 覆盖 `parseViewSchemaSQL`：依赖抽取、CTE（公用表表达式）忽略、跨库依赖、
// 去重与错误路径；以及 `buildViewImportPlan`：多视图拓扑序、大小写归一、
// 同层确定性排序、重复定义拒绝与环检测。

use crate::*;

/// 构造库表名（schema + table/view）。
fn name(schema: &str, table: &str) -> TableName {
    TableName {
        schema: schema.into(),
        name: table.into(),
    }
}

/// 解析 CREATE VIEW SQL，失败时 panic（测试期望成功路径）。
fn parse(schema: &str, view: &str, sql: &str) -> ParsedViewSchema {
    parseViewSchemaSQL(name(schema, view), sql).unwrap()
}

/// 取出依赖列表并排序，便于与期望向量稳定比较。
fn deps(parsed: &ParsedViewSchema) -> Vec<TableName> {
    let mut deps = parsed.deps.clone();
    deps.sort();
    deps
}

/// 构造带固定 CREATE VIEW 文本的解析结果，用于导入计划测试。
fn definition(key: TableName, dependencies: Vec<TableName>) -> ParsedViewSchema {
    ParsedViewSchema {
        create_sql: format!("CREATE VIEW `{}`.`{}` AS SELECT 1;", key.schema, key.name),
        key,
        deps: dependencies,
    }
}

/// 验证解析剥离 DROP、保留 SET NAMES，并抽取单表依赖。
#[test]
fn TestParseViewSchemaSQL() {
    let parsed = parse(
        "test",
        "v2",
        "SET NAMES binary; DROP TABLE IF EXISTS v2; DROP VIEW IF EXISTS v2; CREATE ALGORITHM=UNDEFINED DEFINER=`root`@`%` SQL SECURITY DEFINER VIEW v2 AS SELECT id FROM `test`.`v1`;",
    );
    assert_eq!(parsed.key, name("test", "v2"));
    assert_eq!(parsed.deps, vec![name("test", "v1")]);
    assert!(!parsed.create_sql.contains("DROP TABLE"));
    assert!(!parsed.create_sql.contains("DROP VIEW"));
    assert!(parsed.create_sql.contains("SET NAMES 'binary'"));
}

/// 验证子查询/JOIN 中同名依赖去重，未限定名归属当前 schema。
#[test]
fn TestParseViewSchemaSQLDeduplicatesAndUsesCurrentSchema() {
    let parsed = parse(
        "test",
        "v3",
        "CREATE VIEW v3 AS SELECT src.id FROM (SELECT id FROM v1 UNION SELECT id FROM test.v1) src JOIN v2 ON v2.id=src.id;",
    );
    assert_eq!(deps(&parsed), vec![name("test", "v1"), name("test", "v2")]);
}

/// 验证跨 schema 与本地表依赖均可识别。
#[test]
fn TestParseViewSchemaSQLSupportsMultipleAndCrossSchemaDeps() {
    let parsed = parse(
        "db3",
        "v4",
        "CREATE VIEW v4 AS SELECT src.id FROM db1.v1 src JOIN db2.v2 ON 1 JOIN t_local ON 1 JOIN db2.base_table ON 1;",
    );
    assert_eq!(
        deps(&parsed),
        vec![
            name("db1", "v1"),
            name("db2", "base_table"),
            name("db2", "v2"),
            name("db3", "t_local"),
        ]
    );
}

/// SQL 字符串和注释中的 FROM/JOIN 文本不是 AST 表引用，不应成为依赖。
#[test]
fn TestParseViewSchemaSQLIgnoresReferenceKeywordsInLiteralsAndComments() {
    let parsed = parse(
        "test",
        "v_literals",
        "CREATE VIEW v_literals AS SELECT 'FROM phantom', \"JOIN ghost\" /* FROM hidden */ FROM real_table -- JOIN ignored\nJOIN second_table ON 1;",
    );
    assert_eq!(
        deps(&parsed),
        vec![name("test", "real_table"), name("test", "second_table")]
    );
}

/// CTE 名称本身不是表依赖，只应保留 CTE 体内真实基表。
#[test]
fn TestParseViewSchemaSQLIgnoresCTEDependencies() {
    let parsed = parse(
        "test",
        "v_cte",
        "CREATE VIEW v_cte AS WITH cte AS (SELECT id FROM t1) SELECT cte.id FROM cte;",
    );
    assert_eq!(parsed.deps, vec![name("test", "t1")]);
}

/// 递归 CTE 自引用不应记为依赖。
#[test]
fn TestParseViewSchemaSQLIgnoresRecursiveCTESelfReference() {
    let parsed = parse(
        "test",
        "v_recursive",
        "CREATE VIEW v_recursive AS WITH RECURSIVE cte AS (SELECT id FROM t1 UNION ALL SELECT id FROM cte) SELECT id FROM cte;",
    );
    assert_eq!(parsed.deps, vec![name("test", "t1")]);
}

/// 集合运算根部带 CTE 时，两侧真实表均计入依赖。
#[test]
fn TestParseViewSchemaSQLIgnoresCTEDependenciesInSetOperatorRoot() {
    let parsed = parse(
        "test",
        "v",
        "CREATE VIEW v AS WITH cte AS (SELECT id FROM t1) SELECT id FROM cte UNION SELECT id FROM t2;",
    );
    assert_eq!(deps(&parsed), vec![name("test", "t1"), name("test", "t2")]);
}

/// 集合运算某一分支内的 CTE 仍只贡献其体内基表。
#[test]
fn TestParseViewSchemaSQLIgnoresCTEDependenciesInSetOperatorBranch() {
    let parsed = parse(
        "test",
        "v",
        "CREATE VIEW v AS SELECT id FROM t0 UNION (WITH cte AS (SELECT id FROM t1) SELECT id FROM cte);",
    );
    assert_eq!(deps(&parsed), vec![name("test", "t0"), name("test", "t1")]);
}

/// 非 DROP 的意外语句（如 USE）应保留在 create_sql 中。
#[test]
fn TestParseViewSchemaSQLPreservesUnexpectedStatements() {
    let parsed = parse("test", "v", "USE analytics; CREATE VIEW v AS SELECT 1;");
    assert!(parsed.create_sql.contains("USE analytics"));
}

/// 缺少 CREATE VIEW 时应报错并带上视图全名。
#[test]
fn TestParseViewSchemaSQLReportsMissingCreateViewWithName() {
    let error = parseViewSchemaSQL(name("test", "v_missing"), "USE analytics;").unwrap_err();
    assert!(error.to_string().contains("`test`.`v_missing`"));
}

/// 同一 SQL 中多个 CREATE VIEW 应拒绝。
#[test]
fn TestParseViewSchemaSQLRejectsMultipleCreateStatements() {
    let error = parseViewSchemaSQL(
        name("test", "v"),
        "CREATE VIEW v AS SELECT 1; CREATE VIEW v AS SELECT 2;",
    )
    .unwrap_err();
    assert!(error.to_string().contains("multiple create view"));
}

/// 跨库视图依赖应按拓扑序排列（被依赖者在前）。
#[test]
fn TestBuildViewImportPlanSupportsMultipleAndCrossSchemaViewDeps() {
    let v1 = name("db1", "v1");
    let v2 = name("db2", "v2");
    let v3 = name("db2", "v3");
    let tables = [name("db1", "t1"), name("db2", "t2"), name("db2", "t3")]
        .into_iter()
        .collect();
    let plan = buildViewImportPlan(
        &[
            definition(v1.clone(), vec![name("db1", "t1"), name("db2", "t2")]),
            definition(v3.clone(), vec![name("db2", "t3")]),
            definition(v2.clone(), vec![v1.clone(), v3.clone()]),
        ],
        &tables,
    )
    .unwrap();
    assert_eq!(plan.ordered, vec![v1, v3, v2]);
}

/// 大小写不敏感的库表名应归一，外部依赖清空。
#[test]
fn TestBuildViewImportPlanNormalizesCaseInsensitiveDeps() {
    let plan = buildViewImportPlan(
        &[
            definition(name("test", "v1"), vec![name("test", "t")]),
            definition(name("test", "V2"), vec![name("Test", "V1")]),
        ],
        &[name("TEST", "T")].into_iter().collect(),
    )
    .unwrap();
    assert_eq!(plan.ordered, vec![name("test", "v1"), name("test", "v2")]);
    assert!(plan.nodes[&name("test", "v2")].external_deps.is_empty());
}

/// 同层多视图应按名称稳定排序，保证计划可复现。
#[test]
fn TestBuildViewImportPlanKeepsWideTopoLayerDeterministic() {
    let tables = [name("test", "t1"), name("test", "t2")]
        .into_iter()
        .collect();
    let plan = buildViewImportPlan(
        &[
            definition(name("test", "v1"), vec![name("test", "t1")]),
            definition(name("test", "v2"), vec![name("test", "t2")]),
            definition(name("test", "vz"), vec![name("test", "v1")]),
            definition(name("test", "va"), vec![name("test", "v2")]),
        ],
        &tables,
    )
    .unwrap();
    assert_eq!(
        plan.ordered,
        vec![
            name("test", "v1"),
            name("test", "v2"),
            name("test", "va"),
            name("test", "vz"),
        ]
    );
}

/// 仅大小写不同的重复视图定义应报错。
#[test]
fn TestBuildViewImportPlanRejectsCaseInsensitiveDuplicates() {
    let error = buildViewImportPlan(
        &[
            definition(name("test", "v1"), vec![]),
            definition(name("Test", "V1"), vec![]),
        ],
        &TableNameSet::new(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("duplicate view definition"));
}

/// 视图互相依赖形成环时应报 cyclic 并列出成员。
#[test]
fn TestBuildViewImportPlanDetectsCycle() {
    let error = buildViewImportPlan(
        &[
            definition(name("test", "v1"), vec![name("test", "v2")]),
            definition(name("test", "v2"), vec![name("test", "v1")]),
        ],
        &TableNameSet::new(),
    )
    .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("cyclic"));
    assert!(message.contains("`test`.`v1`"));
    assert!(message.contains("`test`.`v2`"));
}
