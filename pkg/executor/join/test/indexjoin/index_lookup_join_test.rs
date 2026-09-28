// Copyright 2026 AsterSQL.

// Index Lookup Join（索引查找连接）的真实执行器语义用例。
//
// 索引查找连接保留外表每一行，并按连接键在内表侧查找匹配值；
// 这里直接驱动 crate 的 IndexLookUpJoin，并用内表 builder 模拟索引回表。

use astersql_executor_join::index_lookup_join::{
    IndexJoinExecutorBuilder, IndexJoinLookupContent, IndexLookUpJoin, InnerCtx, OuterCtx,
};
use astersql_executor_join::joiner::{JoinType, Joiner, Predicate, Row};
use astersql_executor_join::row_table_builder::Value;
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicUsize, Ordering},
};
use std::thread;

fn row(key: i64, payload: i64) -> Row {
    vec![Value::Int(key), Value::Int(payload)]
}

fn null_row(payload: i64) -> Row {
    vec![Value::Null, Value::Int(payload)]
}

struct Builder {
    rows: Vec<Row>,
    key_columns: Vec<usize>,
    calls: Option<Arc<AtomicUsize>>,
    error: Option<String>,
}

impl IndexJoinExecutorBuilder for Builder {
    fn build(&self, lookup_contents: &[IndexJoinLookupContent]) -> Result<Vec<Row>, String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        if let Some(calls) = &self.calls {
            calls.fetch_add(1, Ordering::Relaxed);
        }
        Ok(self
            .rows
            .iter()
            .filter(|candidate| {
                let candidate_key = self
                    .key_columns
                    .iter()
                    .map(|column| candidate[*column].clone())
                    .collect::<Row>();
                lookup_contents
                    .iter()
                    .any(|content| content.keys == candidate_key)
            })
            .cloned()
            .collect())
    }
}

struct ConcurrentBuilder {
    rows: Arc<Vec<Row>>,
    barrier: Arc<Barrier>,
    calls: Arc<AtomicUsize>,
}

impl IndexJoinExecutorBuilder for ConcurrentBuilder {
    fn build(&self, lookup_contents: &[IndexJoinLookupContent]) -> Result<Vec<Row>, String> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.barrier.wait();
        Ok(self
            .rows
            .iter()
            .filter(|candidate| {
                lookup_contents
                    .iter()
                    .any(|content| content.keys == vec![candidate[0].clone()])
            })
            .cloned()
            .collect())
    }
}

struct FailAfterBuilder {
    rows: Vec<Row>,
    calls: Arc<AtomicUsize>,
    fail_on_call: usize,
}

impl IndexJoinExecutorBuilder for FailAfterBuilder {
    fn build(&self, lookup_contents: &[IndexJoinLookupContent]) -> Result<Vec<Row>, String> {
        let call = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
        if call == self.fail_on_call {
            return Err("inner worker failed".into());
        }
        Ok(self
            .rows
            .iter()
            .filter(|candidate| {
                lookup_contents
                    .iter()
                    .any(|content| content.keys == vec![candidate[0].clone()])
            })
            .cloned()
            .collect())
    }
}

fn execute_join(
    join_type: JoinType,
    outer_rows: Vec<Row>,
    inner_rows: Vec<Row>,
    key_columns: Vec<usize>,
    null_safe: bool,
    outer_is_right: bool,
    initial_batch_size: usize,
    max_batch_size: usize,
) -> Result<Vec<Row>, String> {
    execute_join_with_conditions(
        join_type,
        outer_rows,
        inner_rows,
        key_columns,
        null_safe,
        outer_is_right,
        initial_batch_size,
        max_batch_size,
        Vec::new(),
    )
}

#[allow(clippy::too_many_arguments)]
fn execute_join_with_conditions(
    join_type: JoinType,
    outer_rows: Vec<Row>,
    inner_rows: Vec<Row>,
    key_columns: Vec<usize>,
    null_safe: bool,
    outer_is_right: bool,
    initial_batch_size: usize,
    max_batch_size: usize,
    conditions: Vec<Predicate>,
) -> Result<Vec<Row>, String> {
    let default_inner = vec![Value::Null, Value::Null];
    let joiner = Joiner::new(
        join_type,
        outer_is_right,
        default_inner,
        conditions,
        None,
        false,
        32,
    )?;
    let mut join = IndexLookUpJoin::new(
        OuterCtx {
            rows: outer_rows,
            key_columns: key_columns.clone(),
            filters: Vec::new(),
        },
        InnerCtx {
            builder: Box::new(Builder {
                rows: inner_rows,
                key_columns: key_columns.clone(),
                calls: None,
                error: None,
            }),
            key_columns,
            key_column_ids: vec![1, 2],
            null_safe,
        },
        joiner,
        true,
        initial_batch_size,
        max_batch_size,
    )?;
    let mut output = Vec::new();
    loop {
        let batch = join.next(32)?;
        if batch.is_empty() {
            break;
        }
        output.extend(batch);
    }
    Ok(output)
}

/// 对应 Go `TestIndexJoinNullEQ`：验证真实 IndexLookUpJoin 的 NULL-safe 等值匹配。
#[test]
fn index_lookup_join_null_safe_matches_real_rows() {
    let rows = execute_join(
        JoinType::Inner,
        vec![row(1, 10), null_row(11), row(2, 12)],
        vec![row(1, 20), null_row(21), null_row(22), row(3, 23)],
        vec![0],
        true,
        false,
        8,
        8,
    )
    .unwrap();
    assert_eq!(
        rows,
        vec![
            vec![Value::Int(1), Value::Int(10), Value::Int(1), Value::Int(20)],
            vec![Value::Null, Value::Int(11), Value::Null, Value::Int(21)],
            vec![Value::Null, Value::Int(11), Value::Null, Value::Int(22)],
        ]
    );
}

#[test]
fn index_lookup_join_hang_regression_repeated_next_after_exhaustion() {
    let overflow: Predicate = Arc::new(|_| Err("BIGINT UNSIGNED value is out of range".into()));
    let mut join = IndexLookUpJoin::new(
        OuterCtx {
            rows: (0..5).map(|payload| row(1, payload)).collect(),
            key_columns: vec![0],
            filters: Vec::new(),
        },
        InnerCtx {
            builder: Box::new(Builder {
                rows: vec![row(1, 20)],
                key_columns: vec![0],
                calls: None,
                error: None,
            }),
            key_columns: vec![0],
            key_column_ids: vec![1],
            null_safe: false,
        },
        Joiner::new(
            JoinType::LeftOuter,
            false,
            vec![Value::Null, Value::Null],
            vec![overflow],
            None,
            false,
            8,
        )
        .unwrap(),
        true,
        1,
        1,
    )
    .unwrap();
    for _ in 0..5 {
        assert_eq!(
            join.next(1).unwrap_err(),
            "BIGINT UNSIGNED value is out of range"
        );
    }
    assert!(join.next(1).unwrap().is_empty());
    join.close();
    assert_eq!(
        join.next(1).unwrap_err(),
        "cannot reopen closed index lookup join"
    );
}

#[test]
fn index_lookup_join_null_safe_equal_matches_null_rows() {
    let rows = execute_join(
        JoinType::LeftOuter,
        vec![row(1, 10), null_row(11), row(2, 12)],
        vec![row(1, 20), null_row(21), null_row(22), row(3, 23)],
        vec![0],
        true,
        false,
        1,
        2,
    )
    .unwrap();
    assert_eq!(
        rows,
        vec![
            vec![Value::Int(1), Value::Int(10), Value::Int(1), Value::Int(20)],
            vec![Value::Null, Value::Int(11), Value::Null, Value::Int(21)],
            vec![Value::Null, Value::Int(11), Value::Null, Value::Int(22)],
            vec![Value::Int(2), Value::Int(12), Value::Null, Value::Null],
        ]
    );
}

#[test]
fn index_lookup_join_null_safe_multi_key_matches_only_equal_tuples() {
    let outer = vec![
        vec![Value::Null, Value::Null],
        vec![Value::Null, Value::Int(1)],
        vec![Value::Null, Value::Int(2)],
        vec![Value::Int(1), Value::Int(1)],
        vec![Value::Int(1), Value::Int(2)],
        vec![Value::Int(2), Value::Null],
    ];
    let inner = vec![
        vec![Value::Null, Value::Null],
        vec![Value::Null, Value::Int(1)],
        vec![Value::Null, Value::Int(2)],
        vec![Value::Null, Value::Int(3)],
        vec![Value::Int(1), Value::Int(1)],
        vec![Value::Int(1), Value::Int(2)],
        vec![Value::Int(2), Value::Null],
        vec![Value::Int(2), Value::Int(1)],
    ];
    let regular_equal_on_second_key: Predicate = Arc::new(|joined| {
        let left = &joined[1];
        let right = &joined[3];
        if matches!(left, Value::Null) || matches!(right, Value::Null) {
            Ok(None)
        } else {
            Ok(Some(left == right))
        }
    });
    let mixed_null_semantics = execute_join_with_conditions(
        JoinType::Inner,
        outer.clone(),
        inner.clone(),
        vec![0, 1],
        true,
        false,
        8,
        8,
        vec![regular_equal_on_second_key],
    )
    .unwrap();
    assert_eq!(
        mixed_null_semantics,
        vec![
            vec![Value::Null, Value::Int(1), Value::Null, Value::Int(1)],
            vec![Value::Null, Value::Int(2), Value::Null, Value::Int(2)],
            vec![Value::Int(1), Value::Int(1), Value::Int(1), Value::Int(1)],
            vec![Value::Int(1), Value::Int(2), Value::Int(1), Value::Int(2)],
        ]
    );

    let all_null_safe =
        execute_join(JoinType::Inner, outer, inner, vec![0, 1], true, false, 8, 8).unwrap();
    assert_eq!(
        all_null_safe,
        vec![
            vec![Value::Null, Value::Null, Value::Null, Value::Null],
            vec![Value::Null, Value::Int(1), Value::Null, Value::Int(1)],
            vec![Value::Null, Value::Int(2), Value::Null, Value::Int(2)],
            vec![Value::Int(1), Value::Int(1), Value::Int(1), Value::Int(1)],
            vec![Value::Int(1), Value::Int(2), Value::Int(1), Value::Int(2)],
            vec![Value::Int(2), Value::Null, Value::Int(2), Value::Null],
        ]
    );
}

#[test]
fn index_lookup_join_unique_key_and_duplicate_key_results_are_preserved() {
    let rows = execute_join(
        JoinType::Inner,
        vec![row(1, 10), row(2, 20)],
        vec![row(1, 11), row(1, 12), row(2, 21)],
        vec![0],
        false,
        false,
        8,
        8,
    )
    .unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0][3], Value::Int(11));
    assert_eq!(rows[1][3], Value::Int(12));
    assert_eq!(rows[2][3], Value::Int(21));
}

/// 对应 Go `TestIndexJoinNullEQUniqueKey`：唯一键路径仍须保留 NULL-safe 的重复命中。
#[test]
fn index_lookup_join_unique_key_null_matches_all_inner_rows() {
    let rows = execute_join(
        JoinType::Inner,
        vec![row(1, 10), null_row(11), row(2, 12)],
        vec![row(1, 20), null_row(21), null_row(22), row(3, 23)],
        vec![0],
        true,
        false,
        8,
        8,
    )
    .unwrap();
    assert_eq!(
        rows,
        vec![
            vec![Value::Int(1), Value::Int(10), Value::Int(1), Value::Int(20)],
            vec![Value::Null, Value::Int(11), Value::Null, Value::Int(21)],
            vec![Value::Null, Value::Int(11), Value::Null, Value::Int(22)],
        ]
    );
}

#[test]
fn index_lookup_join_null_safe_outer_join_keeps_both_outer_sides() {
    let left = execute_join(
        JoinType::LeftOuter,
        vec![null_row(10), row(1, 11), row(2, 12)],
        vec![null_row(20), null_row(21), row(1, 22), row(3, 23)],
        vec![0],
        true,
        false,
        8,
        8,
    )
    .unwrap();
    assert_eq!(left.len(), 4);
    assert_eq!(left[0][3], Value::Int(20));
    assert_eq!(left[1][3], Value::Int(21));
    assert_eq!(left[3][2], Value::Null);

    let right = execute_join(
        JoinType::RightOuter,
        vec![null_row(20), null_row(21), row(1, 22), row(3, 23)],
        vec![null_row(10), row(1, 11), row(2, 12)],
        vec![0],
        true,
        true,
        8,
        8,
    )
    .unwrap();
    assert_eq!(
        right,
        vec![
            vec![Value::Null, Value::Int(10), Value::Null, Value::Int(20)],
            vec![Value::Null, Value::Int(10), Value::Null, Value::Int(21)],
            vec![Value::Int(1), Value::Int(11), Value::Int(1), Value::Int(22)],
            vec![Value::Null, Value::Null, Value::Int(3), Value::Int(23)],
        ]
    );
}

#[test]
fn index_lookup_join_partition_like_batches_preserve_outer_order_and_filter() {
    let filter: Predicate = Arc::new(|row| {
        Ok(Some(
            matches!(row.first(), Some(Value::Int(value)) if *value >= 2),
        ))
    });
    let joiner = Joiner::new(
        JoinType::LeftOuter,
        false,
        vec![Value::Null, Value::Null],
        vec![],
        None,
        false,
        8,
    )
    .unwrap();
    let mut join = IndexLookUpJoin::new(
        OuterCtx {
            rows: vec![row(1, 10), row(2, 20), row(3, 30)],
            key_columns: vec![0],
            filters: vec![filter],
        },
        InnerCtx {
            builder: Box::new(Builder {
                rows: vec![row(1, 11), row(2, 22), row(3, 33)],
                key_columns: vec![0],
                calls: None,
                error: None,
            }),
            key_columns: vec![0],
            key_column_ids: vec![1],
            null_safe: false,
        },
        joiner,
        true,
        1,
        2,
    )
    .unwrap();
    let mut rows = Vec::new();
    loop {
        let batch = join.next(1).unwrap();
        if batch.is_empty() {
            break;
        }
        rows.extend(batch);
    }
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0][0], Value::Int(1));
    assert_eq!(rows[1][2], Value::Int(2));
    assert_eq!(rows[2][2], Value::Int(3));
}

/// 对应 Go `TestPartitionTableIndexJoinAndIndexReader`：多批次筛选后的连接结果
/// 与同一组未分区数据的嵌套循环结果一致。
#[test]
fn index_lookup_join_partition_like_workload_matches_reference_rows() {
    let mut random_state = 1_u64;
    let mut next_value = || {
        random_state = random_state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((random_state >> 32) % 512) as i64
    };
    let source = (0..512)
        .map(|_| (next_value(), next_value()))
        .collect::<Vec<_>>();
    let outer = source.iter().map(|(a, b)| row(*a, *b)).collect::<Vec<_>>();
    let all_inner = source.iter().map(|(a, b)| row(*b, *a)).collect::<Vec<_>>();
    let ranges = (0..512)
        .map(|_| {
            let left = next_value();
            let right = next_value();
            (left.min(right), left.max(right))
        })
        .collect::<Vec<_>>();

    for (lower, upper) in ranges {
        let inner = all_inner
            .iter()
            .filter(|candidate| {
                matches!(&candidate[0], Value::Int(key) if lower <= *key && *key <= upper)
            })
            .cloned()
            .collect::<Vec<_>>();
        let expected = outer
            .iter()
            .flat_map(|left| {
                inner.iter().filter_map(|right| {
                    if left[0] != right[0] {
                        return None;
                    }
                    let mut joined = left.clone();
                    joined.extend(right.iter().cloned());
                    Some(joined)
                })
            })
            .collect::<Vec<_>>();
        let actual = execute_join(
            JoinType::Inner,
            outer.clone(),
            inner,
            vec![0],
            false,
            false,
            8,
            64,
        )
        .unwrap();
        assert_eq!(actual, expected, "range {lower}..={upper}");
    }
}

/// 对应 Go `TestIssue16887`：大批量权限行只能返回 WHERE 范围内的 70 个匹配。
#[test]
fn index_lookup_join_issue_16887_returns_all_matching_rows() {
    let roles = (1..=5).map(|role| row(role, role * 10)).collect();
    let mut permissions = (1..=67)
        .map(|permission| row(1, permission))
        .collect::<Vec<_>>();
    permissions.extend([row(4, 5), row(4, 6), row(4, 7)]);

    let rows = execute_join(
        JoinType::Inner,
        roles,
        permissions,
        vec![0],
        false,
        false,
        1,
        64,
    )
    .unwrap();
    let mut expected = (1..=67)
        .map(|permission| {
            vec![
                Value::Int(1),
                Value::Int(10),
                Value::Int(1),
                Value::Int(permission),
            ]
        })
        .collect::<Vec<_>>();
    expected.extend((5..=7).map(|permission| {
        vec![
            Value::Int(4),
            Value::Int(40),
            Value::Int(4),
            Value::Int(permission),
        ]
    }));
    assert_eq!(rows, expected);
}

#[test]
fn index_lookup_join_propagates_inner_builder_errors() {
    let joiner = Joiner::new(
        JoinType::Inner,
        false,
        Vec::new(),
        Vec::new(),
        None,
        false,
        8,
    )
    .unwrap();
    let mut join = IndexLookUpJoin::new(
        OuterCtx {
            rows: vec![row(1, 10)],
            key_columns: vec![0],
            filters: Vec::new(),
        },
        InnerCtx {
            builder: Box::new(Builder {
                rows: Vec::new(),
                key_columns: vec![0],
                calls: None,
                error: Some("builder failed".into()),
            }),
            key_columns: vec![0],
            key_column_ids: vec![1],
            null_safe: false,
        },
        joiner,
        true,
        1,
        1,
    )
    .unwrap();
    assert_eq!(join.next(1).unwrap_err(), "builder failed");
    join.close();
    assert_eq!(
        join.next(1).unwrap_err(),
        "cannot reopen closed index lookup join"
    );
}

/// 对应 Go `TestIssue54055`：前两批成功、第三批 inner worker 失败时传播错误并可关闭。
#[test]
fn index_lookup_join_propagates_delayed_inner_worker_error_and_closes() {
    let calls = Arc::new(AtomicUsize::new(0));
    let joiner = Joiner::new(
        JoinType::Inner,
        false,
        Vec::new(),
        Vec::new(),
        None,
        false,
        32,
    )
    .unwrap();
    let mut join = IndexLookUpJoin::new(
        OuterCtx {
            rows: vec![row(1, 1), row(2, 2), row(3, 3)],
            key_columns: vec![0],
            filters: Vec::new(),
        },
        InnerCtx {
            builder: Box::new(FailAfterBuilder {
                rows: vec![row(1, 1), row(2, 2), row(3, 3)],
                calls: Arc::clone(&calls),
                fail_on_call: 3,
            }),
            key_columns: vec![0],
            key_column_ids: vec![1],
            null_safe: false,
        },
        joiner,
        true,
        1,
        1,
    )
    .unwrap();

    assert_eq!(join.next(32).unwrap_err(), "inner worker failed");
    assert_eq!(calls.load(Ordering::Relaxed), 3);
    join.close();
    assert_eq!(
        join.next(1).unwrap_err(),
        "cannot reopen closed index lookup join"
    );
}

/// 对应 Go `TestIssue54688`：连接探测期间收到取消错误后可正常关闭，不挂起或继续产出。
#[test]
fn index_lookup_join_cancellation_during_probe_closes_cleanly() {
    let probes = Arc::new(AtomicUsize::new(0));
    let cancellation_probe = Arc::clone(&probes);
    let cancel_after_first_match: Predicate = Arc::new(move |_| {
        if cancellation_probe.fetch_add(1, Ordering::Relaxed) == 0 {
            Ok(Some(true))
        } else {
            Err("context canceled".into())
        }
    });
    let joiner = Joiner::new(
        JoinType::Inner,
        false,
        Vec::new(),
        vec![cancel_after_first_match],
        None,
        false,
        32,
    )
    .unwrap();
    let mut join = IndexLookUpJoin::new(
        OuterCtx {
            rows: vec![row(1, 10)],
            key_columns: vec![0],
            filters: Vec::new(),
        },
        InnerCtx {
            builder: Box::new(Builder {
                rows: (0..64).map(|payload| row(1, payload)).collect(),
                key_columns: vec![0],
                calls: None,
                error: None,
            }),
            key_columns: vec![0],
            key_column_ids: vec![1],
            null_safe: false,
        },
        joiner,
        true,
        1,
        1,
    )
    .unwrap();

    assert_eq!(join.next(32).unwrap_err(), "context canceled");
    assert_eq!(probes.load(Ordering::Relaxed), 2);
    join.close();
    assert_eq!(
        join.next(1).unwrap_err(),
        "cannot reopen closed index lookup join"
    );
}

#[test]
fn index_lookup_join_large_duplicate_batch_returns_every_match() {
    let inner = (0..256)
        .flat_map(|_| (1..=16).map(|key| row(key, key)))
        .collect();
    let rows = execute_join(
        JoinType::Inner,
        (1..=16).map(|key| row(key, key)).collect(),
        inner,
        vec![0],
        false,
        false,
        1_000_000,
        1_000_000,
    )
    .unwrap();
    assert_eq!(rows.len(), 4_096);
    for key in 1..=16 {
        assert_eq!(
            rows.iter()
                .filter(|joined| joined[0] == Value::Int(key))
                .count(),
            256
        );
    }
}

#[test]
fn index_lookup_join_repeated_inner_builds_keep_results_stable() {
    let calls = Arc::new(AtomicUsize::new(0));
    let builder = Builder {
        rows: vec![row(1, 2), row(2, 4)],
        key_columns: vec![0],
        calls: Some(Arc::clone(&calls)),
        error: None,
    };
    let joiner = Joiner::new(
        JoinType::Inner,
        false,
        Vec::new(),
        Vec::new(),
        None,
        false,
        8,
    )
    .unwrap();
    let mut join = IndexLookUpJoin::new(
        OuterCtx {
            rows: vec![row(1, 10), row(2, 20), row(1, 30)],
            key_columns: vec![0],
            filters: Vec::new(),
        },
        InnerCtx {
            builder: Box::new(builder),
            key_columns: vec![0],
            key_column_ids: vec![1],
            null_safe: false,
        },
        joiner,
        true,
        1,
        1,
    )
    .unwrap();
    let mut rows = Vec::new();
    loop {
        let batch = join.next(8).unwrap();
        if batch.is_empty() {
            break;
        }
        rows.extend(batch);
    }
    assert_eq!(rows.len(), 3);
    assert_eq!(calls.load(Ordering::Relaxed), 3);
}

/// 对应 Go `TestIndexJoinInnerCTEStorageConcurrentBuild`：共享内表存储可被四条
/// 并发 lookup 流重复构建，且 20 轮均返回完整结果。
#[test]
fn index_lookup_join_shared_inner_storage_supports_concurrent_repeated_builds() {
    const CONCURRENCY: usize = 4;
    const ROW_COUNT: usize = 64;
    const ROUNDS: usize = 20;

    let inner_rows = Arc::new(
        (1..=ROW_COUNT)
            .map(|value| row(value as i64, value as i64))
            .collect::<Vec<_>>(),
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(CONCURRENCY));
    let handles = (0..CONCURRENCY)
        .map(|_| {
            let inner_rows = Arc::clone(&inner_rows);
            let calls = Arc::clone(&calls);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                for _ in 0..ROUNDS {
                    let joiner = Joiner::new(
                        JoinType::Inner,
                        false,
                        Vec::new(),
                        Vec::new(),
                        None,
                        false,
                        32,
                    )
                    .unwrap();
                    let mut join = IndexLookUpJoin::new(
                        OuterCtx {
                            rows: (1..=ROW_COUNT)
                                .map(|value| row(value as i64, value as i64))
                                .collect(),
                            key_columns: vec![0],
                            filters: Vec::new(),
                        },
                        InnerCtx {
                            builder: Box::new(ConcurrentBuilder {
                                rows: Arc::clone(&inner_rows),
                                barrier: Arc::clone(&barrier),
                                calls: Arc::clone(&calls),
                            }),
                            key_columns: vec![0],
                            key_column_ids: vec![1],
                            null_safe: false,
                        },
                        joiner,
                        true,
                        1,
                        1,
                    )
                    .unwrap();
                    let mut result = Vec::new();
                    loop {
                        let batch = join.next(32).unwrap();
                        if batch.is_empty() {
                            break;
                        }
                        result.extend(batch);
                    }
                    assert_eq!(result.len(), ROW_COUNT);
                }
            })
        })
        .collect::<Vec<_>>();

    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(
        calls.load(Ordering::Relaxed),
        CONCURRENCY * ROW_COUNT * ROUNDS
    );
}

#[test]
fn index_lookup_join_empty_inner_side_keeps_left_outer_rows() {
    let rows = execute_join(
        JoinType::LeftOuter,
        vec![row(1, 10), row(2, 20)],
        Vec::new(),
        vec![0],
        false,
        false,
        8,
        8,
    )
    .unwrap();
    assert_eq!(
        rows,
        [
            vec![Value::Int(1), Value::Int(10), Value::Null, Value::Null],
            vec![Value::Int(2), Value::Int(20), Value::Null, Value::Null]
        ]
    );
}
