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

use crate::indexadvisor::advise_indexes_for_sql;
use crate::model::Index;
use crate::optimizer::{FieldType, InMemoryOptimizer, TableMetadata};
use crate::options::AdvisorOptions;
use std::collections::BTreeMap;

fn table(columns: &[(&str, FieldType)], indexes: Vec<Index>) -> TableMetadata {
    TableMetadata {
        columns: columns
            .iter()
            .map(|(name, kind)| ((*name).to_string(), kind.clone()))
            .collect(),
        indexes,
        row_count: 100,
        column_total_size: BTreeMap::new(),
    }
}

fn recommend(
    tables: BTreeMap<(String, String), TableMetadata>,
    sql: &str,
    preferred: &[(&str, &str, &[&str])],
) -> Vec<String> {
    let preferred = preferred
        .iter()
        .map(|(schema, table, columns)| format!("{schema}.{table}({})", columns.join(",")))
        .collect::<Vec<_>>();
    let optimizer = InMemoryOptimizer::new(tables, move |query, indexes| {
        let improvement = indexes
            .iter()
            .map(|index| {
                if !query.to_ascii_lowercase().contains(&index.table_name) {
                    return 0.0;
                }
                let candidate = index.key();
                preferred
                    .iter()
                    .find(|target| target.starts_with(candidate.trim_end_matches(')')))
                    .map_or(0.0, |target| if target == &candidate { 60.0 } else { 20.0 })
            })
            .sum::<f64>();
        Ok(100.0 - improvement)
    });
    let recommendations = advise_indexes_for_sql(
        &optimizer,
        &[sql.to_string()],
        "test",
        &AdvisorOptions::default(),
    )
    .expect("SQL advisor should accept the same workload as Go");
    let mut result = recommendations
        .into_iter()
        .map(|item| {
            format!(
                "{}.{}.{}",
                item.database,
                item.table,
                item.index_columns.join(",")
            )
        })
        .collect::<Vec<_>>();
    result.sort();
    result
}

#[test]
fn index_advisor_for_sql_matches_go_single_table_cases() {
    let tables = BTreeMap::from([(
        ("test".into(), "t".into()),
        table(
            &[
                ("a", FieldType::Integer),
                ("b", FieldType::Integer),
                ("c", FieldType::Integer),
            ],
            Vec::new(),
        ),
    )]);
    assert_eq!(
        recommend(
            tables.clone(),
            "select a from t where a=1",
            &[("test", "t", &["a"])],
        ),
        ["test.t.a"]
    );
    assert_eq!(
        recommend(
            tables.clone(),
            "select a from t where a=1 and b=1",
            &[("test", "t", &["a", "b"])],
        ),
        ["test.t.a,b"]
    );
    assert_eq!(
        recommend(
            tables,
            "select a from t where a=1 and c=1",
            &[("test", "t", &["a", "c"])],
        ),
        ["test.t.a,c"]
    );
}

#[test]
fn index_advisor_for_multiple_tables_obeys_go_result_limit() {
    let mut tables = BTreeMap::new();
    for number in 1..=20 {
        tables.insert(
            ("test".into(), format!("t{number}")),
            table(&[("a", FieldType::Integer)], Vec::new()),
        );
    }
    let sqls = (1..=20)
        .map(|number| format!("select * from t{number} where a=1"))
        .collect::<Vec<_>>();
    let optimizer = InMemoryOptimizer::new(tables, |query, indexes| {
        Ok(100.0
            - indexes
                .iter()
                .filter(|index| query.contains(&format!("from test.{} ", index.table_name)))
                .count() as f64
                * 20.0)
    });
    let recommendations =
        advise_indexes_for_sql(&optimizer, &sqls, "test", &AdvisorOptions::default()).unwrap();
    let actual = recommendations
        .iter()
        .map(|item| {
            format!(
                "{}.{}.{}",
                item.database,
                item.table,
                item.index_columns.join(",")
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        [
            "test.t1.a",
            "test.t10.a",
            "test.t11.a",
            "test.t12.a",
            "test.t13.a"
        ]
    );
}

#[test]
fn index_advisor_for_various_types_matches_go_cases() {
    let tables = BTreeMap::from([(
        ("test".into(), "t".into()),
        table(
            &[
                ("a", FieldType::Integer),
                ("b", FieldType::String),
                ("c", FieldType::Float),
                ("d", FieldType::DateTime),
                ("e", FieldType::Decimal),
            ],
            Vec::new(),
        ),
    )]);
    for (sql, columns) in [
        ("select * from t where a=1", vec!["a"]),
        ("select * from t where b=\"1\"", vec!["b"]),
        ("select * from t where c=1.0", vec!["c"]),
        ("select * from t where d=now()", vec!["d"]),
        ("select * from t where e=1.0", vec!["e"]),
        ("select * from t where a=1 and b=\"1\"", vec!["a", "b"]),
        ("select * from t where a=1 and c=1.0", vec!["a", "c"]),
        ("select * from t where b=\"1\" and c=1.0", vec!["c", "b"]),
        ("select * from t where d=now() and b=\"1\"", vec!["d", "b"]),
    ] {
        let expected = format!("test.t.{}", columns.join(","));
        assert_eq!(
            recommend(tables.clone(), sql, &[("test", "t", &columns)]),
            [expected],
            "{sql}"
        );
    }
}

#[test]
fn index_advisor_returns_empty_when_existing_index_covers_query() {
    let tables = BTreeMap::from([(
        ("test".into(), "t".into()),
        table(
            &[
                ("a", FieldType::Integer),
                ("b", FieldType::Integer),
                ("c", FieldType::Integer),
            ],
            vec![Index::new("test", "t", "idx_a_b_c", ["a", "b", "c"])],
        ),
    )]);
    let recommendations = recommend(
        tables,
        "select * from t where a=1 and b=1 and c=1",
        &[("test", "t", &["a", "b", "c"])],
    );
    assert!(recommendations.is_empty(), "{recommendations:?}");
}
