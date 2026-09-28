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

// 索引集合代价比较规则单测。
//
// `IndexSetCost::less` 对齐 Go：先比相对显著的 workload 代价差，再比总列数，
// 最后比索引键字符串，保证枚举过程中的稳定择优。

/// 代价更低的集合应判定为更优，反向比较为假。
#[test]
fn optimizer_cost_order_matches_go_tie_breakers() {
    // 80 vs 100 的差值相对显著，直接按 total_workload_query_cost 判定。
    let cheaper = crate::model::IndexSetCost {
        total_workload_query_cost: 80.0,
        total_number_of_index_columns: 3,
        index_keys: "z".into(),
    };
    let expensive = crate::model::IndexSetCost {
        total_workload_query_cost: 100.0,
        total_number_of_index_columns: 1,
        index_keys: "a".into(),
    };
    assert!(cheaper.less(&expensive));
    assert!(!expensive.less(&cheaper));
}

#[test]
fn in_memory_optimizer_matches_metadata_and_cost_hook_contract() {
    use crate::model::{Column, Index};
    use crate::optimizer::{FieldType, InMemoryOptimizer, Optimizer, TableMetadata};
    use std::collections::{BTreeMap, BTreeSet};

    let optimizer = InMemoryOptimizer::new(
        BTreeMap::from([(
            ("Test".to_ascii_lowercase(), "T".to_ascii_lowercase()),
            TableMetadata {
                columns: [
                    ("a".into(), FieldType::Integer),
                    ("b".into(), FieldType::String),
                    ("payload".into(), FieldType::Blob),
                ]
                .into_iter()
                .collect(),
                indexes: vec![Index::new("test", "t", "ab", ["a", "b"])],
                row_count: 10,
                column_total_size: BTreeMap::from([("a".into(), 12)]),
            },
        )]),
        |sql, indexes| Ok(100.0 - indexes.len() as f64 - sql.len() as f64 * 0.0),
    );
    let a = Column::new("TEST", "T", "A");
    assert_eq!(optimizer.column_type(&a).unwrap(), FieldType::Integer);
    let mixed_case_a = Column {
        schema_name: "TEST".into(),
        table_name: "T".into(),
        column_name: "A".into(),
    };
    assert_eq!(
        optimizer.column_type(&mixed_case_a).unwrap(),
        FieldType::Integer
    );
    assert!(
        optimizer
            .prefix_contain_index(&Index::new("test", "t", "idx_a", ["a"]))
            .unwrap()
    );
    assert!(
        !optimizer
            .prefix_contain_index(&Index::new("test", "t", "idx_b", ["b"]))
            .unwrap()
    );
    let mixed_case_prefix = Index {
        schema_name: "TEST".into(),
        table_name: "T".into(),
        index_name: "IDX_A".into(),
        columns: vec![mixed_case_a],
    };
    assert!(optimizer.prefix_contain_index(&mixed_case_prefix).unwrap());
    assert_eq!(optimizer.possible_columns("test", "a").unwrap().len(), 1);
    assert!(optimizer.possible_columns("test", "A").unwrap().is_empty());
    assert!(
        optimizer
            .possible_columns("INFORMATION_SCHEMA", "a")
            .unwrap()
            .is_empty()
    );
    let table_columns = optimizer.table_columns("TEST", "T").unwrap();
    assert_eq!(table_columns.len(), 3);
    assert!(
        table_columns
            .iter()
            .all(|column| column.schema_name == "TEST" && column.table_name == "T")
    );
    assert!(optimizer.index_name_exists("test", "t", "ab").unwrap());
    // Go compares the stored lowercase CIStr name with the argument directly.
    assert!(!optimizer.index_name_exists("test", "t", "AB").unwrap());
    assert_eq!(
        optimizer
            .estimate_index_size("test", "t", &["a".into(), "b".into()])
            .unwrap(),
        92.0
    );
    assert_eq!(
        optimizer
            .estimate_index_size("test", "t", &["A".into()])
            .unwrap(),
        80.0
    );
    assert_eq!(optimizer.query_plan_cost("select 1", &[]).unwrap(), 100.0);
    assert_eq!(
        optimizer
            .query_plan_cost("select 1", &[mixed_case_prefix])
            .unwrap(),
        99.0
    );
    assert!(
        optimizer
            .column_type(&Column::new("test", "t", "missing"))
            .is_err()
    );
    assert!(optimizer.table_columns("test", "missing").is_err());

    let _ = BTreeSet::<Column>::new();
}
