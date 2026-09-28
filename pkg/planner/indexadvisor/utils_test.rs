// Copyright 2026 AsterSQL.
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

// 索引顾问（Index Advisor）工具函数的单元测试。
//
// 覆盖：从 SQL 收集带 schema 限定名与默认 schema 下的表名，以及与 Go AST
// 一致的合法语句、字段别名和 DNF 表达式处理。

/// 验证 collect_table_names 同时收集限定名与默认库表，且解析入口与 Go 一样接受 DDL。
#[test]
fn utility_parser_collects_qualified_and_default_schema_tables() {
    // 默认库为 test；JOIN 左侧落在默认库 t，右侧为 analytics.events。
    let tables = crate::utils::collect_table_names(
        "test",
        "select * from t join analytics.events on t.id = events.id",
    )
    .unwrap();
    assert_eq!(tables, vec!["test.t", "analytics.events"]);
    assert!(crate::utils::parse_one_sql("drop table t").is_ok());
}

#[test]
fn utility_parser_keeps_all_tables_in_comma_join_and_subquery() {
    let tables = crate::utils::collect_table_names(
        "test",
        "select * from t1, t2 where t1.id in (select id from t3)",
    )
    .unwrap();
    assert_eq!(tables, vec!["test.t1", "test.t2", "test.t3"]);

    let aliased = crate::utils::collect_table_names("test", "select * from t1 as x").unwrap();
    assert_eq!(aliased, vec!["test.t1"]);
    let cte = crate::utils::collect_table_names(
        "test",
        "with cte as (select * from t1) select * from cte",
    )
    .unwrap();
    assert_eq!(cte, vec!["test.t1"]);
}

#[test]
fn utility_indexable_columns_only_use_go_predicate_and_order_clauses() {
    use crate::model::{Column, Query};
    use crate::optimizer::{FieldType, InMemoryOptimizer, Optimizer, TableMetadata};
    use std::collections::{BTreeMap, BTreeSet};

    let table = TableMetadata {
        columns: [
            ("selected".into(), FieldType::Integer),
            ("filtered".into(), FieldType::Integer),
            ("ordered".into(), FieldType::Integer),
        ]
        .into_iter()
        .collect(),
        indexes: vec![],
        row_count: 0,
        column_total_size: BTreeMap::new(),
    };
    let optimizer = InMemoryOptimizer::new(
        BTreeMap::from([(("test".into(), "t".into()), table)]),
        |_, _| Ok(1.0),
    );
    let query = Query {
        alias: String::new(),
        schema_name: "test".into(),
        text: "select selected from t where filtered = 1 order by ordered".into(),
        frequency: 1,
    };
    let columns =
        crate::utils::collect_indexable_columns(&query, &optimizer as &dyn Optimizer).unwrap();
    assert_eq!(
        columns,
        BTreeSet::from([
            Column::new("test", "t", "filtered"),
            Column::new("test", "t", "ordered"),
        ])
    );

    let function_query = Query {
        text: "select * from t where filtered = now()".into(),
        ..query
    };
    let function_columns =
        crate::utils::collect_indexable_columns(&function_query, &optimizer as &dyn Optimizer)
            .unwrap();
    assert_eq!(
        function_columns,
        BTreeSet::from([Column::new("test", "t", "filtered")])
    );
}

#[test]
fn utility_filters_queries_without_tables_like_go() {
    use crate::model::Query;
    use std::collections::BTreeSet;

    let queries = BTreeSet::from([Query {
        alias: String::new(),
        schema_name: "test".into(),
        text: "select @@autocommit".into(),
        frequency: 1,
    }]);
    assert!(
        crate::utils::filter_system_queries(queries, false)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn utility_ast_equivalents_cover_select_order_dnf_and_schema_restore() {
    use crate::model::Query;
    use std::collections::BTreeSet;

    let query = Query {
        alias: String::new(),
        schema_name: "test".into(),
        text: "select a, b from t where a = 1 or b = 2 order by a, b".into(),
        frequency: 1,
    };
    let selected = crate::utils::collect_select_columns(&query).unwrap();
    assert_eq!(selected.len(), 2);
    assert_eq!(
        crate::utils::collect_order_by_columns(&query)
            .unwrap()
            .iter()
            .map(|column| column.key())
            .collect::<Vec<_>>(),
        vec!["test.t.a", "test.t.b"]
    );
    assert_eq!(crate::utils::collect_dnf_columns(&query).unwrap().len(), 2);

    let restored = crate::utils::restore_schema_name(
        "test",
        BTreeSet::from([Query {
            alias: String::new(),
            schema_name: String::new(),
            text: "select * from t".into(),
            frequency: 1,
        }]),
        false,
    )
    .unwrap()
    .into_iter()
    .next()
    .unwrap();
    assert_eq!(restored.schema_name, "test");
    assert!(restored.text.contains("test.t"));
}

#[test]
fn utility_select_columns_exclude_field_aliases_like_go_ast() {
    use crate::model::Query;

    let query = Query {
        alias: String::new(),
        schema_name: "test".into(),
        text: "select a as renamed, abs(b) calculated from t".into(),
        frequency: 1,
    };
    assert_eq!(
        crate::utils::collect_select_columns(&query)
            .unwrap()
            .into_iter()
            .map(|column| column.key())
            .collect::<Vec<_>>(),
        vec!["test.t.a", "test.t.b"]
    );
}

#[test]
fn utility_dnf_handles_cnf_groups_and_reversed_equality_like_go_ast() {
    use crate::model::Query;

    let query = Query {
        alias: String::new(),
        schema_name: "test".into(),
        text: "select * from t where x = 0 and (1 = a or b = 2)".into(),
        frequency: 1,
    };
    assert_eq!(
        crate::utils::collect_dnf_columns(&query)
            .unwrap()
            .into_iter()
            .map(|column| column.key())
            .collect::<Vec<_>>(),
        vec!["test.t.a", "test.t.b"]
    );
}

#[test]
fn utility_collects_same_named_columns_from_all_schema_tables() {
    use crate::model::Query;
    use crate::optimizer::{FieldType, InMemoryOptimizer, Optimizer, TableMetadata};
    use std::collections::{BTreeMap, BTreeSet};

    let table = |column: &str| TableMetadata {
        columns: [(column.to_string(), FieldType::Integer)]
            .into_iter()
            .collect(),
        indexes: vec![],
        row_count: 0,
        column_total_size: BTreeMap::new(),
    };
    let optimizer = InMemoryOptimizer::new(
        BTreeMap::from([
            (("test".into(), "t1".into()), table("a")),
            (("test".into(), "t2".into()), table("a")),
        ]),
        |_, _| Ok(1.0),
    );
    let query = Query {
        alias: String::new(),
        schema_name: "test".into(),
        text: "select * from t2 where a < 1".into(),
        frequency: 1,
    };
    let columns =
        crate::utils::collect_indexable_columns(&query, &optimizer as &dyn Optimizer).unwrap();
    assert_eq!(columns.len(), 2);
    assert_eq!(
        columns
            .iter()
            .map(|column| column.key())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["test.t1.a".into(), "test.t2.a".into()])
    );
}
