// Copyright 2026 AsterSQL.

use astersql_executor_join::index_lookup_join::{IndexJoinExecutorBuilder, IndexJoinLookupContent};
use astersql_executor_join::index_lookup_merge_join::IndexLookUpMergeJoin;
use astersql_executor_join::joiner::{JoinType, Joiner, Row};
use astersql_executor_join::row_table_builder::Value;

struct Builder {
    rows: Vec<Row>,
    key_columns: Vec<usize>,
}

impl IndexJoinExecutorBuilder for Builder {
    fn build(&self, lookup_contents: &[IndexJoinLookupContent]) -> Result<Vec<Row>, String> {
        Ok(self
            .rows
            .iter()
            .filter(|candidate| {
                let key = self
                    .key_columns
                    .iter()
                    .map(|column| candidate[*column].clone())
                    .collect::<Row>();
                lookup_contents.iter().any(|content| content.keys == key)
            })
            .cloned()
            .collect())
    }
}

fn run_merge_join(
    outer_rows: Vec<Row>,
    inner_rows: Vec<Row>,
    outer_keys: Vec<usize>,
    inner_keys: Vec<usize>,
    inner_column_count: usize,
) -> Vec<Row> {
    let joiner = Joiner::new(
        JoinType::Inner,
        false,
        vec![Value::Null; inner_column_count],
        Vec::new(),
        None,
        false,
        32,
    )
    .unwrap();
    let builder_keys = inner_keys.clone();
    let mut executor = IndexLookUpMergeJoin::new(
        outer_rows,
        outer_keys,
        inner_keys,
        Box::new(Builder {
            rows: inner_rows,
            key_columns: builder_keys,
        }),
        joiner,
        1,
        32,
    )
    .unwrap();
    let mut rows = Vec::new();
    loop {
        let result = executor.next(32).unwrap();
        assert!(result.error.is_none());
        if result.rows.is_empty() {
            break;
        }
        rows.extend(result.rows);
    }
    executor.close();
    rows
}

/// 对应 Go `TestIssue18068`：重复执行小批量 merge join 不得卡死或丢失首行。
#[test]
fn index_lookup_merge_join_repeated_limit_does_not_hang() {
    for _ in 0..3 {
        let joiner = Joiner::new(
            JoinType::Inner,
            false,
            vec![Value::Null, Value::Null],
            Vec::new(),
            None,
            false,
            32,
        )
        .unwrap();
        let mut executor = IndexLookUpMergeJoin::new(
            (0..16)
                .map(|_| vec![Value::Int(1), Value::Int(10)])
                .collect(),
            vec![0],
            vec![0],
            Box::new(Builder {
                rows: (0..16)
                    .map(|_| vec![Value::Int(1), Value::Int(20)])
                    .collect(),
                key_columns: vec![0],
            }),
            joiner,
            1,
            32,
        )
        .unwrap();
        let result = executor.next(1).unwrap();
        assert!(result.error.is_none());
        assert_eq!(result.rows.len(), 1);
        executor.close();
    }
}

/// 对应 Go `TestIssue54064`：多列索引键按外侧 `(y,z,x)` 与内侧 `(y,z,x)` 对齐。
#[test]
fn index_lookup_merge_join_multi_key_index_order_matches() {
    let outer = vec![
        vec![
            Value::Int(1),
            Value::Text("RW ".into()),
            Value::Text("CN000".into()),
            Value::Text("123".into()),
        ],
        vec![
            Value::Int(2),
            Value::Text("123".into()),
            Value::Text("CN000".into()),
            Value::Text("456".into()),
        ],
    ];
    let inner = vec![
        vec![
            Value::Text("CN000".into()),
            Value::Text("123".into()),
            Value::Text("RW ".into()),
        ],
        vec![
            Value::Text("CN000".into()),
            Value::Text("456".into()),
            Value::Text("123".into()),
        ],
    ];
    let rows = run_merge_join(outer, inner, vec![2, 3, 1], vec![0, 1, 2], 3);
    assert_eq!(
        rows,
        vec![
            vec![
                Value::Int(1),
                Value::Text("RW ".into()),
                Value::Text("CN000".into()),
                Value::Text("123".into()),
                Value::Text("CN000".into()),
                Value::Text("123".into()),
                Value::Text("RW ".into()),
            ],
            vec![
                Value::Int(2),
                Value::Text("123".into()),
                Value::Text("CN000".into()),
                Value::Text("456".into()),
                Value::Text("CN000".into()),
                Value::Text("456".into()),
                Value::Text("123".into()),
            ],
        ]
    );
}
