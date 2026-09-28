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

// 索引顾问核心模型语义单测。
//
// 覆盖复合索引的前缀包含判定与稳定键（key）生成：若已有更宽索引覆盖
// 更窄前缀，顾问可避免重复推荐。

/// 宽索引应包含其列前缀索引；不同首列则不构成前缀关系。
#[test]
fn advisor_model_preserves_index_prefix_semantics() {
    // 大小写在构造时统一为小写，键形如 schema.table(col1,col2)。
    let wide = crate::model::Index::new("Test", "T", "idx_ab", ["A", "B"]);
    let prefix = crate::model::Index::new("test", "t", "idx_a", ["a"]);
    let other = crate::model::Index::new("test", "t", "idx_b", ["b"]);
    assert!(wide.prefix_contains(&prefix));
    assert!(!wide.prefix_contains(&other));
    assert_eq!(wide.key(), "test.t(a,b)");
}

#[test]
fn multi_column_candidates_match_go_permutation_enumeration() {
    use std::collections::BTreeSet;

    let columns = ["a", "b", "c"]
        .into_iter()
        .map(|name| crate::model::Column::new("test", "t", name))
        .collect::<BTreeSet<_>>();
    let indexes = crate::algorithm::create_multi_column_indexes(&columns, 2).unwrap();
    let keys = indexes.iter().map(|index| index.key()).collect::<Vec<_>>();

    assert_eq!(keys.len(), 9);
    for key in [
        "test.t(a)",
        "test.t(b)",
        "test.t(c)",
        "test.t(a,b)",
        "test.t(a,c)",
        "test.t(b,a)",
        "test.t(b,c)",
        "test.t(c,a)",
        "test.t(c,b)",
    ] {
        assert!(
            keys.iter().any(|candidate| candidate == key),
            "missing {key}"
        );
    }
}

#[test]
fn advisor_algorithm_propagates_existing_index_lookup_errors() {
    use crate::algorithm::advise_indexes;
    use crate::model::{Column, Query};
    use crate::optimizer::{FieldType, InMemoryOptimizer, Optimizer, TableMetadata};
    use crate::options::AdvisorOptions;
    use std::collections::{BTreeMap, BTreeSet};

    let mut tables = BTreeMap::new();
    tables.insert(
        ("test".to_string(), "t".to_string()),
        TableMetadata {
            columns: [("a".to_string(), FieldType::Integer)]
                .into_iter()
                .collect(),
            indexes: vec![],
            row_count: 0,
            column_total_size: BTreeMap::new(),
        },
    );
    let optimizer = InMemoryOptimizer::new(tables, |_, indexes| {
        Ok(if indexes.is_empty() { 100.0 } else { 1.0 })
    });
    let query = Query {
        alias: String::new(),
        schema_name: "test".into(),
        text: "select * from t where a = 1".into(),
        frequency: 1,
    };
    let columns = [Column::new("test", "missing", "a")]
        .into_iter()
        .collect::<BTreeSet<_>>();
    let options = AdvisorOptions {
        max_num_indexes: 1,
        max_index_width: 1,
        max_num_query: 10,
        timeout: std::time::Duration::from_secs(1),
    };

    let error = advise_indexes(
        &BTreeSet::from([query]),
        &columns,
        &optimizer as &dyn Optimizer,
        &options,
    )
    .unwrap_err();
    assert!(error.contains("missing"));
}

#[test]
fn recommendation_metrics_match_go_per_index_calculation() {
    use crate::indexadvisor::prepare_recommendations;
    use crate::model::{Index, Query};
    use crate::optimizer::{FieldType, InMemoryOptimizer, Optimizer, TableMetadata};
    use std::collections::{BTreeMap, BTreeSet};

    let optimizer = InMemoryOptimizer::new(
        BTreeMap::from([(
            ("test".into(), "t".into()),
            TableMetadata {
                columns: [("a".into(), FieldType::Integer)].into_iter().collect(),
                indexes: vec![],
                row_count: 1,
                column_total_size: BTreeMap::from([("a".into(), 8)]),
            },
        )]),
        |sql, indexes| {
            Ok(if indexes.is_empty() {
                100.0
            } else if sql.contains("where a") {
                50.0
            } else {
                100.0
            })
        },
    );
    let queries = BTreeSet::from([Query {
        alias: String::new(),
        schema_name: "test".into(),
        text: "select * from t where a = 1".into(),
        frequency: 2,
    }]);
    let indexes = BTreeSet::from([Index::new("test", "t", "idx_a", ["a"])]);
    let results =
        prepare_recommendations(&indexes, &queries, &optimizer as &dyn Optimizer).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0]
            .workload_impact
            .as_ref()
            .unwrap()
            .workload_improvement,
        0.5
    );
    assert_eq!(results[0].top_impacted_queries[0].improvement, 0.5);
    assert!(
        results[0]
            .index_detail
            .as_ref()
            .unwrap()
            .reason
            .contains("Column [a] appear")
    );
}

#[test]
fn graceful_index_name_uses_go_fallback_sequence() {
    use crate::indexadvisor::graceful_index_name;
    use crate::model::Index;
    use crate::optimizer::{FieldType, InMemoryOptimizer, Optimizer, TableMetadata};
    use std::collections::{BTreeMap, BTreeSet};

    let optimizer = InMemoryOptimizer::new(
        BTreeMap::from([(
            ("test".into(), "t".into()),
            TableMetadata {
                columns: [("a".into(), FieldType::Integer)].into_iter().collect(),
                indexes: vec![Index::new("test", "t", "idx_a_b", ["a"])],
                row_count: 0,
                column_total_size: BTreeMap::new(),
            },
        )]),
        |_, _| Ok(1.0),
    );
    let name = graceful_index_name(
        &optimizer as &dyn Optimizer,
        "test",
        "t",
        &["a".into(), "b".into()],
    )
    .unwrap();
    assert_eq!(name, "idx_a");
    let _ = BTreeSet::<Index>::new();
}

#[test]
fn specified_sqls_are_not_limited_by_statement_summary_max_query() {
    use crate::indexadvisor::advise_indexes_for_sql;
    use crate::model::Index;
    use crate::optimizer::{FieldType, InMemoryOptimizer, Optimizer, TableMetadata};
    use crate::options::AdvisorOptions;
    use std::collections::BTreeMap;

    let optimizer = InMemoryOptimizer::new(
        BTreeMap::from([(
            ("test".into(), "t".into()),
            TableMetadata {
                columns: [
                    ("a".into(), FieldType::Integer),
                    ("b".into(), FieldType::Integer),
                ]
                .into_iter()
                .collect(),
                indexes: vec![],
                row_count: 1,
                column_total_size: BTreeMap::from([("a".into(), 8), ("b".into(), 8)]),
            },
        )]),
        |sql, indexes| {
            let indexed_columns = indexes
                .iter()
                .flat_map(|index: &Index| index.columns.iter())
                .map(|column| column.column_name.as_str())
                .collect::<Vec<_>>();
            let covered = (sql.contains("where a") && indexed_columns.contains(&"a"))
                || (sql.contains("where b") && indexed_columns.contains(&"b"));
            Ok(if covered { 1.0 } else { 100.0 })
        },
    );
    let options = AdvisorOptions {
        max_num_indexes: 2,
        max_index_width: 1,
        max_num_query: 1,
        timeout: std::time::Duration::from_secs(1),
    };

    let results = advise_indexes_for_sql(
        &optimizer as &dyn Optimizer,
        &[
            "select * from t where a = 1".into(),
            "select * from t where b = 1".into(),
        ],
        "test",
        &options,
    )
    .unwrap();

    let columns = results
        .iter()
        .map(|result| result.index_columns.join(","))
        .collect::<Vec<_>>();
    assert!(columns.contains(&"a".to_string()));
    assert!(columns.contains(&"b".to_string()));
}

#[test]
fn recommendation_checks_index_size_before_discarding_no_benefit_index() {
    use crate::indexadvisor::prepare_recommendations;
    use crate::model::{Column, Index, Query};
    use crate::optimizer::{FieldType, Optimizer};
    use std::collections::BTreeSet;

    struct SizeErrorOptimizer;

    impl Optimizer for SizeErrorOptimizer {
        fn column_type(&self, _: &Column) -> Result<FieldType, String> {
            unreachable!()
        }
        fn prefix_contain_index(&self, _: &Index) -> Result<bool, String> {
            unreachable!()
        }
        fn possible_columns(&self, _: &str, _: &str) -> Result<Vec<Column>, String> {
            unreachable!()
        }
        fn table_columns(&self, _: &str, _: &str) -> Result<Vec<Column>, String> {
            unreachable!()
        }
        fn index_name_exists(&self, _: &str, _: &str, _: &str) -> Result<bool, String> {
            Ok(false)
        }
        fn estimate_index_size(&self, _: &str, _: &str, _: &[String]) -> Result<f64, String> {
            Err("size lookup failed".into())
        }
        fn query_plan_cost(&self, _: &str, _: &[Index]) -> Result<f64, String> {
            Ok(100.0)
        }
    }

    let optimizer = SizeErrorOptimizer;
    let queries = BTreeSet::from([Query {
        alias: String::new(),
        schema_name: "test".into(),
        text: "select * from missing where a = 1".into(),
        frequency: 1,
    }]);
    let indexes = BTreeSet::from([Index::new("test", "missing", "idx_a", ["a"])]);

    let error = prepare_recommendations(&indexes, &queries, &optimizer as &dyn Optimizer)
        .expect_err("Go checks index metadata before calculating and filtering improvements");
    assert_eq!(error, "size lookup failed");
}
