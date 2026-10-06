// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 执行器运行时覆盖测试，对应 Go 的 executor benchmark。
//
// Real executor runtime coverage corresponding to Go's executor benchmarks.
//
// 用真实聚合/连接/窗口/排序/Limit 等执行器跑代表规模与（可选）全量 Go 矩阵，
// 验证迁移后行为与结果语义一致；`#[ignore]` 用例为全量规模门禁。

// Real executor runtime coverage corresponding to Go's executor benchmarks.

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_variables,
    unused_mut
)]

use crate::insert_common::{
    CompleteInsertErrorForColumn, CompleteLoadErrorForColumn, DmlErrorCause, InsertErrorKind,
};
use crate::physical_plan_runtime::ExecuteLimitRows;
use crate::slow_query::ReadLastLinesFromFile;
use astersql_executor_aggfuncs::{PartialResult, new_agg_partial_result_mapper};
use astersql_executor_aggregate::agg_hash_executor::{HashAggExec, HashAggInput};
use astersql_executor_aggregate::agg_stream_executor::StreamAggExec;
use astersql_executor_aggregate::agg_util::{AggKind, Aggregation, Value as AggValue};
use astersql_executor_join::hash_join_base::HashJoinContextBase;
use astersql_executor_join::hash_join_v1::{BuildWorkerV1, HashJoinCtxV1, HashJoinV1Exec};
use astersql_executor_join::hash_join_v2::{HashJoinCtxV2, HashJoinV2Exec};
use astersql_executor_join::index_lookup_hash_join::IndexNestedLoopHashJoin;
use astersql_executor_join::index_lookup_join::{
    IndexJoinExecutorBuilder, IndexJoinLookupContent, IndexLookUpJoin, InnerCtx as IndexInnerCtx,
    OuterCtx as IndexOuterCtx, encode_key,
};
use astersql_executor_join::index_lookup_merge_join::IndexLookUpMergeJoin;
use astersql_executor_join::joiner::{JoinType, Joiner};
use astersql_executor_join::merge_join::{MergeJoinExec, MergeJoinTable, ShuffleMergeJoinExec};
use astersql_executor_join::row_table_builder::{Chunk as JoinChunk, Value as JoinValue};
use astersql_executor_sortexec::sort::VecRowSource;
use astersql_executor_sortexec::{DataChunk, Limit, Row, SortExec, SortKey, SortValue, TopNExec};
use astersql_executor_windows::builder::{PhysicalWindowPlan, build as build_window};
use astersql_executor_windows::window::{
    Average, BitXor, BoundType as WindowBoundType, Chunk as WindowChunk, CountRows, Decimal,
    DecimalAverage, DecimalSum, ExecContext as WindowExecContext, FrameBound as WindowFrameBound,
    FrameType as WindowFrameType, Lag, MaxValue, MinValue, OrderBy as WindowOrderBy, RowNumber,
    Sum, Value as WindowValue, VecChunkExecutor, WindowFrame,
};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// 构造 TopN 输入行：键按降序排列以便验证取最小 N 个。
fn top_n_input(rows: usize) -> Vec<Row> {
    (0..rows)
        .map(|offset| {
            let key = (rows - offset - 1) as i64;
            Row(vec![
                SortValue::Int(key),
                SortValue::Int(key * 2),
                SortValue::Int(-key),
            ])
        })
        .collect()
}

/// 抽干 TopN 执行器输出并 Close。
fn drain_top_n(executor: &mut TopNExec) -> Vec<Row> {
    let mut result = Vec::new();
    loop {
        let chunk = executor.Next(1024).expect("execute TopN benchmark case");
        if chunk.is_empty() {
            break;
        }
        result.extend(chunk.rows);
    }
    executor.Close().expect("close TopN benchmark case");
    result
}

/// 对比内联投影与外层 Projection 两条 TopN 路径。
fn top_n_benchmark_case(using_inline_projection: bool) -> Vec<Row> {
    let child = Box::new(VecRowSource::new(vec![DataChunk::new(top_n_input(
        100_000,
    ))]));
    let mut executor = TopNExec::new(
        child,
        vec![SortKey::asc(0)],
        Limit {
            Offset: 0,
            Count: 10,
        },
        None,
        4,
        1024,
        -1,
    );
    // 内联投影只保留子节点需要的列；否则先 TopN 再外层 Project
    if using_inline_projection {
        executor.SetColumnIdxsUsedByChild(vec![1]);
        drain_top_n(&mut executor)
    } else {
        let rows = drain_top_n(&mut executor);
        crate::projection::ProjectRows(rows, &[1]).expect("execute outer Projection benchmark case")
    }
}

#[test]
/// TopN：内联/外层投影结果应一致。
fn benchmark_top_n_exec_matches_go_inline_and_outer_projection_paths() {
    let expected = (0..10)
        .map(|value| Row(vec![SortValue::Int(value * 2)]))
        .collect::<Vec<_>>();
    assert_eq!(top_n_benchmark_case(true), expected);
    assert_eq!(top_n_benchmark_case(false), expected);
}

#[test]
/// StreamAgg：未排序输入在指定并发下先排序再聚合。
fn benchmark_stream_aggregate_sorts_unsorted_source_with_requested_concurrency() {
    let input = vec![vec![
        vec![AggValue::Float(1.0), AggValue::Integer(2)],
        vec![AggValue::Float(2.0), AggValue::Integer(1)],
        vec![AggValue::Float(3.0), AggValue::Integer(2)],
        vec![AggValue::Float(4.0), AggValue::Integer(1)],
    ]];
    let mut executor = build_stream_aggregate_executor(
        input,
        vec![1],
        vec![Aggregation::new(AggKind::Sum, Some(0))],
        2,
        4,
        false,
    );
    executor
        .open()
        .expect("open unsorted stream aggregate benchmark case");
    let mut rows = Vec::new();
    while let Some(chunk) = executor
        .next()
        .expect("execute unsorted stream aggregate benchmark case")
    {
        rows.extend(chunk);
    }
    assert_eq!(
        rows,
        vec![
            vec![AggValue::Integer(1), AggValue::Float(6.0)],
            vec![AggValue::Integer(2), AggValue::Float(4.0)],
        ]
    );
}

/// 构建 StreamAgg；必要时先经 SortExec 排序。
fn build_stream_aggregate_executor(
    input: Vec<Vec<Vec<AggValue>>>,
    group_columns: Vec<usize>,
    aggregations: Vec<Aggregation>,
    max_chunk_size: usize,
    concurrency: usize,
    data_source_sorted: bool,
) -> StreamAggExec {
    // 未排序时先用 SortExec 按分组列排序，模拟 Go shuffle/stream 前置
    let input = if data_source_sorted {
        input
    } else {
        let sort_chunks = input
            .into_iter()
            .map(|chunk| {
                DataChunk::new(
                    chunk
                        .into_iter()
                        .map(|row| {
                            Row(row
                                .into_iter()
                                .map(|value| match value {
                                    AggValue::Null => SortValue::Null,
                                    AggValue::Integer(value) => SortValue::Int(value),
                                    AggValue::Float(value) => SortValue::Float(value),
                                    AggValue::Text(value) => SortValue::Bytes(value.into_bytes()),
                                    AggValue::Bytes(value) => SortValue::Bytes(value),
                                    AggValue::Bool(value) => SortValue::UInt(u64::from(value)),
                                })
                                .collect())
                        })
                        .collect(),
                )
            })
            .collect();
        let mut sort = SortExec::new(
            Box::new(VecRowSource::new(sort_chunks)),
            group_columns.iter().copied().map(SortKey::asc).collect(),
            concurrency,
            max_chunk_size,
            -1,
        );
        let mut sorted = Vec::new();
        loop {
            let chunk = sort
                .Next(max_chunk_size)
                .expect("execute Sort child for stream aggregate benchmark");
            if chunk.is_empty() {
                break;
            }
            sorted.push(
                chunk
                    .rows
                    .into_iter()
                    .map(|row| {
                        row.0
                            .into_iter()
                            .map(|value| match value {
                                SortValue::Null => AggValue::Null,
                                SortValue::Int(value) => AggValue::Integer(value),
                                SortValue::UInt(value) => AggValue::Bool(value != 0),
                                SortValue::Float(value) => AggValue::Float(value),
                                SortValue::Bytes(value) => AggValue::Bytes(value),
                            })
                            .collect()
                    })
                    .collect(),
            );
        }
        sort.Close()
            .expect("close Sort child for stream aggregate benchmark");
        sorted
    };
    StreamAggExec::new(input, group_columns, aggregations, max_chunk_size)
}

#[derive(Clone, Copy, Debug)]
/// 非 DISTINCT 聚合执行器变体（Hash / Stream / Shuffle）。
enum NonDistinctAggExecutor {
    Hash,
    Stream,
}

/// 生成非 DISTINCT 聚合输入（按 NDV 循环键）。
fn non_distinct_aggregate_input(
    row_count: usize,
    ndv: usize,
    data_source_sorted: bool,
) -> Vec<Vec<Vec<AggValue>>> {
    let ndv = ndv.max(1).min(row_count.max(1));
    let rows = (0..row_count)
        .map(|offset| {
            let group = if data_source_sorted {
                offset.saturating_mul(ndv) / row_count.max(1)
            } else {
                offset % ndv
            };
            vec![
                AggValue::Float((offset + 1) as f64),
                AggValue::Integer(group as i64),
            ]
        })
        .collect::<Vec<_>>();
    rows.chunks(4_096).map(<[_]>::to_vec).collect()
}

/// 计算非 DISTINCT SUM 的期望分组结果。
fn expected_non_distinct_sums(
    input: &[Vec<Vec<AggValue>>],
) -> std::collections::BTreeMap<i64, f64> {
    let mut expected = std::collections::BTreeMap::new();
    for row in input.iter().flatten() {
        let [AggValue::Float(value), AggValue::Integer(group)] = row.as_slice() else {
            panic!("non-distinct aggregate input must be [float, integer]");
        };
        *expected.entry(*group).or_insert(0.0) += value;
    }
    expected
}

/// 将聚合输出整理为 group_key → sum 映射。
fn non_distinct_result_sums(rows: Vec<Vec<AggValue>>) -> std::collections::BTreeMap<i64, f64> {
    rows.into_iter()
        .map(|row| match row.as_slice() {
            [AggValue::Integer(group), AggValue::Float(value)] => (*group, *value),
            _ => panic!("non-distinct aggregate result must be [integer, float]: {row:?}"),
        })
        .collect()
}

/// 运行单组非 DISTINCT 聚合用例并校验结果。
fn run_non_distinct_aggregate_case(
    executor_kind: NonDistinctAggExecutor,
    row_count: usize,
    ndv: usize,
    concurrency: usize,
    data_source_sorted: bool,
) {
    let input = non_distinct_aggregate_input(row_count, ndv, data_source_sorted);
    let expected = expected_non_distinct_sums(&input);
    let aggregation = Aggregation::new(AggKind::Sum, Some(0));
    let rows = match executor_kind {
        NonDistinctAggExecutor::Hash => {
            let mut executor = HashAggExec::new(
                HashAggInput {
                    chunks: input,
                    group_columns: vec![1],
                    aggregations: vec![aggregation],
                },
                concurrency,
                concurrency,
                64,
                None,
            );
            executor.open();
            let mut rows = Vec::new();
            while let Some(chunk) = executor
                .next()
                .expect("execute HashAgg benchmark matrix case")
            {
                rows.extend(chunk);
            }
            executor.close();
            rows
        }
        NonDistinctAggExecutor::Stream => {
            let mut executor = build_stream_aggregate_executor(
                input,
                vec![1],
                vec![aggregation],
                64,
                concurrency,
                data_source_sorted,
            );
            executor
                .open()
                .expect("open StreamAgg benchmark matrix case");
            let mut rows = Vec::new();
            while let Some(chunk) = executor
                .next()
                .expect("execute StreamAgg benchmark matrix case")
            {
                rows.extend(chunk);
            }
            executor.close();
            rows
        }
    };
    assert_eq!(
        non_distinct_result_sums(rows),
        expected,
        "executor={executor_kind:?}, rows={row_count}, ndv={ndv}, concurrency={concurrency}, sorted={data_source_sorted}"
    );
}

/// Go Shuffle StreamAgg 行数矩阵。
const GO_SHUFFLE_STREAM_AGG_ROWS: [usize; 4] = [10_000, 100_000, 1_000_000, 10_000_000];
/// Go Shuffle StreamAgg 并发矩阵。
const GO_SHUFFLE_STREAM_AGG_CONCURRENCIES: [usize; 4] = [1, 2, 4, 8];
/// Go HashAgg 行数矩阵。
const GO_HASH_AGG_ROWS: [usize; 3] = [100_000, 1_000_000, 10_000_000];
/// Go 聚合并发矩阵。
const GO_AGG_CONCURRENCIES: [usize; 7] = [1, 4, 8, 15, 20, 30, 40];
/// Go 聚合 NDV（不同值个数）矩阵。
const GO_AGG_NDVS: [usize; 7] = [10, 100, 1_000, 10_000, 100_000, 1_000_000, 10_000_000];

#[allow(dead_code)]
/// 遍历 Go 非 DISTINCT 聚合全矩阵（辅助，非默认测试）。
fn run_go_non_distinct_aggregate_matrix() {
    for rows in GO_SHUFFLE_STREAM_AGG_ROWS {
        for concurrency in GO_SHUFFLE_STREAM_AGG_CONCURRENCIES {
            for sorted in [false, true] {
                run_non_distinct_aggregate_case(
                    NonDistinctAggExecutor::Stream,
                    rows,
                    1_000,
                    concurrency,
                    sorted,
                );
            }
        }
    }
    for rows in GO_HASH_AGG_ROWS {
        for concurrency in GO_AGG_CONCURRENCIES {
            run_non_distinct_aggregate_case(
                NonDistinctAggExecutor::Hash,
                rows,
                1_000,
                concurrency,
                false,
            );
        }
    }
    for ndv in GO_AGG_NDVS {
        for executor in [NonDistinctAggExecutor::Hash, NonDistinctAggExecutor::Stream] {
            run_non_distinct_aggregate_case(executor, 10_000_000, ndv, 4, true);
        }
    }
    for concurrency in GO_AGG_CONCURRENCIES {
        for executor in [NonDistinctAggExecutor::Hash, NonDistinctAggExecutor::Stream] {
            run_non_distinct_aggregate_case(executor, 10_000_000, 1_000, concurrency, true);
        }
    }
}

#[test]
/// 非 DISTINCT 聚合：跑 Go 矩阵代表规模。
fn benchmark_non_distinct_aggregate_runs_real_go_matrix_representatives() {
    for sorted in [false, true] {
        run_non_distinct_aggregate_case(
            NonDistinctAggExecutor::Stream,
            GO_SHUFFLE_STREAM_AGG_ROWS[0],
            1_000,
            GO_SHUFFLE_STREAM_AGG_CONCURRENCIES[3],
            sorted,
        );
    }
    run_non_distinct_aggregate_case(
        NonDistinctAggExecutor::Hash,
        GO_HASH_AGG_ROWS[0],
        1_000,
        GO_AGG_CONCURRENCIES[6],
        false,
    );
    for executor in [NonDistinctAggExecutor::Hash, NonDistinctAggExecutor::Stream] {
        run_non_distinct_aggregate_case(executor, 100_000, GO_AGG_NDVS[3], 4, true);
    }
}

#[test]
#[ignore = "full Go benchmark scale; run explicitly for the migration evidence gate"]
/// 非 DISTINCT HashAgg 最大规模门禁（ignore）。
fn benchmark_non_distinct_hash_aggregate_runs_exact_go_maximum_scale() {
    run_non_distinct_aggregate_case(
        NonDistinctAggExecutor::Hash,
        GO_HASH_AGG_ROWS[2],
        1_000,
        4,
        false,
    );
}

#[test]
/// DISTINCT 聚合：跨真实执行器去重正确。
fn benchmark_distinct_aggregate_deduplicates_across_real_executors() {
    let input = vec![vec![
        vec![AggValue::Float(1.0), AggValue::Integer(0)],
        vec![AggValue::Float(1.0), AggValue::Integer(0)],
        vec![AggValue::Float(2.0), AggValue::Integer(0)],
        vec![AggValue::Float(2.0), AggValue::Integer(0)],
    ]];
    let aggregation = Aggregation::new_distinct(AggKind::Sum, Some(0));
    let mut hash = HashAggExec::new(
        HashAggInput {
            chunks: input.clone(),
            group_columns: vec![1],
            aggregations: vec![aggregation.clone()],
        },
        4,
        4,
        64,
        None,
    );
    hash.open();
    let mut hash_rows = Vec::new();
    while let Some(chunk) = hash
        .next()
        .expect("execute distinct HashAgg benchmark case")
    {
        hash_rows.extend(chunk);
    }
    let mut stream =
        build_stream_aggregate_executor(input, vec![1], vec![aggregation], 64, 4, false);
    stream
        .open()
        .expect("open distinct StreamAgg benchmark case");
    let mut stream_rows = Vec::new();
    while let Some(chunk) = stream
        .next()
        .expect("execute distinct StreamAgg benchmark case")
    {
        stream_rows.extend(chunk);
    }
    let expected = std::collections::BTreeMap::from([(0, 3.0)]);
    assert_eq!(non_distinct_result_sums(hash_rows), expected);
    assert_eq!(non_distinct_result_sums(stream_rows), expected);
}

/// Go DISTINCT 聚合行数矩阵。
const GO_AGG_DISTINCT_ROWS: [usize; 3] = [100_000, 1_000_000, 10_000_000];

/// 构造 DISTINCT 聚合输入块。
fn aggregate_distinct_input(row_count: usize) -> Vec<Vec<Vec<AggValue>>> {
    let rows = (0..row_count)
        .map(|offset| {
            let group = offset.saturating_mul(1_000) / row_count.max(1);
            vec![
                AggValue::Float((offset % 64) as f64),
                AggValue::Integer(group as i64),
            ]
        })
        .collect::<Vec<_>>();
    rows.chunks(4_096).map(<[_]>::to_vec).collect()
}

/// DISTINCT/普通聚合期望 SUM。
fn expected_aggregate_sums(
    input: &[Vec<Vec<AggValue>>],
    distinct: bool,
) -> std::collections::BTreeMap<i64, f64> {
    let mut values = std::collections::BTreeMap::<i64, std::collections::BTreeMap<u64, f64>>::new();
    for row in input.iter().flatten() {
        let [AggValue::Float(value), AggValue::Integer(group)] = row.as_slice() else {
            panic!("aggregate distinct input must be [float, integer]");
        };
        let group_values = values.entry(*group).or_default();
        if distinct {
            group_values.entry(value.to_bits()).or_insert(*value);
        } else {
            let occurrence = group_values.len() as u64;
            group_values.insert(occurrence, *value);
        }
    }
    values
        .into_iter()
        .map(|(group, values)| (group, values.into_values().sum()))
        .collect()
}

/// 运行 DISTINCT 聚合用例。
fn run_aggregate_distinct_case(
    executor_kind: NonDistinctAggExecutor,
    row_count: usize,
    distinct: bool,
) {
    let input = aggregate_distinct_input(row_count);
    let expected = expected_aggregate_sums(&input, distinct);
    let aggregation = if distinct {
        Aggregation::new_distinct(AggKind::Sum, Some(0))
    } else {
        Aggregation::new(AggKind::Sum, Some(0))
    };
    let rows = match executor_kind {
        NonDistinctAggExecutor::Hash => {
            let mut executor = HashAggExec::new(
                HashAggInput {
                    chunks: input,
                    group_columns: vec![1],
                    aggregations: vec![aggregation],
                },
                4,
                4,
                64,
                None,
            );
            executor.open();
            let mut rows = Vec::new();
            while let Some(chunk) = executor
                .next()
                .expect("execute aggregate distinct HashAgg matrix case")
            {
                rows.extend(chunk);
            }
            executor.close();
            rows
        }
        NonDistinctAggExecutor::Stream => {
            let mut executor =
                build_stream_aggregate_executor(input, vec![1], vec![aggregation], 64, 4, true);
            executor
                .open()
                .expect("open aggregate distinct StreamAgg matrix case");
            let mut rows = Vec::new();
            while let Some(chunk) = executor
                .next()
                .expect("execute aggregate distinct StreamAgg matrix case")
            {
                rows.extend(chunk);
            }
            executor.close();
            rows
        }
    };
    assert_eq!(
        non_distinct_result_sums(rows),
        expected,
        "executor={executor_kind:?}, rows={row_count}, distinct={distinct}"
    );
}

#[allow(dead_code)]
/// 遍历 Go DISTINCT 聚合矩阵。
fn run_go_aggregate_distinct_matrix() {
    for rows in GO_AGG_DISTINCT_ROWS {
        for executor in [NonDistinctAggExecutor::Hash, NonDistinctAggExecutor::Stream] {
            for distinct in [false, true] {
                run_aggregate_distinct_case(executor, rows, distinct);
            }
        }
    }
}

#[test]
/// DISTINCT 聚合：10 万行代表规模。
fn benchmark_aggregate_distinct_runs_exact_go_100000_row_representatives() {
    for executor in [NonDistinctAggExecutor::Hash, NonDistinctAggExecutor::Stream] {
        for distinct in [false, true] {
            run_aggregate_distinct_case(executor, GO_AGG_DISTINCT_ROWS[0], distinct);
        }
    }
}

#[test]
#[ignore = "full Go benchmark scale; run explicitly for the migration evidence gate"]
/// DISTINCT 聚合最大规模门禁（ignore）。
fn benchmark_aggregate_distinct_runs_exact_go_maximum_scale() {
    for executor in [NonDistinctAggExecutor::Hash, NonDistinctAggExecutor::Stream] {
        run_aggregate_distinct_case(executor, GO_AGG_DISTINCT_ROWS[2], true);
    }
}

/// 构造可产生多匹配的 HashJoin 输入块。
fn repeated_hash_join_rows(rows: usize) -> JoinChunk {
    let wide = vec![b'x'; 5 * 1024];
    (0..rows)
        .map(|_| vec![JoinValue::Int(1), JoinValue::Bytes(wide.clone())])
        .collect()
}

/// 构造代表性 HashJoin V1 执行器。
fn representative_hash_join_v1(children_used: Option<[Vec<usize>; 2]>) -> HashJoinV1Exec {
    let context = HashJoinCtxV1 {
        base: HashJoinContextBase::default(),
        join_type: JoinType::Inner,
        build_key_indices: vec![0],
        probe_key_indices: vec![0],
        null_aware: false,
        build_side_is_outer: false,
        concurrency: 4,
        max_chunk_size: 4,
    };
    let joiner = Joiner::new(
        JoinType::Inner,
        true,
        Vec::new(),
        Vec::new(),
        children_used,
        false,
        4,
    )
    .expect("build V1 benchmark joiner");
    HashJoinV1Exec::new(
        context,
        joiner,
        vec![repeated_hash_join_rows(6)],
        vec![repeated_hash_join_rows(6)],
    )
    .expect("build V1 benchmark executor")
}

#[test]
/// HashJoin V1：抽干超过一个输出 chunk 的全部匹配。
fn benchmark_hash_join_v1_drains_every_match_past_one_output_chunk() {
    let mut executor = representative_hash_join_v1(Some([vec![1], vec![]]));
    let rows = executor
        .execute_all()
        .expect("execute representative V1 hash join");
    assert_eq!(
        rows.len(),
        36,
        "six-by-six equal keys must produce every match"
    );
    assert!(rows.iter().all(|row| {
        matches!(row.as_slice(), [JoinValue::Bytes(value)] if value.len() == 5 * 1024)
    }));
}

#[test]
/// HashJoin V1：落盘 spill 后仍能抽干结果。
fn benchmark_hash_join_v1_disk_case_spills_and_still_drains_results() {
    let mut executor = representative_hash_join_v1(None);
    executor.SetMemoryLimit(Some(1));
    let rows = executor
        .execute_all()
        .expect("execute spilled V1 hash join");
    assert_eq!(rows.len(), 36);
    assert!(executor.IsSpillTriggered(), "disk=true must trigger spill");
    assert!(
        executor.DiskBytes() > 0,
        "spilled rows must be tracked on disk"
    );
}

/// 构造代表性 HashJoin V2（可选内存上限）。
fn representative_hash_join_v2(memory_limit: Option<i64>) -> HashJoinV2Exec {
    let context = HashJoinCtxV2::new(
        JoinType::Inner,
        vec![0],
        vec![0],
        false,
        false,
        4,
        4,
        memory_limit,
    )
    .expect("build V2 benchmark context");
    let joiner = Joiner::new(
        JoinType::Inner,
        true,
        Vec::new(),
        Vec::new(),
        Some([vec![1], vec![]]),
        false,
        4,
    )
    .expect("build V2 benchmark joiner");
    HashJoinV2Exec::new(
        context,
        joiner,
        vec![repeated_hash_join_rows(6)],
        vec![repeated_hash_join_rows(6)],
    )
    .expect("build V2 benchmark executor")
}

#[test]
/// HashJoin V2：内存与 spill 两条路径均可抽干。
fn benchmark_hash_join_v2_helper_drains_memory_and_spill_paths() {
    for memory_limit in [None, Some(1)] {
        let mut executor = representative_hash_join_v2(memory_limit);
        let rows = executor.execute_all().unwrap_or_else(|error| {
            panic!("execute V2 hash join helper ({memory_limit:?}): {error}")
        });
        assert_eq!(rows.len(), 36);
        if memory_limit.is_some() {
            assert!(
                executor.stats.spill.spilled_bytes.iter().sum::<i64>() > 0,
                "disk=true V2 helper must report spilled bytes"
            );
            assert!(
                executor.stats.spill.restored_bytes.iter().sum::<i64>() > 0,
                "disk=true V2 helper must restore spilled partitions"
            );
        }
    }
}

#[test]
/// Outer HashJoin：build 侧作为外表。
fn benchmark_outer_hash_join_uses_build_side_as_outer() {
    let context = HashJoinCtxV1 {
        base: HashJoinContextBase::default(),
        join_type: JoinType::RightOuter,
        build_key_indices: vec![0],
        probe_key_indices: vec![0],
        null_aware: false,
        build_side_is_outer: true,
        concurrency: 4,
        max_chunk_size: 2,
    };
    let joiner = Joiner::new(
        JoinType::RightOuter,
        true,
        vec![JoinValue::Null, JoinValue::Null],
        Vec::new(),
        None,
        false,
        2,
    )
    .expect("build outer hash joiner");
    let build = vec![
        vec![JoinValue::Int(0), JoinValue::Text("build-0".into())],
        vec![JoinValue::Int(1), JoinValue::Text("build-1".into())],
        vec![JoinValue::Int(2), JoinValue::Text("build-2".into())],
    ];
    let probe = vec![
        vec![JoinValue::Int(0), JoinValue::Text("probe-0".into())],
        vec![JoinValue::Int(1), JoinValue::Text("probe-1".into())],
    ];
    let mut executor = HashJoinV1Exec::new(context, joiner, vec![build], vec![probe])
        .expect("build outer V1 benchmark executor");
    let rows = executor.execute_all().expect("execute outer V1 hash join");
    assert_eq!(rows.len(), 3);
    assert!(rows.contains(&vec![
        JoinValue::Int(0),
        JoinValue::Text("probe-0".into()),
        JoinValue::Int(0),
        JoinValue::Text("build-0".into()),
    ]));
    assert!(rows.contains(&vec![
        JoinValue::Null,
        JoinValue::Null,
        JoinValue::Int(2),
        JoinValue::Text("build-2".into()),
    ]));
}

#[derive(Clone, Copy, Debug)]
/// HashJoin 基准载荷类型（键宽/行宽）。
enum HashJoinPayload {
    WideString,
    Double,
    NoPayload,
}

#[derive(Clone, Debug)]
/// 单组 HashJoin 基准参数。
struct HashJoinBenchmarkCase {
    rows: usize,
    key_indices: Vec<usize>,
    payload: HashJoinPayload,
    disk: bool,
}

/// 按载荷类型生成 HashJoin 行。
fn benchmark_hash_join_rows(rows: usize, payload: HashJoinPayload) -> JoinChunk {
    let wide = vec![b'x'; 5 * 1024];
    (0..rows)
        .map(|row| {
            let mut values = vec![JoinValue::Int(row as i64)];
            match payload {
                HashJoinPayload::WideString => values.push(JoinValue::Bytes(wide.clone())),
                HashJoinPayload::Double => values.push(JoinValue::Float(row as f64)),
                HashJoinPayload::NoPayload => {}
            }
            values
        })
        .collect()
}

/// 执行并校验一组 HashJoin V1 用例。
fn run_hash_join_v1_case(
    case: &HashJoinBenchmarkCase,
    build_side_is_outer: bool,
    inline_projection: bool,
) -> HashJoinV1Exec {
    let join_type = if build_side_is_outer {
        JoinType::RightOuter
    } else {
        JoinType::Inner
    };
    let context = HashJoinCtxV1 {
        base: HashJoinContextBase::default(),
        join_type,
        build_key_indices: case.key_indices.clone(),
        probe_key_indices: case.key_indices.clone(),
        null_aware: false,
        build_side_is_outer,
        concurrency: 4,
        max_chunk_size: 17,
    };
    let column_count = match case.payload {
        HashJoinPayload::NoPayload => 1,
        HashJoinPayload::WideString | HashJoinPayload::Double => 2,
    };
    let children_used = inline_projection.then_some([vec![column_count - 1], Vec::new()]);
    let joiner = Joiner::new(
        join_type,
        true,
        vec![JoinValue::Null; column_count],
        Vec::new(),
        children_used,
        false,
        17,
    )
    .expect("build hash join benchmark joiner");
    let rows = benchmark_hash_join_rows(case.rows, case.payload);
    let mut executor = HashJoinV1Exec::new(context, joiner, vec![rows.clone()], vec![rows])
        .expect("build hash join benchmark executor");
    if case.disk {
        executor.SetMemoryLimit(Some(1));
    }
    executor
}

/// 生成键/载荷/磁盘代表用例列表。
fn hash_join_exec_cases(rows: usize) -> Vec<HashJoinBenchmarkCase> {
    vec![
        HashJoinBenchmarkCase {
            rows,
            key_indices: vec![0, 1],
            payload: HashJoinPayload::WideString,
            disk: false,
        },
        HashJoinBenchmarkCase {
            rows,
            key_indices: vec![0],
            payload: HashJoinPayload::WideString,
            disk: false,
        },
        HashJoinBenchmarkCase {
            rows,
            key_indices: vec![0],
            payload: HashJoinPayload::WideString,
            disk: true,
        },
        HashJoinBenchmarkCase {
            rows: 5,
            key_indices: vec![0],
            payload: HashJoinPayload::Double,
            disk: false,
        },
        HashJoinBenchmarkCase {
            rows,
            key_indices: vec![0, 1],
            payload: HashJoinPayload::Double,
            disk: false,
        },
        HashJoinBenchmarkCase {
            rows,
            key_indices: vec![0],
            payload: HashJoinPayload::Double,
            disk: false,
        },
        HashJoinBenchmarkCase {
            rows,
            key_indices: vec![0],
            payload: HashJoinPayload::NoPayload,
            disk: true,
        },
    ]
}

#[allow(dead_code)]
/// 遍历 Go HashJoin 全矩阵。
fn run_go_hash_join_exec_matrix(build_side_is_outer: bool) {
    for case in hash_join_exec_cases(100_000) {
        if build_side_is_outer && matches!(case.payload, HashJoinPayload::NoPayload) {
            continue;
        }
        let mut executor = run_hash_join_v1_case(&case, build_side_is_outer, false);
        let rows = executor
            .execute_all()
            .expect("execute exact Go hash join matrix case");
        assert_eq!(rows.len(), case.rows, "case: {case:?}");
        assert_eq!(executor.IsSpillTriggered(), case.disk, "case: {case:?}");
    }
}

#[test]
/// HashJoin 内联投影开关覆盖。
fn benchmark_hash_join_inline_projection_covers_on_and_off() {
    let case = HashJoinBenchmarkCase {
        rows: 128,
        key_indices: vec![0],
        payload: HashJoinPayload::WideString,
        disk: false,
    };
    let inline = run_hash_join_v1_case(&case, false, true)
        .execute_all()
        .expect("execute inline hash join benchmark");
    let outer = run_hash_join_v1_case(&case, false, false)
        .execute_all()
        .expect("execute non-inline hash join benchmark");
    assert_eq!(inline.len(), case.rows);
    assert!(inline.iter().all(|row| row.len() == 1));
    assert_eq!(outer.len(), case.rows);
    assert!(outer.iter().all(|row| row.len() == 4));
}

#[test]
/// HashJoin：键/载荷/磁盘代表规模。
fn benchmark_hash_join_exec_runs_key_payload_and_disk_representatives() {
    for case in hash_join_exec_cases(128) {
        let mut executor = run_hash_join_v1_case(&case, false, false);
        let rows = executor
            .execute_all()
            .expect("execute hash join matrix case");
        assert_eq!(rows.len(), case.rows, "case: {case:?}");
        assert_eq!(executor.IsSpillTriggered(), case.disk, "case: {case:?}");
    }
}

#[test]
/// Outer HashJoin：键/载荷/磁盘代表规模。
fn benchmark_outer_hash_join_exec_runs_key_payload_and_disk_representatives() {
    for case in hash_join_exec_cases(128) {
        if matches!(case.payload, HashJoinPayload::NoPayload) {
            continue;
        }
        let mut executor = run_hash_join_v1_case(&case, true, false);
        let rows = executor
            .execute_all()
            .expect("execute outer hash join matrix case");
        assert_eq!(rows.len(), case.rows, "case: {case:?}");
        assert_eq!(executor.IsSpillTriggered(), case.disk, "case: {case:?}");
    }
}

/// 仅构建哈希表的基准用例。
fn run_build_hash_table_case(
    rows: usize,
    key_indices: Vec<usize>,
    disk: bool,
    payload: HashJoinPayload,
) {
    let context = HashJoinCtxV1 {
        base: HashJoinContextBase::default(),
        join_type: JoinType::Inner,
        build_key_indices: key_indices.clone(),
        probe_key_indices: key_indices,
        null_aware: false,
        build_side_is_outer: false,
        concurrency: 4,
        max_chunk_size: 1024,
    };
    let worker = BuildWorkerV1::new(0, context.base.clone(), disk.then_some(1));
    let table = worker
        .build(&context, &[benchmark_hash_join_rows(rows, payload)])
        .expect("build hash table benchmark case");
    assert_eq!(table.len(), rows);
    assert_eq!(table.already_spilled(), disk);
    assert_eq!(context.base.is_spilled(), disk);
}

#[allow(dead_code)]
/// 遍历 Go build hash table 矩阵。
fn run_go_build_hash_table_for_list_matrix() {
    for rows in [10_usize, 100_000] {
        for key_indices in [vec![0, 1], vec![0]] {
            for disk in [false, true] {
                run_build_hash_table_case(
                    rows,
                    key_indices.clone(),
                    disk,
                    HashJoinPayload::WideString,
                );
            }
        }
    }
}

#[test]
/// Build hash table：行数/键/磁盘代表。
fn benchmark_build_hash_table_for_list_runs_rows_keys_and_disk_representatives() {
    for key_indices in [vec![0, 1], vec![0]] {
        for disk in [false, true] {
            run_build_hash_table_case(512, key_indices.clone(), disk, HashJoinPayload::WideString);
        }
    }
}

#[test]
#[ignore = "full Go benchmark scale; run explicitly for the migration evidence gate"]
/// HashJoin 精确 10 万行门禁（ignore）。
fn benchmark_hash_join_exec_runs_exact_go_100000_rows() {
    let case = HashJoinBenchmarkCase {
        rows: 100_000,
        key_indices: vec![0],
        payload: HashJoinPayload::Double,
        disk: false,
    };
    let mut executor = run_hash_join_v1_case(&case, false, false);
    let rows = executor
        .execute_all()
        .expect("execute exact 100000-row hash join");
    assert_eq!(rows.len(), case.rows);
}

#[test]
#[ignore = "full Go benchmark scale; run explicitly for the migration evidence gate"]
/// Build hash table 精确 10 万行门禁（ignore）。
fn benchmark_build_hash_table_for_list_runs_exact_go_100000_rows() {
    run_build_hash_table_case(100_000, vec![0], false, HashJoinPayload::WideString);
}

/// 聚合部分结果 mapper：返回条目数与内存跟踪值。
fn run_agg_partial_result_mapper_case(row_count: usize) -> (usize, u64) {
    let mut mapper = new_agg_partial_result_mapper();
    let partial_results = std::sync::Arc::new(
        (0..10)
            .map(|_| Box::new(()) as PartialResult)
            .collect::<Vec<_>>(),
    );
    for row in 0..row_count {
        mapper.Set(row.to_string(), partial_results.clone());
    }
    (mapper.Len(), mapper.Bytes)
}

/// Go agg partial result mapper 行数矩阵。
const GO_AGG_MAPPER_ROWS: [usize; 8] = [
    0, 100, 10_000, 1_000_000, 851_968, 851_969, 425_984, 425_985,
];

#[allow(dead_code)]
/// 遍历 mapper 行数矩阵。
fn run_go_agg_partial_result_mapper_matrix() {
    for rows in GO_AGG_MAPPER_ROWS {
        let (length, bytes) = run_agg_partial_result_mapper_case(rows);
        assert_eq!(length, rows);
        assert!(bytes > 0);
    }
}

#[test]
/// mapper：真实条目的内存跟踪。
fn benchmark_agg_partial_result_mapper_tracks_memory_for_real_entries() {
    for rows in GO_AGG_MAPPER_ROWS[..3].iter().copied() {
        let (length, bytes) = run_agg_partial_result_mapper_case(rows);
        assert_eq!(length, rows);
        assert!(bytes > 0);
    }
}

#[test]
#[ignore = "full Go benchmark row matrix; run explicitly for the migration evidence gate"]
/// mapper 全行数矩阵门禁（ignore）。
fn benchmark_agg_partial_result_mapper_runs_exact_go_row_matrix() {
    run_go_agg_partial_result_mapper_matrix();
}

/// 验证聚合 Open/Next/Close 生命周期可重复使用。
fn run_aggregate_lifecycle_case(executor_kind: NonDistinctAggExecutor, row_count: usize) {
    let input = non_distinct_aggregate_input(row_count, 1_000, true);
    let expected = expected_non_distinct_sums(&input);
    let aggregation = Aggregation::new(AggKind::Sum, Some(0));
    match executor_kind {
        NonDistinctAggExecutor::Hash => {
            let mut executor = HashAggExec::new(
                HashAggInput {
                    chunks: input,
                    group_columns: vec![1],
                    aggregations: vec![aggregation],
                },
                4,
                4,
                64,
                None,
            );
            for iteration in 0..2 {
                executor.open();
                let mut rows = Vec::new();
                while let Some(chunk) = executor
                    .next()
                    .expect("drain reusable HashAgg benchmark lifecycle")
                {
                    rows.extend(chunk);
                }
                assert_eq!(
                    non_distinct_result_sums(rows),
                    expected,
                    "HashAgg lifecycle iteration {iteration}"
                );
                executor.close();
            }
        }
        NonDistinctAggExecutor::Stream => {
            let mut executor = StreamAggExec::new(input, vec![1], vec![aggregation], 64);
            for iteration in 0..2 {
                executor
                    .open()
                    .expect("open reusable StreamAgg benchmark lifecycle");
                let mut rows = Vec::new();
                while let Some(chunk) = executor
                    .next()
                    .expect("drain reusable StreamAgg benchmark lifecycle")
                {
                    rows.extend(chunk);
                }
                assert_eq!(
                    non_distinct_result_sums(rows),
                    expected,
                    "StreamAgg lifecycle iteration {iteration}"
                );
                executor.close();
            }
        }
    }
}

#[test]
/// 聚合执行器生命周期复用。
fn benchmark_aggregate_executor_reuses_open_next_close_lifecycle() {
    for executor in [NonDistinctAggExecutor::Hash, NonDistinctAggExecutor::Stream] {
        run_aggregate_lifecycle_case(executor, 100_000);
    }
}

#[test]
#[ignore = "full Go default scale; run explicitly for the migration evidence gate"]
/// 聚合生命周期默认规模门禁（ignore）。
fn benchmark_aggregate_executor_reuses_exact_go_default_scale_lifecycle() {
    for executor in [NonDistinctAggExecutor::Hash, NonDistinctAggExecutor::Stream] {
        run_aggregate_lifecycle_case(executor, 10_000_000);
    }
}

/// 运行 ROW_NUMBER 窗口（缓冲或流水线）。
fn run_row_number_window_case(pipelined: bool) -> Vec<Vec<WindowValue>> {
    let input = vec![
        WindowChunk::new(vec![
            vec![WindowValue::Real(10.0), WindowValue::Int(0)],
            vec![WindowValue::Real(11.0), WindowValue::Int(0)],
            vec![WindowValue::Real(20.0), WindowValue::Int(1)],
        ]),
        WindowChunk::new(vec![vec![WindowValue::Real(21.0), WindowValue::Int(1)]]),
    ];
    let plan = PhysicalWindowPlan {
        schema_columns: 3,
        partition_by: vec![1],
        order_by: Vec::new(),
        window_functions: vec![Box::new(RowNumber::default())],
        frame: None,
        pipelined_enabled: pipelined,
    };
    let mut executor = build_window(plan, Box::new(VecChunkExecutor::new(input)), pipelined)
        .expect("build real row-number window benchmark case");
    let context = WindowExecContext::default();
    executor
        .open(&context)
        .expect("open real row-number window benchmark case");
    let mut rows = Vec::new();
    loop {
        let mut output = WindowChunk::default();
        executor
            .next(&context, &mut output)
            .expect("execute real row-number window benchmark case");
        if output.rows.is_empty() {
            break;
        }
        rows.extend(output.rows);
    }
    executor
        .close()
        .expect("close real row-number window benchmark case");
    rows
}

#[test]
/// 窗口：缓冲与流水线 ROW_NUMBER。
fn benchmark_window_rows_runs_buffered_and_pipelined_row_number() {
    let expected = vec![
        vec![
            WindowValue::Real(10.0),
            WindowValue::Int(0),
            WindowValue::UInt(1),
        ],
        vec![
            WindowValue::Real(11.0),
            WindowValue::Int(0),
            WindowValue::UInt(2),
        ],
        vec![
            WindowValue::Real(20.0),
            WindowValue::Int(1),
            WindowValue::UInt(1),
        ],
        vec![
            WindowValue::Real(21.0),
            WindowValue::Int(1),
            WindowValue::UInt(2),
        ],
    ];
    assert_eq!(run_row_number_window_case(false), expected);
    assert_eq!(run_row_number_window_case(true), expected);
}

/// 运行 LAG 窗口用例。
fn run_lag_window_case(pipelined: bool) -> Vec<Vec<WindowValue>> {
    let input = vec![
        WindowChunk::new(vec![
            vec![WindowValue::Real(10.0), WindowValue::Int(0)],
            vec![WindowValue::Real(11.0), WindowValue::Int(0)],
            vec![WindowValue::Real(20.0), WindowValue::Int(1)],
        ]),
        WindowChunk::new(vec![vec![WindowValue::Real(21.0), WindowValue::Int(1)]]),
    ];
    let plan = PhysicalWindowPlan {
        schema_columns: 3,
        partition_by: vec![1],
        order_by: Vec::new(),
        window_functions: vec![Box::new(Lag::new(0, 1, WindowValue::Null))],
        frame: None,
        pipelined_enabled: pipelined,
    };
    let mut executor = build_window(plan, Box::new(VecChunkExecutor::new(input)), pipelined)
        .expect("build real lag window benchmark case");
    let context = WindowExecContext::default();
    executor
        .open(&context)
        .expect("open real lag window benchmark case");
    let mut rows = Vec::new();
    loop {
        let mut output = WindowChunk::default();
        executor
            .next(&context, &mut output)
            .expect("execute real lag window benchmark case");
        if output.rows.is_empty() {
            break;
        }
        rows.extend(output.rows);
    }
    executor
        .close()
        .expect("close real lag window benchmark case");
    rows
}

#[test]
/// 窗口：缓冲与流水线 LAG。
fn benchmark_window_functions_runs_buffered_and_pipelined_lag() {
    let expected = vec![
        vec![
            WindowValue::Real(10.0),
            WindowValue::Int(0),
            WindowValue::Null,
        ],
        vec![
            WindowValue::Real(11.0),
            WindowValue::Int(0),
            WindowValue::Real(10.0),
        ],
        vec![
            WindowValue::Real(20.0),
            WindowValue::Int(1),
            WindowValue::Null,
        ],
        vec![
            WindowValue::Real(21.0),
            WindowValue::Int(1),
            WindowValue::Real(20.0),
        ],
    ];
    assert_eq!(run_lag_window_case(false), expected);
    assert_eq!(run_lag_window_case(true), expected);
}

/// 运行带 frame 的 BIT_XOR 窗口。
fn run_bit_xor_window_frame_case(pipelined: bool) -> Vec<Vec<WindowValue>> {
    let input = vec![WindowChunk::new(
        (1..=4)
            .map(|value| vec![WindowValue::Int(value), WindowValue::Int(0)])
            .collect(),
    )];
    let plan = PhysicalWindowPlan {
        schema_columns: 3,
        partition_by: vec![1],
        order_by: Vec::new(),
        window_functions: vec![Box::new(BitXor::new(0))],
        frame: Some(WindowFrame {
            frame_type: WindowFrameType::Rows,
            start: WindowFrameBound::preceding(1),
            end: WindowFrameBound::following(1),
        }),
        pipelined_enabled: pipelined,
    };
    let mut executor = build_window(plan, Box::new(VecChunkExecutor::new(input)), pipelined)
        .expect("build real BIT_XOR ROWS-frame window benchmark case");
    let context = WindowExecContext::default();
    executor.open(&context).expect("open BIT_XOR window case");
    let mut rows = Vec::new();
    loop {
        let mut output = WindowChunk::default();
        executor
            .next(&context, &mut output)
            .expect("execute BIT_XOR ROWS-frame window benchmark case");
        if output.rows.is_empty() {
            break;
        }
        rows.extend(output.rows);
    }
    executor.close().expect("close BIT_XOR window case");
    rows
}

#[test]
/// 窗口 frame：BIT_XOR 边界。
fn benchmark_window_frame_validates_buffered_and_pipelined_bit_xor_boundaries() {
    let expected_xor = [3_u64, 0, 5, 7];
    for pipelined in [false, true] {
        let rows = run_bit_xor_window_frame_case(pipelined);
        assert_eq!(rows.len(), expected_xor.len());
        for (row, expected) in rows.iter().zip(expected_xor) {
            assert_eq!(row.last(), Some(&WindowValue::UInt(expected)));
        }
    }
}

/// 运行带 frame 的 AVG 窗口。
fn run_average_window_frame_case(pipelined: bool) -> Vec<Vec<WindowValue>> {
    let input = vec![WindowChunk::new(
        (1..=4)
            .map(|value| vec![WindowValue::Int(value), WindowValue::Int(0)])
            .collect(),
    )];
    let plan = PhysicalWindowPlan {
        schema_columns: 3,
        partition_by: vec![1],
        order_by: Vec::new(),
        window_functions: vec![Box::new(Average::new(0))],
        frame: Some(WindowFrame {
            frame_type: WindowFrameType::Rows,
            start: WindowFrameBound::preceding(1),
            end: WindowFrameBound::following(1),
        }),
        pipelined_enabled: pipelined,
    };
    let mut executor = build_window(plan, Box::new(VecChunkExecutor::new(input)), pipelined)
        .expect("build real AVG ROWS-frame window benchmark case");
    let context = WindowExecContext::default();
    executor.open(&context).expect("open AVG window case");
    let mut rows = Vec::new();
    loop {
        let mut output = WindowChunk::default();
        executor
            .next(&context, &mut output)
            .expect("execute AVG ROWS-frame window benchmark case");
        if output.rows.is_empty() {
            break;
        }
        rows.extend(output.rows);
    }
    executor.close().expect("close AVG window case");
    rows
}

#[test]
/// 窗口 frame：AVG 边界。
fn benchmark_window_frame_validates_buffered_and_pipelined_average_boundaries() {
    let expected_average = [1.5_f64, 2.0, 3.0, 3.5];
    for pipelined in [false, true] {
        let rows = run_average_window_frame_case(pipelined);
        assert_eq!(rows.len(), expected_average.len());
        for (row, expected) in rows.iter().zip(expected_average) {
            assert_eq!(row.last(), Some(&WindowValue::Real(expected)));
        }
    }
}

/// 运行 Decimal AVG 窗口 frame。
fn run_decimal_average_window_frame_case(pipelined: bool) -> Vec<Vec<WindowValue>> {
    let input = vec![WindowChunk::new(
        [100_i128, 200, 300, 400]
            .into_iter()
            .map(|coefficient| {
                vec![
                    WindowValue::Decimal(Decimal::new(coefficient, 2)),
                    WindowValue::Int(0),
                ]
            })
            .collect(),
    )];
    let plan = PhysicalWindowPlan {
        schema_columns: 3,
        partition_by: vec![1],
        order_by: Vec::new(),
        window_functions: vec![Box::new(DecimalAverage::new(0))],
        frame: Some(WindowFrame {
            frame_type: WindowFrameType::Rows,
            start: WindowFrameBound::preceding(1),
            end: WindowFrameBound::following(1),
        }),
        pipelined_enabled: pipelined,
    };
    let mut executor = build_window(plan, Box::new(VecChunkExecutor::new(input)), pipelined)
        .expect("build real DECIMAL AVG ROWS-frame window benchmark case");
    let context = WindowExecContext::default();
    executor
        .open(&context)
        .expect("open DECIMAL AVG window case");
    let mut rows = Vec::new();
    loop {
        let mut output = WindowChunk::default();
        executor
            .next(&context, &mut output)
            .expect("execute DECIMAL AVG ROWS-frame window benchmark case");
        if output.rows.is_empty() {
            break;
        }
        rows.extend(output.rows);
    }
    executor.close().expect("close DECIMAL AVG window case");
    rows
}

#[test]
/// 窗口 frame：Decimal AVG 边界。
fn benchmark_window_frame_validates_decimal_average_boundaries() {
    let expected = [150_i128, 200, 300, 350];
    for pipelined in [false, true] {
        let rows = run_decimal_average_window_frame_case(pipelined);
        assert_eq!(rows.len(), expected.len());
        for (row, coefficient) in rows.iter().zip(expected) {
            assert_eq!(
                row.last(),
                Some(&WindowValue::Decimal(Decimal::new(coefficient, 2)))
            );
        }
    }
}

/// 运行 Decimal SUM 窗口 frame。
fn run_decimal_sum_window_frame_case(pipelined: bool) -> Vec<Vec<WindowValue>> {
    let input = vec![WindowChunk::new(
        [100_i128, 200, 300, 400]
            .into_iter()
            .map(|coefficient| {
                vec![
                    WindowValue::Decimal(Decimal::new(coefficient, 2)),
                    WindowValue::Int(0),
                ]
            })
            .collect(),
    )];
    let plan = PhysicalWindowPlan {
        schema_columns: 3,
        partition_by: vec![1],
        order_by: Vec::new(),
        window_functions: vec![Box::new(DecimalSum::new(0))],
        frame: Some(WindowFrame {
            frame_type: WindowFrameType::Rows,
            start: WindowFrameBound::preceding(1),
            end: WindowFrameBound::following(1),
        }),
        pipelined_enabled: pipelined,
    };
    let mut executor = build_window(plan, Box::new(VecChunkExecutor::new(input)), pipelined)
        .expect("build real DECIMAL SUM ROWS-frame window benchmark case");
    let context = WindowExecContext::default();
    executor
        .open(&context)
        .expect("open DECIMAL SUM window case");
    let mut rows = Vec::new();
    loop {
        let mut output = WindowChunk::default();
        executor
            .next(&context, &mut output)
            .expect("execute DECIMAL SUM ROWS-frame window benchmark case");
        if output.rows.is_empty() {
            break;
        }
        rows.extend(output.rows);
    }
    executor.close().expect("close DECIMAL SUM window case");
    rows
}

#[test]
/// 窗口 frame：Decimal SUM 边界。
fn benchmark_window_frame_validates_decimal_sum_boundaries() {
    let expected = [300_i128, 600, 900, 700];
    for pipelined in [false, true] {
        let rows = run_decimal_sum_window_frame_case(pipelined);
        assert_eq!(rows.len(), expected.len());
        for (row, coefficient) in rows.iter().zip(expected) {
            assert_eq!(
                row.last(),
                Some(&WindowValue::Decimal(Decimal::new(coefficient, 2)))
            );
        }
    }
}

/// 运行 MAX 窗口 frame。
fn run_max_window_frame_case(pipelined: bool) -> Vec<Vec<WindowValue>> {
    let input = vec![WindowChunk::new(
        [4_i64, 1, 3, 2]
            .into_iter()
            .map(|value| vec![WindowValue::Int(value), WindowValue::Int(0)])
            .collect(),
    )];
    let plan = PhysicalWindowPlan {
        schema_columns: 3,
        partition_by: vec![1],
        order_by: Vec::new(),
        window_functions: vec![Box::new(MaxValue::new(0))],
        frame: Some(WindowFrame {
            frame_type: WindowFrameType::Rows,
            start: WindowFrameBound::preceding(1),
            end: WindowFrameBound::following(1),
        }),
        pipelined_enabled: pipelined,
    };
    let mut executor = build_window(plan, Box::new(VecChunkExecutor::new(input)), pipelined)
        .expect("build real MAX ROWS-frame window benchmark case");
    let context = WindowExecContext::default();
    executor.open(&context).expect("open MAX window case");
    let mut rows = Vec::new();
    loop {
        let mut output = WindowChunk::default();
        executor
            .next(&context, &mut output)
            .expect("execute MAX ROWS-frame window benchmark case");
        if output.rows.is_empty() {
            break;
        }
        rows.extend(output.rows);
    }
    executor.close().expect("close MAX window case");
    rows
}

#[test]
/// 窗口 frame：MAX 边界。
fn benchmark_window_frame_validates_buffered_and_pipelined_max_boundaries() {
    let expected = [4_i64, 4, 3, 3];
    for pipelined in [false, true] {
        let rows = run_max_window_frame_case(pipelined);
        assert_eq!(rows.len(), expected.len());
        for (row, value) in rows.iter().zip(expected) {
            assert_eq!(row.last(), Some(&WindowValue::Int(value)));
        }
    }
}

/// 运行 MIN 窗口 frame。
fn run_min_window_frame_case(pipelined: bool) -> Vec<Vec<WindowValue>> {
    let input = vec![WindowChunk::new(
        [4_i64, 1, 3, 2]
            .into_iter()
            .map(|value| vec![WindowValue::Int(value), WindowValue::Int(0)])
            .collect(),
    )];
    let plan = PhysicalWindowPlan {
        schema_columns: 3,
        partition_by: vec![1],
        order_by: Vec::new(),
        window_functions: vec![Box::new(MinValue::new(0))],
        frame: Some(WindowFrame {
            frame_type: WindowFrameType::Rows,
            start: WindowFrameBound::preceding(1),
            end: WindowFrameBound::following(1),
        }),
        pipelined_enabled: pipelined,
    };
    let mut executor = build_window(plan, Box::new(VecChunkExecutor::new(input)), pipelined)
        .expect("build real MIN ROWS-frame window benchmark case");
    let context = WindowExecContext::default();
    executor.open(&context).expect("open MIN window case");
    let mut rows = Vec::new();
    loop {
        let mut output = WindowChunk::default();
        executor
            .next(&context, &mut output)
            .expect("execute MIN ROWS-frame window benchmark case");
        if output.rows.is_empty() {
            break;
        }
        rows.extend(output.rows);
    }
    executor.close().expect("close MIN window case");
    rows
}

#[test]
/// 窗口 frame：MIN 边界。
fn benchmark_window_frame_validates_buffered_and_pipelined_min_boundaries() {
    let expected = [1_i64, 1, 1, 2];
    for pipelined in [false, true] {
        let rows = run_min_window_frame_case(pipelined);
        assert_eq!(rows.len(), expected.len());
        for (row, value) in rows.iter().zip(expected) {
            assert_eq!(row.last(), Some(&WindowValue::Int(value)));
        }
    }
}

#[derive(Clone, Copy)]
/// 窗口基准函数枚举。
enum WindowBenchmarkFunction {
    RowNumber,
    Lag,
    BitXor,
    Sum,
    DecimalSum,
    Count,
    Average,
    DecimalAverage,
    Max,
    Min,
}

#[derive(Clone, Copy)]
/// 窗口基准输入形态。
enum WindowBenchmarkInput {
    Real,
    Integer,
    Decimal,
}

/// WindowValue → SortValue 转换。
fn window_value_to_sort_value(value: WindowValue) -> SortValue {
    match value {
        WindowValue::Null => SortValue::Null,
        WindowValue::Int(value) => SortValue::Int(value),
        WindowValue::UInt(value) => SortValue::UInt(value),
        WindowValue::Real(value) => SortValue::Float(value),
        WindowValue::Text(value) => SortValue::Bytes(value.into_bytes()),
        WindowValue::Bool(value) => SortValue::UInt(u64::from(value)),
        WindowValue::Decimal(_) => {
            panic!("the Go window shuffle source does not use DECIMAL columns")
        }
    }
}

/// SortValue → WindowValue 转换。
fn sort_value_to_window_value(value: SortValue) -> WindowValue {
    match value {
        SortValue::Null => WindowValue::Null,
        SortValue::Int(value) => WindowValue::Int(value),
        SortValue::UInt(value) => WindowValue::UInt(value),
        SortValue::Float(value) => WindowValue::Real(value),
        SortValue::Bytes(value) => WindowValue::Text(
            String::from_utf8(value).expect("window benchmark bytes are valid UTF-8"),
        ),
    }
}

/// 对窗口源数据按分区/排序键排序。
fn sort_window_source(
    rows: Vec<Vec<WindowValue>>,
    concurrency: usize,
    order_value: bool,
) -> Vec<Vec<WindowValue>> {
    let chunks = rows
        .chunks(1024)
        .map(|chunk| {
            DataChunk::new(
                chunk
                    .iter()
                    .cloned()
                    .map(|row| Row(row.into_iter().map(window_value_to_sort_value).collect()))
                    .collect(),
            )
        })
        .collect();
    let child = Box::new(VecRowSource::new(chunks));
    let mut keys = vec![SortKey::asc(1)];
    if order_value {
        keys.push(SortKey::asc(0));
    }
    let mut sort = SortExec::new(child, keys, concurrency, 1024, -1);
    let mut result = Vec::new();
    loop {
        let chunk = sort
            .Next(1024)
            .expect("execute real partition-key SortExec for window benchmark");
        if chunk.rows.is_empty() {
            break;
        }
        result.extend(
            chunk
                .rows
                .into_iter()
                .map(|row| row.0.into_iter().map(sort_value_to_window_value).collect()),
        );
    }
    sort.Close()
        .expect("close real partition-key SortExec for window benchmark");
    result
}

/// 返回待测窗口函数列表。
fn window_benchmark_functions(
    function: WindowBenchmarkFunction,
    count: usize,
) -> Vec<Box<dyn astersql_executor_windows::window::WindowFunction>> {
    (0..count)
        .map(|_| match function {
            WindowBenchmarkFunction::RowNumber => Box::new(RowNumber::default())
                as Box<dyn astersql_executor_windows::window::WindowFunction>,
            WindowBenchmarkFunction::Lag => Box::new(Lag::new(1, 1, WindowValue::Null)),
            WindowBenchmarkFunction::BitXor => Box::new(BitXor::new(0)),
            WindowBenchmarkFunction::Sum => Box::new(Sum::new(0)),
            WindowBenchmarkFunction::DecimalSum => Box::new(DecimalSum::new(0)),
            WindowBenchmarkFunction::Count => Box::new(CountRows::default()),
            WindowBenchmarkFunction::Average => Box::new(Average::new(0)),
            WindowBenchmarkFunction::DecimalAverage => Box::new(DecimalAverage::new(0)),
            WindowBenchmarkFunction::Max => Box::new(MaxValue::new(0)),
            WindowBenchmarkFunction::Min => Box::new(MinValue::new(0)),
        })
        .collect()
}

/// 单分区通道上运行窗口函数。
fn run_window_partition_lane(
    rows: Vec<Vec<WindowValue>>,
    function: WindowBenchmarkFunction,
    function_count: usize,
    pipelined: bool,
    frame: Option<WindowFrame>,
    order_by: Vec<WindowOrderBy>,
) -> Vec<Vec<WindowValue>> {
    let chunks = rows
        .chunks(1024)
        .map(|rows| WindowChunk::new(rows.to_vec()))
        .collect();
    let plan = PhysicalWindowPlan {
        schema_columns: 4 + function_count,
        partition_by: vec![1],
        order_by,
        window_functions: window_benchmark_functions(function, function_count),
        frame,
        pipelined_enabled: pipelined,
    };
    let mut executor = build_window(plan, Box::new(VecChunkExecutor::new(chunks)), pipelined)
        .expect("build partition lane for real window benchmark");
    let context = WindowExecContext::default();
    executor
        .open(&context)
        .expect("open partition lane for real window benchmark");
    let mut result = Vec::new();
    loop {
        let mut output = WindowChunk::default();
        executor
            .next(&context, &mut output)
            .expect("execute partition lane for real window benchmark");
        if output.rows.is_empty() {
            break;
        }
        result.extend(output.rows);
    }
    executor
        .close()
        .expect("close partition lane for real window benchmark");
    result
}

/// Shuffle 分区后运行窗口。
fn run_window_shuffle_case(
    row_count: usize,
    ndv: usize,
    concurrency: usize,
    data_source_sorted: bool,
    pipelined: bool,
    function: WindowBenchmarkFunction,
) -> Vec<Vec<WindowValue>> {
    run_window_matrix_case(
        row_count,
        ndv,
        concurrency,
        data_source_sorted,
        pipelined,
        function,
        1,
        WindowBenchmarkInput::Real,
        None,
        Vec::new(),
    )
}

#[allow(clippy::too_many_arguments)]
/// 窗口矩阵单点：排序 + 分区 shuffle。
fn run_window_matrix_case(
    row_count: usize,
    ndv: usize,
    concurrency: usize,
    data_source_sorted: bool,
    pipelined: bool,
    function: WindowBenchmarkFunction,
    function_count: usize,
    input: WindowBenchmarkInput,
    frame: Option<WindowFrame>,
    order_by: Vec<WindowOrderBy>,
) -> Vec<Vec<WindowValue>> {
    assert!(ndv > 0);
    assert!(concurrency > 0);
    let mut rows = (0..row_count)
        .map(|row| {
            let source_position = if data_source_sorted {
                row
            } else {
                row_count - row - 1
            };
            let partition = if data_source_sorted {
                source_position.saturating_mul(ndv) / row_count.max(1)
            } else {
                source_position % ndv
            };
            let value = match input {
                WindowBenchmarkInput::Real => WindowValue::Real(source_position as f64),
                WindowBenchmarkInput::Integer => WindowValue::Int(source_position as i64),
                WindowBenchmarkInput::Decimal => {
                    WindowValue::Decimal(Decimal::new(source_position as i128 * 100, 2))
                }
            };
            vec![
                value,
                WindowValue::Int(partition.min(ndv - 1) as i64),
                WindowValue::Int(source_position as i64),
                WindowValue::Int(-(source_position as i64)),
            ]
        })
        .collect::<Vec<_>>();
    if !data_source_sorted {
        rows = sort_window_source(rows, concurrency, !order_by.is_empty());
    }

    let mut lanes = vec![Vec::new(); concurrency];
    for row in rows {
        let partition = match row.get(1) {
            Some(WindowValue::Int(value)) => *value as usize,
            value => panic!("unexpected window partition value {value:?}"),
        };
        lanes[partition % concurrency].push(row);
    }

    std::thread::scope(|scope| {
        let handles = lanes
            .into_iter()
            .map(|rows| {
                let frame = frame.clone();
                let order_by = order_by.clone();
                scope.spawn(move || {
                    run_window_partition_lane(
                        rows,
                        function,
                        function_count,
                        pipelined,
                        frame,
                        order_by,
                    )
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("window partition lane panicked"))
            .collect()
    })
}

/// 校验 shuffle 后 ROW_NUMBER 结果。
fn validate_row_number_shuffle_result(rows: &[Vec<WindowValue>], expected_rows: usize) {
    assert_eq!(rows.len(), expected_rows);
    let mut counts = std::collections::HashMap::<i64, u64>::new();
    for row in rows {
        let partition = match row.get(1) {
            Some(WindowValue::Int(value)) => *value,
            value => panic!("unexpected window partition value {value:?}"),
        };
        let expected = counts.entry(partition).or_default();
        *expected += 1;
        assert_eq!(row.last(), Some(&WindowValue::UInt(*expected)));
    }
}

/// 校验 shuffle 后 LAG 结果。
fn validate_lag_shuffle_result(rows: &[Vec<WindowValue>], expected_rows: usize) {
    assert_eq!(rows.len(), expected_rows);
    let mut previous = std::collections::HashMap::<i64, WindowValue>::new();
    for row in rows {
        let partition = match row.get(1) {
            Some(WindowValue::Int(value)) => *value,
            value => panic!("unexpected window partition value {value:?}"),
        };
        let expected = previous
            .insert(partition, row[1].clone())
            .unwrap_or(WindowValue::Null);
        assert_eq!(row.last(), Some(&expected));
    }
}

#[test]
/// 窗口 ROW_NUMBER：真实排序 + 分区 shuffle 代表规模。
fn benchmark_window_rows_uses_real_sort_and_partition_shuffle_representative() {
    for pipelined in [false, true] {
        let rows = run_window_shuffle_case(
            1_000,
            10,
            4,
            false,
            pipelined,
            WindowBenchmarkFunction::RowNumber,
        );
        validate_row_number_shuffle_result(&rows, 1_000);
    }
}

#[test]
#[ignore = "maximum Go WindowRows scale; run explicitly for the migration evidence gate"]
/// 窗口 ROW_NUMBER 最大规模门禁（ignore）。
fn benchmark_window_rows_maximum_scale_gate() {
    let rows = run_window_shuffle_case(
        100_000,
        1_000,
        4,
        false,
        true,
        WindowBenchmarkFunction::RowNumber,
    );
    validate_row_number_shuffle_result(&rows, 100_000);
}

#[test]
#[ignore = "full Go WindowRows parameter matrix; run explicitly for migration evidence"]
/// 窗口 ROW_NUMBER 精确 Go 矩阵（ignore）。
fn benchmark_window_rows_runs_exact_go_matrix() {
    for pipelined in [false, true] {
        for rows in [1_000, 100_000] {
            for ndv in [1, 10, 1_000] {
                for concurrency in [1, 2, 4] {
                    let result = run_window_shuffle_case(
                        rows,
                        ndv,
                        concurrency,
                        false,
                        pipelined,
                        WindowBenchmarkFunction::RowNumber,
                    );
                    validate_row_number_shuffle_result(&result, rows);
                }
            }
        }
    }
}

#[test]
/// 窗口 LAG：真实排序 + 分区 shuffle 代表规模。
fn benchmark_window_lag_uses_real_sort_and_partition_shuffle_representative() {
    for pipelined in [false, true] {
        let rows = run_window_shuffle_case(
            10_000,
            100,
            4,
            false,
            pipelined,
            WindowBenchmarkFunction::Lag,
        );
        validate_lag_shuffle_result(&rows, 10_000);
    }
}

#[test]
#[ignore = "full Go WindowFunctions LAG parameter matrix; run explicitly for migration evidence"]
/// 窗口 LAG 精确 Go 矩阵（ignore）。
fn benchmark_window_lag_runs_exact_go_matrix() {
    for pipelined in [false, true] {
        for concurrency in [1, 4] {
            let rows = run_window_shuffle_case(
                100_000,
                1_000,
                concurrency,
                false,
                pipelined,
                WindowBenchmarkFunction::Lag,
            );
            validate_lag_shuffle_result(&rows, 100_000);
        }
    }
}

/// 校验重复键下 ROW_NUMBER 连续编号。
fn validate_repeated_row_number_results(
    rows: &[Vec<WindowValue>],
    expected_rows: usize,
    function_count: usize,
) {
    assert_eq!(rows.len(), expected_rows);
    let mut counts = std::collections::HashMap::<i64, u64>::new();
    for row in rows {
        let partition = match row.get(1) {
            Some(WindowValue::Int(value)) => *value,
            value => panic!("unexpected window partition value {value:?}"),
        };
        let expected = counts.entry(partition).or_default();
        *expected += 1;
        assert!(
            row[4..]
                .iter()
                .take(function_count)
                .all(|value| value == &WindowValue::UInt(*expected))
        );
    }
}

/// 校验分区内 BIT_XOR 累计。
fn validate_repeated_partition_xor_results(
    rows: &[Vec<WindowValue>],
    expected_rows: usize,
    function_count: usize,
) {
    assert_eq!(rows.len(), expected_rows);
    let mut partition_xor = std::collections::HashMap::<i64, u64>::new();
    for row in rows {
        let partition = match row.get(1) {
            Some(WindowValue::Int(value)) => *value,
            value => panic!("unexpected window partition value {value:?}"),
        };
        let value = match row.first() {
            Some(WindowValue::Real(value)) => *value as u64,
            value => panic!("unexpected BIT_XOR source value {value:?}"),
        };
        *partition_xor.entry(partition).or_default() ^= value;
    }
    for row in rows {
        let partition = match row.get(1) {
            Some(WindowValue::Int(value)) => *value,
            value => panic!("unexpected window partition value {value:?}"),
        };
        let expected = WindowValue::UInt(partition_xor[&partition]);
        assert!(
            row[4..]
                .iter()
                .take(function_count)
                .all(|value| value == &expected)
        );
    }
}

/// 带 frame 的窗口函数 Go 矩阵。
fn run_window_functions_with_frame_exact_go_matrix() {
    for pipelined in [false, true] {
        for (function, frame) in [
            (
                WindowBenchmarkFunction::RowNumber,
                Some(WindowFrame {
                    frame_type: WindowFrameType::Rows,
                    start: WindowFrameBound::unbounded(WindowBoundType::Preceding),
                    end: WindowFrameBound::default(),
                }),
            ),
            (WindowBenchmarkFunction::BitXor, None),
        ] {
            for data_source_sorted in [false, true] {
                for function_count in [1, 5] {
                    for concurrency in [1, 2, 3, 4, 5, 6] {
                        let rows = run_window_matrix_case(
                            100_000,
                            1_000,
                            concurrency,
                            data_source_sorted,
                            pipelined,
                            function,
                            function_count,
                            WindowBenchmarkInput::Real,
                            frame.clone(),
                            Vec::new(),
                        );
                        match function {
                            WindowBenchmarkFunction::RowNumber => {
                                validate_repeated_row_number_results(
                                    &rows,
                                    100_000,
                                    function_count,
                                );
                            }
                            WindowBenchmarkFunction::BitXor => {
                                validate_repeated_partition_xor_results(
                                    &rows,
                                    100_000,
                                    function_count,
                                );
                            }
                            _ => unreachable!(),
                        }
                    }
                }
            }
        }
    }
}

#[test]
/// 带 frame 窗口函数代表规模。
fn benchmark_window_functions_with_frame_representative() {
    for function in [
        WindowBenchmarkFunction::RowNumber,
        WindowBenchmarkFunction::BitXor,
    ] {
        let frame = matches!(function, WindowBenchmarkFunction::RowNumber).then(|| WindowFrame {
            frame_type: WindowFrameType::Rows,
            start: WindowFrameBound::unbounded(WindowBoundType::Preceding),
            end: WindowFrameBound::default(),
        });
        let rows = run_window_matrix_case(
            1_000,
            10,
            4,
            false,
            true,
            function,
            5,
            WindowBenchmarkInput::Real,
            frame,
            Vec::new(),
        );
        match function {
            WindowBenchmarkFunction::RowNumber => {
                validate_repeated_row_number_results(&rows, 1_000, 5)
            }
            WindowBenchmarkFunction::BitXor => {
                validate_repeated_partition_xor_results(&rows, 1_000, 5)
            }
            _ => unreachable!(),
        }
    }
}

#[test]
/// 缓冲 ROW_NUMBER：跨结果 chunk 保持状态。
fn benchmark_buffered_row_number_frame_preserves_state_across_result_chunks() {
    let rows = run_window_matrix_case(
        2_048,
        1,
        1,
        true,
        false,
        WindowBenchmarkFunction::RowNumber,
        1,
        WindowBenchmarkInput::Real,
        Some(WindowFrame {
            frame_type: WindowFrameType::Rows,
            start: WindowFrameBound::unbounded(WindowBoundType::Preceding),
            end: WindowFrameBound::default(),
        }),
        Vec::new(),
    );
    validate_repeated_row_number_results(&rows, 2_048, 1);
}

#[test]
#[ignore = "full Go WindowFunctionsWithFrame matrix; run explicitly for migration evidence"]
/// 带 frame 窗口精确 Go 矩阵（ignore）。
fn benchmark_window_functions_with_frame_runs_exact_go_matrix() {
    run_window_functions_with_frame_exact_go_matrix();
}

/// 校验 UNBOUNDED 分区 MAX 在分区边界重置。
fn validate_unbounded_partition_max(rows: &[Vec<WindowValue>], expected_rows: usize) {
    assert_eq!(rows.len(), expected_rows);
    let mut maxima = std::collections::HashMap::<i64, f64>::new();
    for row in rows {
        let partition = match row.get(1) {
            Some(WindowValue::Int(value)) => *value,
            value => panic!("unexpected window partition value {value:?}"),
        };
        let value = match row.first() {
            Some(WindowValue::Real(value)) => *value,
            value => panic!("unexpected MAX source value {value:?}"),
        };
        maxima
            .entry(partition)
            .and_modify(|maximum| *maximum = maximum.max(value))
            .or_insert(value);
    }
    for row in rows {
        let partition = match row.get(1) {
            Some(WindowValue::Int(value)) => *value,
            value => panic!("unexpected window partition value {value:?}"),
        };
        assert_eq!(row.last(), Some(&WindowValue::Real(maxima[&partition])));
    }
}

#[test]
/// UNBOUNDED MAX：缓冲/流水线均在分区边界重置。
fn benchmark_unbounded_max_validates_partition_reset_for_both_executors() {
    let frame = WindowFrame {
        frame_type: WindowFrameType::Rows,
        start: WindowFrameBound::unbounded(WindowBoundType::Preceding),
        end: WindowFrameBound::unbounded(WindowBoundType::Following),
    };
    for pipelined in [false, true] {
        let rows = run_window_matrix_case(
            10_000,
            10,
            1,
            false,
            pipelined,
            WindowBenchmarkFunction::Max,
            1,
            WindowBenchmarkInput::Real,
            Some(frame.clone()),
            Vec::new(),
        );
        validate_unbounded_partition_max(&rows, 10_000);
    }
}

/// Go 滑动窗口函数与输入组合。
const GO_SLIDING_WINDOW_FUNCTIONS: [(WindowBenchmarkFunction, WindowBenchmarkInput); 10] = [
    (WindowBenchmarkFunction::Sum, WindowBenchmarkInput::Real),
    (
        WindowBenchmarkFunction::DecimalSum,
        WindowBenchmarkInput::Decimal,
    ),
    (
        WindowBenchmarkFunction::Count,
        WindowBenchmarkInput::Integer,
    ),
    (WindowBenchmarkFunction::Average, WindowBenchmarkInput::Real),
    (
        WindowBenchmarkFunction::DecimalAverage,
        WindowBenchmarkInput::Decimal,
    ),
    (
        WindowBenchmarkFunction::BitXor,
        WindowBenchmarkInput::Integer,
    ),
    (WindowBenchmarkFunction::Max, WindowBenchmarkInput::Integer),
    (WindowBenchmarkFunction::Max, WindowBenchmarkInput::Real),
    (WindowBenchmarkFunction::Min, WindowBenchmarkInput::Integer),
    (WindowBenchmarkFunction::Min, WindowBenchmarkInput::Real),
];

/// 从窗口值提取整型用于滑动窗口校验。
fn sliding_source_integer(value: &WindowValue) -> i128 {
    match value {
        WindowValue::Int(value) => *value as i128,
        WindowValue::Real(value) => *value as i128,
        WindowValue::Decimal(value) => value.coefficient() / 100,
        value => panic!("unexpected sliding-window source value {value:?}"),
    }
}

/// 校验 ROWS/RANGE 滑动窗口结果。
fn validate_sliding_window_results(
    rows: &[Vec<WindowValue>],
    expected_rows: usize,
    function: WindowBenchmarkFunction,
    input: WindowBenchmarkInput,
) {
    assert_eq!(rows.len(), expected_rows);
    let mut group_start = 0;
    while group_start < rows.len() {
        let partition = rows[group_start][1].clone();
        let mut group_end = group_start + 1;
        while group_end < rows.len() && rows[group_end][1] == partition {
            group_end += 1;
        }
        for current in group_start..group_end {
            let start = current.saturating_sub(10).max(group_start);
            let end = current.saturating_add(11).min(group_end);
            let count = end - start;
            let integer_sum = rows[start..end]
                .iter()
                .map(|row| sliding_source_integer(&row[0]))
                .sum::<i128>();
            let expected = match function {
                WindowBenchmarkFunction::Sum => WindowValue::Real(integer_sum as f64),
                WindowBenchmarkFunction::DecimalSum => {
                    WindowValue::Decimal(Decimal::new(integer_sum * 100, 2))
                }
                WindowBenchmarkFunction::Count => WindowValue::UInt(count as u64),
                WindowBenchmarkFunction::Average => {
                    WindowValue::Real(integer_sum as f64 / count as f64)
                }
                WindowBenchmarkFunction::DecimalAverage => {
                    WindowValue::Decimal(Decimal::new(integer_sum * 100 / count as i128, 2))
                }
                WindowBenchmarkFunction::BitXor => WindowValue::UInt(
                    rows[start..end]
                        .iter()
                        .map(|row| sliding_source_integer(&row[0]) as u64)
                        .fold(0, |result, value| result ^ value),
                ),
                WindowBenchmarkFunction::Max => match input {
                    WindowBenchmarkInput::Integer => {
                        WindowValue::Int(sliding_source_integer(&rows[end - 1][0]) as i64)
                    }
                    WindowBenchmarkInput::Real => {
                        WindowValue::Real(sliding_source_integer(&rows[end - 1][0]) as f64)
                    }
                    WindowBenchmarkInput::Decimal => unreachable!(),
                },
                WindowBenchmarkFunction::Min => match input {
                    WindowBenchmarkInput::Integer => {
                        WindowValue::Int(sliding_source_integer(&rows[start][0]) as i64)
                    }
                    WindowBenchmarkInput::Real => {
                        WindowValue::Real(sliding_source_integer(&rows[start][0]) as f64)
                    }
                    WindowBenchmarkInput::Decimal => unreachable!(),
                },
                _ => unreachable!(),
            };
            assert_eq!(
                rows[current].last(),
                Some(&expected),
                "sliding result mismatch at row {current}"
            );
        }
        group_start = group_end;
    }
}

/// 运行滑动窗口 Go 矩阵点。
fn run_sliding_window_go_matrix(row_count: usize, ndv: usize) {
    for pipelined in [false, true] {
        for frame_type in [WindowFrameType::Rows, WindowFrameType::Range] {
            for (function, input) in GO_SLIDING_WINDOW_FUNCTIONS {
                let order_by = (frame_type == WindowFrameType::Range)
                    .then(|| {
                        vec![WindowOrderBy {
                            column: 0,
                            descending: false,
                        }]
                    })
                    .unwrap_or_default();
                let rows = run_window_matrix_case(
                    row_count,
                    ndv,
                    1,
                    true,
                    pipelined,
                    function,
                    1,
                    input,
                    Some(WindowFrame {
                        frame_type,
                        start: WindowFrameBound::preceding(10),
                        end: WindowFrameBound::following(10),
                    }),
                    order_by,
                );
                validate_sliding_window_results(&rows, row_count, function, input);
            }
        }
    }
}

#[test]
/// 滑动窗口 ROWS/RANGE 代表规模。
fn benchmark_sliding_window_rows_and_range_representative() {
    run_sliding_window_go_matrix(1_000, 10);
}

#[test]
#[ignore = "full Go sliding-window 40-case matrix; run explicitly for migration evidence"]
/// 滑动窗口精确 Go 矩阵（ignore）。
fn benchmark_sliding_window_rows_and_range_runs_exact_go_matrix() {
    run_sliding_window_go_matrix(100_000, 100);
}

#[derive(Clone)]
/// IndexJoin 基准用的索引构建辅助。
struct BenchmarkIndexBuilder {
    rows_by_key: std::sync::Arc<std::collections::HashMap<Vec<u8>, Vec<Vec<JoinValue>>>>,
}

impl BenchmarkIndexBuilder {
    fn new(rows: &[Vec<JoinValue>], key_columns: &[usize]) -> Self {
        let mut rows_by_key = std::collections::HashMap::<Vec<u8>, Vec<Vec<JoinValue>>>::new();
        for row in rows {
            let key = key_columns
                .iter()
                .map(|column| row[*column].clone())
                .collect::<Vec<_>>();
            rows_by_key
                .entry(encode_key(&key))
                .or_default()
                .push(row.clone());
        }
        Self {
            rows_by_key: std::sync::Arc::new(rows_by_key),
        }
    }
}

impl IndexJoinExecutorBuilder for BenchmarkIndexBuilder {
    fn build(
        &self,
        lookup_contents: &[IndexJoinLookupContent],
    ) -> Result<Vec<Vec<JoinValue>>, String> {
        let mut rows = Vec::new();
        for content in lookup_contents {
            if let Some(matches) = self.rows_by_key.get(&encode_key(&content.keys)) {
                rows.extend(matches.iter().cloned());
            }
        }
        Ok(rows)
    }
}

/// 构造 IndexJoin 行（可选逆序）。
fn index_join_rows(row_count: usize, reversed: bool) -> Vec<Vec<JoinValue>> {
    index_join_rows_with_payload(row_count, reversed, 64)
}

/// 构造带载荷字节的 IndexJoin 行。
fn index_join_rows_with_payload(
    row_count: usize,
    reversed: bool,
    payload_bytes: usize,
) -> Vec<Vec<JoinValue>> {
    let positions: Box<dyn Iterator<Item = usize>> = if reversed {
        Box::new((0..row_count).rev())
    } else {
        Box::new(0..row_count)
    };
    positions
        .map(|row| {
            vec![
                JoinValue::Int(row as i64),
                JoinValue::Float(row as f64),
                JoinValue::Bytes(vec![b'x'; payload_bytes]),
            ]
        })
        .collect()
}

/// 构造 Inner Joiner。
fn benchmark_inner_joiner() -> Joiner {
    Joiner::new(
        JoinType::Inner,
        false,
        vec![JoinValue::Null; 3],
        Vec::new(),
        None,
        false,
        1024,
    )
    .expect("build benchmark inner joiner")
}

/// 抽干 IndexLookUpMergeJoin 并返回行数。
fn drain_index_merge_join(executor: &mut IndexLookUpMergeJoin) -> usize {
    executor.open().expect("open index lookup merge join");
    let mut count = 0;
    loop {
        let result = executor
            .next(1024)
            .expect("execute index lookup merge join");
        assert!(result.error.is_none());
        if result.rows.is_empty() {
            break;
        }
        count += result.rows.len();
    }
    executor.close();
    count
}

/// 运行 Index Merge Join（可选外表排序）。
fn run_index_merge_join_case(row_count: usize, need_outer_sort: bool) -> usize {
    let inner_rows = index_join_rows(row_count + row_count / 40, false);
    let outer_rows = index_join_rows(row_count, need_outer_sort);
    let builder = BenchmarkIndexBuilder::new(&inner_rows, &[0, 1]);
    let mut executor = IndexLookUpMergeJoin::new_with_outer_sort(
        outer_rows,
        vec![0, 1],
        vec![0, 1],
        Box::new(builder),
        benchmark_inner_joiner(),
        1,
        4096,
        need_outer_sort,
    )
    .expect("build index lookup merge join benchmark case");
    drain_index_merge_join(&mut executor)
}

#[test]
/// Index Merge Join：区分是否需要外表排序的路径。
fn benchmark_index_merge_join_distinguishes_outer_sort_paths() {
    assert_eq!(run_index_merge_join_case(10_000, true), 10_000);
    assert_eq!(run_index_merge_join_case(10_000, false), 10_000);
}

/// 运行 Index LookUp Join。
fn run_index_lookup_join_case(row_count: usize) -> usize {
    let inner_rows = index_join_rows(row_count + row_count / 40, false);
    let outer_rows = index_join_rows(row_count, false);
    let builder = BenchmarkIndexBuilder::new(&inner_rows, &[0, 1]);
    let mut executor = IndexLookUpJoin::new(
        IndexOuterCtx {
            rows: outer_rows,
            key_columns: vec![0, 1],
            filters: Vec::new(),
        },
        IndexInnerCtx {
            builder: Box::new(builder),
            key_columns: vec![0, 1],
            key_column_ids: vec![0, 1],
            null_safe: false,
        },
        benchmark_inner_joiner(),
        true,
        1,
        4096,
    )
    .expect("build index lookup join benchmark case");
    executor.open().expect("open index lookup join");
    let mut count = 0;
    loop {
        let rows = executor.next(1024).expect("execute index lookup join");
        if rows.is_empty() {
            break;
        }
        count += rows.len();
    }
    executor.close();
    count
}

#[test]
/// Index Inner Hash Join：完整抽干。
fn benchmark_index_inner_hash_join_fully_drains_results() {
    assert_eq!(run_index_lookup_join_case(10_000), 10_000);
}

/// 运行 Index Nested Loop Hash Join。
fn run_index_nested_loop_hash_join_case(row_count: usize) -> usize {
    let inner_rows = index_join_rows(row_count + row_count / 40, false);
    let outer_rows = index_join_rows(row_count, false);
    let builder = BenchmarkIndexBuilder::new(&inner_rows, &[0, 1]);
    let mut executor = IndexNestedLoopHashJoin::new(
        outer_rows,
        vec![0, 1],
        vec![0, 1],
        Box::new(builder),
        benchmark_inner_joiner(),
        true,
        1,
    )
    .expect("build index nested-loop hash join benchmark case");
    executor.open().expect("open index nested-loop hash join");
    let mut count = 0;
    loop {
        let result = executor
            .next(1024)
            .expect("execute index nested-loop hash join");
        assert!(result.error.is_none());
        if result.rows.is_empty() {
            break;
        }
        count += result.rows.len();
    }
    executor.close();
    count
}

#[test]
/// Index Outer Hash Join：完整抽干。
fn benchmark_index_outer_hash_join_fully_drains_results() {
    assert_eq!(run_index_nested_loop_hash_join_case(10_000), 10_000);
}

#[derive(Clone, Copy)]
/// IndexJoin 四条路径种类。
enum IndexJoinBenchmarkKind {
    MergeWithOuterSort,
    MergeWithoutOuterSort,
    InnerHash,
    OuterHash,
}

/// 单条 IndexJoin 路径执行。
fn run_index_join_lane(
    kind: IndexJoinBenchmarkKind,
    outer_rows: Vec<Vec<JoinValue>>,
    builder: BenchmarkIndexBuilder,
) -> usize {
    match kind {
        IndexJoinBenchmarkKind::MergeWithOuterSort
        | IndexJoinBenchmarkKind::MergeWithoutOuterSort => {
            let need_outer_sort = matches!(kind, IndexJoinBenchmarkKind::MergeWithOuterSort);
            let mut executor = IndexLookUpMergeJoin::new_with_outer_sort(
                outer_rows,
                vec![0, 1],
                vec![0, 1],
                Box::new(builder),
                benchmark_inner_joiner(),
                1,
                4096,
                need_outer_sort,
            )
            .expect("build concurrent index merge join lane");
            drain_index_merge_join(&mut executor)
        }
        IndexJoinBenchmarkKind::InnerHash => {
            let mut executor = IndexLookUpJoin::new(
                IndexOuterCtx {
                    rows: outer_rows,
                    key_columns: vec![0, 1],
                    filters: Vec::new(),
                },
                IndexInnerCtx {
                    builder: Box::new(builder),
                    key_columns: vec![0, 1],
                    key_column_ids: vec![0, 1],
                    null_safe: false,
                },
                benchmark_inner_joiner(),
                true,
                1,
                4096,
            )
            .expect("build concurrent index lookup join lane");
            executor.open().expect("open concurrent index lookup lane");
            let mut count = 0;
            loop {
                let rows = executor.next(1024).expect("execute index lookup lane");
                if rows.is_empty() {
                    break;
                }
                count += rows.len();
            }
            executor.close();
            count
        }
        IndexJoinBenchmarkKind::OuterHash => {
            let mut executor = IndexNestedLoopHashJoin::new(
                outer_rows,
                vec![0, 1],
                vec![0, 1],
                Box::new(builder),
                benchmark_inner_joiner(),
                true,
                1,
            )
            .expect("build concurrent index nested-loop hash join lane");
            executor
                .open()
                .expect("open concurrent index nested-loop hash lane");
            let mut count = 0;
            loop {
                let result = executor
                    .next(1024)
                    .expect("execute index nested-loop hash join lane");
                assert!(result.error.is_none());
                if result.rows.is_empty() {
                    break;
                }
                count += result.rows.len();
            }
            executor.close();
            count
        }
    }
}

/// IndexJoin 并发矩阵。
fn run_index_join_concurrency_matrix(
    outer_row_count: usize,
    inner_row_count: usize,
    concurrency: usize,
    payload_bytes: usize,
) {
    let inner_rows = index_join_rows_with_payload(inner_row_count, false, payload_bytes);
    let builder = BenchmarkIndexBuilder::new(&inner_rows, &[0, 1]);
    for kind in [
        IndexJoinBenchmarkKind::MergeWithOuterSort,
        IndexJoinBenchmarkKind::MergeWithoutOuterSort,
        IndexJoinBenchmarkKind::InnerHash,
        IndexJoinBenchmarkKind::OuterHash,
    ] {
        let reverse = matches!(kind, IndexJoinBenchmarkKind::MergeWithOuterSort);
        let mut lanes = vec![Vec::new(); concurrency];
        for (index, row) in index_join_rows_with_payload(outer_row_count, reverse, payload_bytes)
            .into_iter()
            .enumerate()
        {
            lanes[index % concurrency].push(row);
        }
        let joined = std::thread::scope(|scope| {
            let handles = lanes
                .into_iter()
                .map(|outer_rows| {
                    let builder = builder.clone();
                    scope.spawn(move || run_index_join_lane(kind, outer_rows, builder))
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("index join lane panicked"))
                .sum::<usize>()
        });
        assert_eq!(joined, outer_row_count);
    }
}

#[test]
/// IndexJoin 四路径并发代表规模。
fn benchmark_index_join_four_paths_use_real_concurrency_representative() {
    run_index_join_concurrency_matrix(10_000, 10_240, 4, 64);
}

#[test]
#[ignore = "full Go IndexJoin scale and 5KiB payload; run explicitly for migration evidence"]
/// IndexJoin 四路径精确 Go 规模（ignore）。
fn benchmark_index_join_four_paths_run_exact_go_scale() {
    run_index_join_concurrency_matrix(100_000, 102_400, 4, 5 * 1024);
}

/// JoinValue → SortValue。
/// JoinValue 转为 SortExec 使用的 SortValue。
fn join_value_to_sort_value(value: JoinValue) -> SortValue {
    match value {
        JoinValue::Null => SortValue::Null,
        JoinValue::Int(value) => SortValue::Int(value),
        JoinValue::UInt(value) => SortValue::UInt(value),
        JoinValue::Float(value) => SortValue::Float(value),
        JoinValue::Bytes(value) => SortValue::Bytes(value),
        JoinValue::Text(value) => SortValue::Bytes(value.into_bytes()),
        JoinValue::Bool(value) => SortValue::UInt(u64::from(value)),
    }
}

/// SortValue → JoinValue。
/// SortValue 转回 JoinValue。
fn sort_value_to_join_value(value: SortValue) -> JoinValue {
    match value {
        SortValue::Null => JoinValue::Null,
        SortValue::Int(value) => JoinValue::Int(value),
        SortValue::UInt(value) => JoinValue::UInt(value),
        SortValue::Float(value) => JoinValue::Float(value),
        SortValue::Bytes(value) => JoinValue::Bytes(value),
    }
}

/// 用 SortExec 对连接行排序。
fn sort_join_rows(rows: Vec<Vec<JoinValue>>, concurrency: usize) -> Vec<Vec<JoinValue>> {
    let chunks = rows
        .chunks(1024)
        .map(|chunk| {
            DataChunk::new(
                chunk
                    .iter()
                    .cloned()
                    .map(|row| Row(row.into_iter().map(join_value_to_sort_value).collect()))
                    .collect(),
            )
        })
        .collect();
    let child = Box::new(VecRowSource::new(chunks));
    let mut sort = SortExec::new(
        child,
        vec![SortKey::asc(0), SortKey::asc(1)],
        concurrency,
        1024,
        -1,
    );
    let mut result = Vec::new();
    loop {
        let chunk = sort.Next(1024).expect("execute MergeJoin input SortExec");
        if chunk.rows.is_empty() {
            break;
        }
        result.extend(
            chunk
                .rows
                .into_iter()
                .map(|row| row.0.into_iter().map(sort_value_to_join_value).collect()),
        );
    }
    sort.Close().expect("close MergeJoin input SortExec");
    result
}

/// 构造 MergeJoin 外表行。
fn merge_join_outer_rows(row_count: usize, payload_bytes: usize) -> Vec<Vec<JoinValue>> {
    (0..row_count)
        .rev()
        .map(|row| {
            vec![
                JoinValue::Int(row as i64),
                JoinValue::Float(row as f64),
                JoinValue::Bytes(vec![b'x'; payload_bytes]),
            ]
        })
        .collect()
}

/// 构造 MergeJoin 内表行。
fn merge_join_inner_rows(
    outer_row_count: usize,
    duplicate_count: usize,
    redundant_count: usize,
    payload_bytes: usize,
) -> Vec<Vec<JoinValue>> {
    let row_count = outer_row_count * duplicate_count + redundant_count;
    (0..row_count)
        .rev()
        .map(|row| {
            let key = row / duplicate_count;
            vec![
                JoinValue::Int(key as i64),
                JoinValue::Float(key as f64),
                JoinValue::Bytes(vec![b'x'; payload_bytes]),
            ]
        })
        .collect()
}

/// 单通道 MergeJoin。
fn run_merge_join_lane(
    outer_rows: Vec<Vec<JoinValue>>,
    inner_rows: Vec<Vec<JoinValue>>,
    inline_projection: bool,
) -> usize {
    let outer_table =
        MergeJoinTable::new(outer_rows, vec![0, 1], false).expect("build outer MergeJoin table");
    let inner_table =
        MergeJoinTable::new(inner_rows, vec![0, 1], true).expect("build inner MergeJoin table");
    let children_used = inline_projection.then(|| [vec![0], vec![1]]);
    let joiner = Joiner::new(
        JoinType::Inner,
        false,
        vec![JoinValue::Null; 3],
        Vec::new(),
        children_used,
        false,
        1024,
    )
    .expect("build MergeJoin benchmark joiner");
    let mut executor =
        MergeJoinExec::new(outer_table, inner_table, joiner).expect("build MergeJoin executor");
    executor.open().expect("open MergeJoin executor");
    let mut count = 0;
    loop {
        let rows = executor.next(1024).expect("execute MergeJoin benchmark");
        if rows.is_empty() {
            break;
        }
        assert!(
            rows.iter()
                .all(|row| row.len() == if inline_projection { 2 } else { 6 })
        );
        count += rows.len();
    }
    executor.close();
    count
}

/// MergeJoin：排序 + shuffle + 内联投影组合。
fn run_merge_join_sort_shuffle_case(
    total_result_rows: usize,
    duplicate_count: usize,
    redundant_count: usize,
    inline_projection: bool,
    payload_bytes: usize,
) {
    let outer_row_count = total_result_rows / duplicate_count;
    let outer_rows = sort_join_rows(merge_join_outer_rows(outer_row_count, payload_bytes), 2);
    let inner_rows = sort_join_rows(
        merge_join_inner_rows(
            outer_row_count,
            duplicate_count,
            redundant_count,
            payload_bytes,
        ),
        2,
    );
    let children_used = inline_projection.then(|| [vec![0], vec![1]]);
    let joiner = Joiner::new(
        JoinType::Inner,
        false,
        vec![JoinValue::Null; 3],
        Vec::new(),
        children_used,
        false,
        1024,
    )
    .expect("build shuffled MergeJoin benchmark joiner");
    let mut executor =
        ShuffleMergeJoinExec::new(outer_rows, inner_rows, vec![0, 1], vec![0, 1], joiner, 2)
            .expect("build real ShuffleMergeJoinExec");
    executor.open().expect("open real ShuffleMergeJoinExec");
    let mut count = 0;
    loop {
        let rows = executor
            .next(1024)
            .expect("execute real ShuffleMergeJoinExec");
        if rows.is_empty() {
            break;
        }
        assert!(
            rows.iter()
                .all(|row| row.len() == if inline_projection { 2 } else { 6 })
        );
        count += rows.len();
    }
    executor.close();
    assert_eq!(count, total_result_rows);
}

/// MergeJoin Go 矩阵点。
fn run_merge_join_go_matrix(total_result_rows: usize, payload_bytes: usize) {
    for (duplicate_count, redundant_count) in [(1, 0), (100, 0), (10_000, 0), (1, 30_000)] {
        for inline_projection in [false, true] {
            run_merge_join_sort_shuffle_case(
                total_result_rows,
                duplicate_count,
                redundant_count,
                inline_projection,
                payload_bytes,
            );
        }
    }
}

#[test]
/// MergeJoin 排序/shuffle/内联投影代表规模。
fn benchmark_merge_join_sort_shuffle_and_inline_projection_representative() {
    run_merge_join_go_matrix(30_000, 64);
}

#[test]
#[ignore = "full Go MergeJoin 300k result and 5KiB payload matrix; run explicitly"]
/// MergeJoin 精确 Go 矩阵（ignore）。
fn benchmark_merge_join_runs_exact_go_matrix() {
    run_merge_join_go_matrix(300_000, 5 * 1024);
}

/// Limit 基准输入行。
fn limit_benchmark_input(rows: usize) -> Vec<Row> {
    (0..rows)
        .map(|row| {
            Row(vec![
                SortValue::Int(row as i64),
                SortValue::Int((row * 2) as i64),
            ])
        })
        .collect()
}

#[test]
/// Limit：真实 Select 运行时代表规模。
fn benchmark_limit_exec_runs_real_select_runtime_representative() {
    let rows = ExecuteLimitRows(limit_benchmark_input(30), 10, 10, 8, None)
        .expect("execute Limit through physical SelectRuntime adapter");
    assert_eq!(rows.len(), 10);
    assert_eq!(rows.first().unwrap().0[0], SortValue::Int(10));
    assert_eq!(rows.last().unwrap().0[0], SortValue::Int(19));
}

/// Limit：内联/外层投影路径。
fn run_limit_go_case(using_inline_projection: bool) -> Vec<Row> {
    let input = limit_benchmark_input(30_000);
    if using_inline_projection {
        ExecuteLimitRows(input, 10_000, 10_000, 1024, Some(&[1]))
            .expect("execute inline-projection Limit benchmark path")
    } else {
        let rows = ExecuteLimitRows(input, 10_000, 10_000, 1024, None)
            .expect("execute outer-projection Limit benchmark path");
        crate::projection::ProjectRows(rows, &[1])
            .expect("execute outer Projection after Limit benchmark path")
    }
}

#[test]
#[ignore = "full Go default Limit scale; run explicitly for migration evidence"]
/// Limit：精确对齐 Go 内联/外层投影。
fn benchmark_limit_exec_runs_exact_go_inline_and_outer_projection_paths() {
    let expected = (10_000..20_000)
        .map(|row| Row(vec![SortValue::Int((row * 2) as i64)]))
        .collect::<Vec<_>>();
    assert_eq!(run_limit_go_case(true), expected);
    assert_eq!(run_limit_go_case(false), expected);
}

/// 临时慢查询日志文件，Drop 时删除。
struct TemporarySlowLog {
    path: PathBuf,
    file: File,
}

impl TemporarySlowLog {
    fn new(bytes: &[u8]) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "astersql-benchmark-slow-log-{}-{unique}",
            std::process::id()
        ));
        let mut writer = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("create temporary slow log");
        writer.write_all(bytes).expect("write temporary slow log");
        writer.sync_all().expect("sync temporary slow log");
        drop(writer);
        let file = File::open(&path).expect("open temporary slow log for reading");
        Self { path, file }
    }
}

impl Drop for TemporarySlowLog {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).expect("remove temporary slow log");
    }
}

#[test]
/// ReadLastLines：真实文件尾部边界。
fn benchmark_read_last_lines_uses_real_file_and_preserves_tail_boundaries() {
    let mut log = TemporarySlowLog::new(b"first\nsecond-tail");
    let (lines, read_bytes) =
        ReadLastLinesFromFile(&mut log.file, 17, 64).expect("read slow-log tail from a real file");
    assert_eq!(lines, vec!["first", "second-tail"]);
    assert_eq!(read_bytes, 17);

    let (tail, tail_bytes) =
        ReadLastLinesFromFile(&mut log.file, 17, 11).expect("read exact final-line boundary");
    assert_eq!(tail, vec!["second-tail"]);
    assert_eq!(tail_bytes, 11);

    let (prefix, prefix_bytes) =
        ReadLastLinesFromFile(&mut log.file, 6, 64).expect("read at an earlier end cursor");
    assert_eq!(prefix, vec!["first"]);
    assert_eq!(prefix_bytes, 6);
}

#[test]
#[ignore = "real Go 10MiB single-line slow-log file I/O; run explicitly"]
/// ReadLastLines：超长行精确 Go 规模。
fn benchmark_read_last_lines_of_huge_line_runs_exact_go_scale() {
    const HUGE_LINE_BYTES: usize = 10 * 1024 * 1024;
    let huge_line = (0..HUGE_LINE_BYTES)
        .map(|index| b'a' + (index % 26) as u8)
        .collect::<Vec<_>>();
    let mut log = TemporarySlowLog::new(&huge_line);
    let (lines, read_bytes) = ReadLastLinesFromFile(
        &mut log.file,
        HUGE_LINE_BYTES as i64,
        crate::slow_query::maxReadCacheSize,
    )
    .expect("read real 10MiB single-line slow-log file");
    assert_eq!(read_bytes, HUGE_LINE_BYTES);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].as_bytes(), huge_line);
}

#[test]
/// CompleteInsertError：分类并链接 out-of-range 原因。
fn benchmark_complete_insert_err_classifies_and_chains_out_of_range_cause() {
    let cause = DmlErrorCause::new(
        InsertErrorKind::WarnDataOutOfRange,
        "BIGINT value is out of range",
    );
    let completed = CompleteInsertErrorForColumn("a", 0, cause);
    assert_eq!(completed.kind(), InsertErrorKind::WarnDataOutOfRange);
    assert_eq!(
        completed.to_string(),
        "Out of range value for column 'a' at row 1"
    );
    assert_eq!(
        std::error::Error::source(&completed)
            .expect("completed insert error preserves source")
            .to_string(),
        "BIGINT value is out of range"
    );
}

#[test]
/// CompleteLoadError：分类并链接 data-too-long 原因。
fn benchmark_complete_load_err_classifies_and_chains_data_too_long_cause() {
    let cause = DmlErrorCause::new(InsertErrorKind::DataTooLong, "value exceeds BLOB capacity");
    let completed = CompleteLoadErrorForColumn("a", 0, cause);
    assert_eq!(completed.kind(), InsertErrorKind::Truncated);
    assert_eq!(
        completed.to_string(),
        "Data truncated for column 'a' at row 0"
    );
    assert_eq!(
        std::error::Error::source(&completed)
            .expect("completed load error preserves source")
            .to_string(),
        "value exceeds BLOB capacity"
    );
}
