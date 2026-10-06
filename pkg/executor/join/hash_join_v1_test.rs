// Copyright 2026 AsterSQL.

// Hash Join v1 执行器的行为回归测试。
//
// 覆盖内连接与外连接的行拼接语义、构建侧作为 outer 时的未匹配行输出，
// 以及 Nested Loop Apply 跨批次拉取时按关联键复用内表结果的缓存行为。

use super::hash_join_test_util::{HashJoinInfo, build_hash_join_v1_exec, generate_cmp_func};
use super::hash_join_v1::NestedLoopApplyExec;
use super::joiner::{JoinType, Joiner, Row};
use super::row_table_builder::{Chunk, Value};
use std::sync::{
    Arc,
    atomic::{AtomicI64, AtomicUsize, Ordering},
};

/// 把整数切片转换为测试执行器使用的行值。
fn row(values: &[i64]) -> Row {
    values.iter().copied().map(Value::Int).collect()
}

/// 构造并完整执行 Hash Join，排序结果以消除并发输出顺序的影响。
fn execute(info: HashJoinInfo) -> Vec<Row> {
    let mut executor = build_hash_join_v1_exec(&info).unwrap();
    let mut rows = executor.execute_all().unwrap();
    rows.sort_by(generate_cmp_func());
    rows
}

/// 生成单列整型连接键的通用配置；`-1` 作为外连接未匹配侧的占位值。
fn info(join_type: JoinType, build: Chunk, probe: Chunk) -> HashJoinInfo {
    HashJoinInfo {
        join_type,
        build_key_indices: vec![0],
        probe_key_indices: vec![0],
        build_chunks: vec![build],
        probe_chunks: vec![probe],
        outer_is_right: false,
        build_side_is_outer: false,
        null_aware: false,
        concurrency: 2,
        max_chunk_size: 2,
        default_inner: vec![Value::Int(-1), Value::Int(-1)],
        conditions: Vec::new(),
        children_used: None,
    }
}

#[test]
fn hash_join_v1_inner_and_left_outer_match_go_row_semantics() {
    // 同键的多条构建侧记录都应输出，不能因哈希键相同而折叠。
    let inner = execute(info(
        JoinType::Inner,
        vec![row(&[1, 10]), row(&[1, 11])],
        vec![row(&[1]), row(&[2])],
    ));
    assert_eq!(inner, [row(&[1, 1, 10]), row(&[1, 1, 11])]);

    // 左外连接还应为未命中的探测侧记录补齐默认内表行。
    let left = execute(info(
        JoinType::LeftOuter,
        vec![row(&[1, 10])],
        vec![row(&[1]), row(&[2])],
    ));
    assert_eq!(left, [row(&[1, 1, 10]), row(&[2, -1, -1])]);
}

#[test]
fn hash_join_v1_outer_build_emits_only_unmatched_build_rows() {
    // 构建侧同时作为 outer 时，扫描结束后只补发未匹配的构建行。
    let build = vec![row(&[1, 10]), row(&[2, 20])];
    let context = HashJoinInfo {
        join_type: JoinType::RightOuter,
        build_key_indices: vec![0],
        probe_key_indices: vec![0],
        build_chunks: vec![build],
        probe_chunks: vec![vec![row(&[1]), row(&[3])]],
        outer_is_right: true,
        build_side_is_outer: true,
        null_aware: false,
        concurrency: 1,
        max_chunk_size: 4,
        default_inner: vec![Value::Int(-1)],
        conditions: Vec::new(),
        children_used: None,
    };

    assert_eq!(execute(context), [row(&[-1, 2, 20]), row(&[1, 1, 10])]);
}

#[test]
fn hash_join_v1_can_be_opened_again_after_exhaustion_and_close() {
    let mut executor =
        build_hash_join_v1_exec(&info(JoinType::Inner, vec![row(&[1, 10])], vec![row(&[1])]))
            .unwrap();

    assert_eq!(executor.execute_all().unwrap(), [row(&[1, 1, 10])]);
    executor.open().unwrap();
    assert_eq!(executor.execute_all().unwrap(), [row(&[1, 1, 10])]);

    executor.close();
    executor.open().unwrap();
    assert_eq!(executor.execute_all().unwrap(), [row(&[1, 1, 10])]);
}

#[test]
fn hash_join_v1_records_constructed_rows_across_repeated_open() {
    let runtime_stats = Arc::new(std::sync::Mutex::new(
        astersql_util_execdetails::execdetails::RuntimeStatsColl::default(),
    ));
    let mut executor =
        build_hash_join_v1_exec(&info(JoinType::Inner, vec![row(&[1, 10])], vec![row(&[1])]))
            .unwrap()
            .with_runtime_stats(19, runtime_stats.clone());

    for _ in 0..2 {
        executor.open().unwrap();
        assert_eq!(executor.execute_all().unwrap(), [row(&[1, 1, 10])]);
        executor.close();
    }

    let snapshot = runtime_stats
        .lock()
        .unwrap()
        .GetRootHashStateRowsSnapshot(19)
        .unwrap();
    assert_eq!(snapshot.Rows, 2);
    assert!(!snapshot.Invalid());
}

#[test]
fn nested_loop_apply_reuses_inner_results_for_equal_correlated_keys() {
    // 两条关联键同为 1 的 outer 行应共享一次内表构建，且缓存跨 next 批次生效。
    let calls = Arc::new(AtomicUsize::new(0));
    let call_counter = Arc::clone(&calls);
    let joiner = Joiner::new(
        JoinType::LeftOuter,
        false,
        row(&[-1]),
        Vec::new(),
        None,
        false,
        8,
    )
    .unwrap();
    let mut apply = NestedLoopApplyExec::new(
        vec![row(&[1, 100]), row(&[1, 101]), row(&[2, 200])],
        Box::new(move |outer| {
            call_counter.fetch_add(1, Ordering::Relaxed);
            if outer[0] == Value::Int(1) {
                Ok(vec![row(&[10])])
            } else {
                Ok(Vec::new())
            }
        }),
        joiner,
        vec![0],
    );
    apply.open();

    assert_eq!(apply.next(1).unwrap(), [row(&[1, 100, 10])]);
    assert_eq!(
        apply.next(2).unwrap(),
        [row(&[1, 101, 10]), row(&[2, 200, -1])]
    );
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert_eq!(apply.cache_info.hits, 1);
    assert_eq!(apply.cache_info.misses, 2);
    assert_eq!(apply.cache_info.entries, 2);
}

#[test]
fn nested_loop_apply_open_discards_cached_inner_results_like_go() {
    let inner_value = Arc::new(AtomicI64::new(10));
    let builder_value = Arc::clone(&inner_value);
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
    let mut apply = NestedLoopApplyExec::new(
        vec![row(&[1])],
        Box::new(move |_| Ok(vec![row(&[builder_value.load(Ordering::Relaxed)])])),
        joiner,
        vec![0],
    );

    apply.open();
    assert_eq!(apply.next(1).unwrap(), [row(&[1, 10])]);

    inner_value.store(20, Ordering::Relaxed);
    apply.open();
    assert_eq!(apply.next(1).unwrap(), [row(&[1, 20])]);
    assert_eq!(apply.cache_info.hits, 0);
    assert_eq!(apply.cache_info.misses, 1);
}
