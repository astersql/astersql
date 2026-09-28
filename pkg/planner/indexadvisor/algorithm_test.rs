// Copyright 2026 AsterSQL.

use crate::algorithm::advise_indexes;
use crate::model::{Column, Query};
use crate::optimizer::{FieldType, InMemoryOptimizer, Optimizer, TableMetadata};
use crate::options::AdvisorOptions;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

#[test]
fn each_query_keeps_only_three_best_single_index_candidates() {
    let columns = ["a", "b", "c", "d"]
        .into_iter()
        .map(|name| Column::new("test", "t", name))
        .collect::<BTreeSet<_>>();
    let optimizer = InMemoryOptimizer::new(
        BTreeMap::from([(
            ("test".into(), "t".into()),
            TableMetadata {
                columns: ["a", "b", "c", "d"]
                    .into_iter()
                    .map(|name| (name.into(), FieldType::Integer))
                    .collect(),
                indexes: vec![],
                row_count: 0,
                column_total_size: BTreeMap::new(),
            },
        )]),
        |_, indexes| Ok(200.0 - indexes.len() as f64 * 30.0),
    );
    let queries = BTreeSet::from([Query {
        alias: String::new(),
        schema_name: "test".into(),
        text: "select * from t where a=1 and b=1 and c=1 and d=1".into(),
        frequency: 1,
    }]);
    let options = AdvisorOptions {
        max_num_indexes: 4,
        max_index_width: 1,
        max_num_query: 10,
        timeout: Duration::from_secs(1),
    };

    let selected = advise_indexes(&queries, &columns, &optimizer as &dyn Optimizer, &options)
        .expect("advisor should succeed");
    assert_eq!(selected.len(), 3);
}
