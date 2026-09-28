// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 对应 `pkg/executor/join/test/mergejoin/merge_join_test.go`。
// `mergejoin_test` 只依赖 `testkit`（不直接导入 executor 内部包），但当前会话执行器
// （`ConcreteSession`）尚未实现多表 JOIN（见 `pkg/session/runtime.rs` 的
// `"joins are not supported by the session KV executor"`），因此 Go 版本里通过
// `tk.MustQuery("... TIDB_SMJ ...")` 触发的整套 SQL JOIN 目前无法经会话执行。
// 本文件保留 Go 测试真实覆盖的两层语义：
//   1. 用真实 `TestKit`/`AnalyzeStatsStore` 建表、插入数据、设置
//      `tidb_merge_join_concurrency`/`tidb_max_chunk_size`/`tidb_mem_quota_query`
//      等会话变量（这些均是当前会话执行器已支持的真实能力）；
//   2. 直接调用生产 `MergeJoinExec`/`ShuffleMergeJoinExec`（`astersql-executor-join`
//      crate 的真实合并连接算法），使用与 Go 测试完全一致的数据集和过滤条件，
//      核对连接结果与 Go 断言的期望行一致。
// Go 版本额外验证的 `MemTracker`/`DiskTracker` 落盘行为依赖会话执行器的
// `SessionVars.MemTracker` 与 failpoint 注入的 spill action；当前 Rust 合并连接
// 实现尚未接入内存/磁盘 tracker（见 `pkg/executor/join/merge_join.rs` 顶部保留的
// 旧版注释块），因此该部分暂不可移植，未在本文件中假造。

use astersql_executor_join::joiner::{JoinType, Joiner};
use astersql_executor_join::merge_join::{MergeJoinExec, MergeJoinTable, ShuffleMergeJoinExec};
use astersql_executor_join::row_table_builder::Value as JoinValue;
use astersql_sessionctx_vardef::DefInitChunkSize;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;

type JoinRow = Vec<JoinValue>;

/// 把整数切片转成合并连接用的一行（各列均为 `JoinValue::Int`）。
fn int_row(values: &[i64]) -> JoinRow {
    values.iter().copied().map(JoinValue::Int).collect()
}

/// 按指定列上的 Int 键对结果行排序，便于与期望集比较。
fn sort_rows_by_key(rows: &mut [JoinRow], key_index: usize) {
    rows.sort_by_key(|row| match row[key_index] {
        JoinValue::Int(value) => value,
        _ => panic!("merge join test rows always key on an Int column"),
    });
}

/// 将行集格式化为排序后的调试字符串，忽略输出顺序差异。
fn debug_sorted(rows: &[JoinRow]) -> Vec<String> {
    let mut rendered: Vec<String> = rows.iter().map(|row| format!("{row:?}")).collect();
    rendered.sort();
    rendered
}

/// 对应 Go `checkMergeAndRun` 的实际连接执行部分：直接跑生产 `MergeJoinExec`，
/// 而不是通过 `explain format = 'brief'` 断言计划里出现 "MergeJoin"
/// （当前会话执行器不构建物理计划字符串，见文件头注释）。
fn run_merge_join(
    join_type: JoinType,
    outer_rows: Vec<JoinRow>,
    inner_rows: Vec<JoinRow>,
    join_keys: Vec<usize>,
    inner_column_count: usize,
) -> Vec<JoinRow> {
    let outer_table =
        MergeJoinTable::new(outer_rows, join_keys.clone(), false).expect("build outer table");
    let inner_table = MergeJoinTable::new(inner_rows, join_keys, true).expect("build inner table");
    let joiner = Joiner::new(
        join_type,
        false,
        vec![JoinValue::Null; inner_column_count],
        Vec::new(),
        None,
        false,
        1024,
    )
    .expect("build real merge join joiner");
    let mut executor =
        MergeJoinExec::new(outer_table, inner_table, joiner).expect("build real MergeJoinExec");
    executor.open().expect("open real MergeJoinExec");
    let mut rows = Vec::new();
    loop {
        let batch = executor.next(1024).expect("execute real MergeJoinExec");
        if batch.is_empty() {
            break;
        }
        rows.extend(batch);
    }
    executor.close();
    rows
}

/// 对应 Go 并发 shuffle 归并路径：用 `ShuffleMergeJoinExec` 按 `concurrency` 分片执行。
fn run_shuffle_merge_join(
    join_type: JoinType,
    outer_rows: Vec<JoinRow>,
    inner_rows: Vec<JoinRow>,
    join_keys: Vec<usize>,
    inner_column_count: usize,
    concurrency: usize,
) -> Vec<JoinRow> {
    let joiner = Joiner::new(
        join_type,
        false,
        vec![JoinValue::Null; inner_column_count],
        Vec::new(),
        None,
        false,
        1024,
    )
    .expect("build real shuffle merge join joiner");
    let mut executor = ShuffleMergeJoinExec::new(
        outer_rows,
        inner_rows,
        join_keys.clone(),
        join_keys,
        joiner,
        concurrency,
    )
    .expect("build real ShuffleMergeJoinExec");
    executor.open().expect("open real ShuffleMergeJoinExec");
    let mut rows = Vec::new();
    loop {
        let batch = executor
            .next(1024)
            .expect("execute real ShuffleMergeJoinExec");
        if batch.is_empty() {
            break;
        }
        rows.extend(batch);
    }
    executor.close();
    rows
}

/// 对应 Go `TestShuffleMergeJoinInDisk`/`TestMergeJoinInDisk` 建表插入前的会话准备：
/// 真实 `TestKit` + `AnalyzeStatsStore`，验证建表、插入和会话变量设置全部可用。
/// 当前会话执行器的通用 `SELECT col FROM table` 尚未返回真实行数据（见
/// `pkg/session/runtime.rs` 的 `execute_relational_select`，只做谓词列统计，不产出
/// `RecordSet`），因此这里只对能在会话执行器里真正落地的语句做 `MustExec` 断言，
/// 不虚构一个当前不可用的 `MustQuery` 行校验。
#[test]
fn merge_join_setup_uses_real_testkit_session_variables_and_tables() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@tidb_mem_quota_query=1", Vec::new());
    tk.MustExec("set @@tidb_merge_join_concurrency=4", Vec::new());
    tk.MustExec("set @@tidb_max_chunk_size=32", Vec::new());
    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec("drop table if exists t1", Vec::new());
    tk.MustExec("create table t(c1 int, c2 int)", Vec::new());
    tk.MustExec("create table t1(c1 int, c2 int)", Vec::new());
    tk.MustExec("insert into t values(1,1),(2,2),(3,3),(4,4)", Vec::new());
    tk.MustExec("insert into t1 values(1,3),(4,4)", Vec::new());
}

/// 对应 Go `TestMergeJoinInDisk`：t(1,1)；t1(1,3),(4,4)；
/// `t1 left outer join t on t.c1=t1.c1 where t.c1=1 or t1.c2>20`。
/// 该 WHERE 条件同时引用两侧列，在 LEFT JOIN 之后才能求值（无法下推到任一侧的
/// `MergeJoinTable`），因此在真实合并连接结果之上，用与 Go 相同的谓词做后置过滤，
/// 对应 Go 执行计划里 MergeJoin 之上的 Selection 节点。
#[test]
fn merge_join_in_disk_matches_go_expected_row() {
    let t = vec![int_row(&[1, 1])];
    let t1 = vec![int_row(&[1, 3]), int_row(&[4, 4])];

    let joined = run_merge_join(JoinType::LeftOuter, t1, t, vec![0], 2);
    // 行布局：[t1.c1, t1.c2, t.c1, t.c2]（outer=t1 在前，inner=t 在后）。
    let filtered: Vec<JoinRow> = joined
        .into_iter()
        .filter(|row| {
            let t_c1_is_one = matches!(row[2], JoinValue::Int(1));
            let t1_c2_gt_20 = matches!(row[1], JoinValue::Int(value) if value > 20);
            t_c1_is_one || t1_c2_gt_20
        })
        .collect();

    assert_eq!(
        filtered,
        vec![int_row(&[1, 3, 1, 1])],
        "expected Go row \"1 3 1 1\""
    );
}

/// 对应 Go `TestShuffleMergeJoinInDisk`：t(1,1),(2,2),(3,3),(4,4)；
/// t1 为 1..=1024 的 (i,i) 行（Go 按 4 行一批插入，等价于连续整数）；
/// 同一个 `t.c1=1 or t1.c2>20` 谓词，用 `ShuffleMergeJoinExec`
/// （对应 Go `tidb_merge_join_concurrency=4` 触发的并发 shuffle）。
#[test]
fn shuffle_merge_join_in_disk_matches_go_expected_rows() {
    let t = vec![
        int_row(&[1, 1]),
        int_row(&[2, 2]),
        int_row(&[3, 3]),
        int_row(&[4, 4]),
    ];
    let t1: Vec<JoinRow> = (1..=1024i64)
        .map(|value| int_row(&[value, value]))
        .collect();

    let joined = run_shuffle_merge_join(JoinType::LeftOuter, t1, t, vec![0], 2, 4);
    let mut filtered: Vec<JoinRow> = joined
        .into_iter()
        .filter(|row| {
            let t_c1_is_one = matches!(row[2], JoinValue::Int(1));
            let t1_c2_gt_20 = matches!(row[1], JoinValue::Int(value) if value > 20);
            t_c1_is_one || t1_c2_gt_20
        })
        .collect();
    sort_rows_by_key(&mut filtered, 0);

    let mut expected = vec![int_row(&[1, 1, 1, 1])];
    expected.extend((21..=1024i64).map(|value| {
        vec![
            JoinValue::Int(value),
            JoinValue::Int(value),
            JoinValue::Null,
            JoinValue::Null,
        ]
    }));
    sort_rows_by_key(&mut expected, 0);

    assert_eq!(filtered, expected);
}

/// 对应 Go `TestVectorizedMergeJoin`：在 chunk-size 边界矩阵上验证合并连接结果
/// 与暴力嵌套循环内连接一致，覆盖 Go 用例中的单键、多键、空批次组合。
/// 过滤条件 `t1.b>5 and t2.b<5` 只引用各自表自身列，可安全下推为逐表前置过滤
/// （等价于 Go 计划里 TableReader 下的 Selection cop 算子）。
/// 按各键的重复次数构造 `(key, b)` 行；`b` 用确定性公式生成，便于过滤复现。
fn build_keyed_rows(key_counts: &[i64]) -> Vec<JoinRow> {
    let mut rows = Vec::new();
    for (key, &count) in key_counts.iter().enumerate() {
        if count == 0 {
            continue;
        }
        for offset in 0..count {
            let b = (key as i64 * 7 + offset * 3) % 10;
            rows.push(int_row(&[key as i64, b]));
        }
    }
    rows
}

/// 暴力嵌套循环内连接，作为合并连接正确性的对照期望。
fn brute_force_inner_join(outer: &[JoinRow], inner: &[JoinRow]) -> Vec<JoinRow> {
    let mut result = Vec::new();
    for outer_row in outer {
        for inner_row in inner {
            if outer_row[0] == inner_row[0] {
                let mut row = outer_row.clone();
                row.extend(inner_row.clone());
                result.push(row);
            }
        }
    }
    result
}

/// 对一组键重复计数：先下推过滤，再比对串行与 shuffle 归并结果与暴力期望。
fn assert_merge_join_matches_brute_force(ts1: &[i64], ts2: &[i64]) {
    let mut t1: Vec<JoinRow> = build_keyed_rows(ts1)
        .into_iter()
        .filter(|row| matches!(row[1], JoinValue::Int(value) if value > 5))
        .collect();
    let mut t2: Vec<JoinRow> = build_keyed_rows(ts2)
        .into_iter()
        .filter(|row| matches!(row[1], JoinValue::Int(value) if value < 5))
        .collect();
    sort_rows_by_key(&mut t1, 0);
    sort_rows_by_key(&mut t2, 0);

    let expected = brute_force_inner_join(&t1, &t2);
    let actual = run_merge_join(JoinType::Inner, t1.clone(), t2.clone(), vec![0], 2);
    assert_eq!(debug_sorted(&actual), debug_sorted(&expected));

    let shuffled = run_shuffle_merge_join(JoinType::Inner, t1, t2, vec![0], 2, 4);
    assert_eq!(debug_sorted(&shuffled), debug_sorted(&expected));
}

/// Go `cases` 表中 `chunkSize` 相关的边界矩阵（`chunkSize = vardef.DefInitChunkSize`）。
fn go_chunk_size_boundary_cases() -> Vec<(Vec<i64>, Vec<i64>)> {
    let chunk_size = DefInitChunkSize;
    vec![
        (vec![0], vec![chunk_size]),
        (vec![0], vec![chunk_size - 1]),
        (vec![0], vec![chunk_size + 1]),
        (vec![1], vec![chunk_size]),
        (vec![1], vec![chunk_size - 1]),
        (vec![1], vec![chunk_size + 1]),
        (vec![chunk_size - 1], vec![chunk_size]),
        (vec![chunk_size - 1], vec![chunk_size - 1]),
        (vec![chunk_size - 1], vec![chunk_size + 1]),
        (vec![chunk_size], vec![chunk_size]),
        (vec![chunk_size], vec![chunk_size + 1]),
        (vec![chunk_size + 1], vec![chunk_size + 1]),
        (
            vec![1, 1, 1],
            vec![chunk_size + 1, chunk_size * 5 + 5, chunk_size - 5],
        ),
        (
            vec![0, 0, chunk_size],
            vec![chunk_size + 1, chunk_size * 5 + 5, chunk_size - 5],
        ),
        (
            vec![chunk_size + 1, 0, chunk_size],
            vec![chunk_size + 1, chunk_size * 5 + 5, chunk_size - 5],
        ),
    ]
}

/// 在 Go 版 chunk-size 边界矩阵上双向核对串行与 shuffle 内连接结果。
#[test]
fn merge_join_matches_expected_inner_join_across_go_chunk_size_boundaries() {
    for (ts1, ts2) in go_chunk_size_boundary_cases() {
        assert_merge_join_matches_brute_force(&ts1, &ts2);
        assert_merge_join_matches_brute_force(&ts2, &ts1);
    }
}

/// 对应 Go `TestVectorizedShuffleMergeJoin`：同一矩阵，额外核对并发 shuffle 路径
/// （`tidb_merge_join_concurrency = 4`）与串行合并连接结果一致。
#[test]
fn shuffle_merge_join_matches_expected_inner_join_across_go_chunk_size_boundaries() {
    for (ts1, ts2) in go_chunk_size_boundary_cases() {
        assert_merge_join_matches_brute_force(&ts1, &ts2);
    }
}
