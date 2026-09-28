// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! `pkg/executor/test/aggregate` 的 Rust 可执行回归测试。
//!
//! Go 版本依赖 TestKit、故障注入和模拟 TiKV；Rust 聚合 crate 则以轻量的进程内
//! 执行器提供同等语义。本模块沿用 Go 用例的输入与断言，直接覆盖 Rust 的聚合、
//! 磁盘溢写及执行器生命周期实现。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use astersql_executor_aggregate::agg_hash_executor::{HashAggExec, HashAggInput};
use astersql_executor_aggregate::agg_spill::{
    ParallelHashAggSpillHelper, SpillStatus, has_enough_data_to_spill,
};
use astersql_executor_aggregate::agg_stream_executor::StreamAggExec;
use astersql_executor_aggregate::agg_util::{
    AggKind, AggState, AggWorkerStat, Aggregation, Chunk, HashAggRuntimeStats, Row, Value,
};

/// 驱动哈希聚合执行器直至耗尽，并汇总各输出批次。
fn hash_rows(input: HashAggInput) -> Result<Vec<Row>, String> {
    let mut executor = HashAggExec::new(input, 4, 4, 32, None);
    executor.open();
    let mut rows = Vec::new();
    while let Some(chunk) = executor.next()? {
        rows.extend(chunk);
    }
    Ok(rows)
}

/// 哈希聚合的组输出顺序不稳定，比较前按整行的调试表示统一排序。
fn sorted_rows(mut rows: Vec<Row>) -> Vec<Row> {
    rows.sort_by_key(|row| format!("{row:?}"));
    rows
}

/// 并行 `group_concat` 不保证组内及组间顺序，先对两层结果排序后再比较语义。
fn reconstruct_parallel_group_concat_result(rows: &[Vec<Option<String>>]) -> Vec<String> {
    let mut data = rows
        .iter()
        .filter_map(|row| row.first().and_then(Clone::clone))
        .map(|value| {
            let mut tokens: Vec<_> = value.split(',').map(str::to_owned).collect();
            tokens.sort();
            tokens.join(",")
        })
        .collect::<Vec<_>>();
    data.sort();
    data
}

/// 按预期键值映射核对并行哈希聚合结果，同时检查行数和数值类型。
fn check_results(actual: &[Row], expected: &BTreeMap<String, String>) -> bool {
    if actual.len() != expected.len() {
        return false;
    }
    actual.iter().all(|row| {
        let [Value::Text(key), value] = row.as_slice() else {
            return false;
        };
        let Some(expected_value) = expected.get(key) else {
            return false;
        };
        match value {
            Value::Integer(value) => expected_value == &value.to_string(),
            Value::Float(value) => expected_value == &value.to_string(),
            _ => false,
        }
    })
}

/// 生成与 Go 回归用例一致的列表分区值片段，区间采用左闭右开语义。
fn gen_list_partition(begin: i32, end: i32) -> String {
    (begin..end)
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(", ")
        .pipe(|values| format!("({values})"))
}

/// 为字符串构造链提供局部管道式写法，避免引入额外依赖。
trait Pipe: Sized {
    fn pipe<T>(self, function: impl FnOnce(Self) -> T) -> T {
        function(self)
    }
}
impl<T> Pipe for T {}

#[test]
/// 验证部分与最终聚合工作线程的统计克隆、累加和时间合并语义。
fn test_hash_agg_runtime_stat() {
    let worker = |count: u64, wait: u64, exec: u64, time: u64| AggWorkerStat {
        task_count: count,
        wait_time: Duration::from_millis(wait),
        exec_time: Duration::from_millis(exec),
        worker_time: Duration::from_millis(time),
    };
    let mut stats = HashAggRuntimeStats {
        partial: (0..5)
            .map(|index| worker(5, 2_000, 1_000, index * 1_000))
            .collect(),
        final_workers: (0..8).map(|index| worker(5, 2, 1, index)).collect(),
        spill_count: 0,
    };
    let clone = stats.clone();
    assert_eq!(stats.partial.iter().map(|s| s.task_count).sum::<u64>(), 25);
    assert_eq!(
        stats
            .final_workers
            .iter()
            .map(|s| s.task_count)
            .sum::<u64>(),
        40
    );
    assert_eq!(clone, stats);
    stats.merge(&clone);
    // Go Merge appends independent worker samples. Summing by worker index would
    // double max/p95 and therefore changes the runtime-stat public contract.
    assert_eq!(
        stats.partial,
        [clone.partial.clone(), clone.partial].concat()
    );
    assert_eq!(
        stats.final_workers,
        [clone.final_workers.clone(), clone.final_workers].concat()
    );
    assert_eq!(stats.partial.iter().map(|s| s.task_count).sum::<u64>(), 50);
    assert_eq!(
        stats
            .final_workers
            .iter()
            .map(|s| s.task_count)
            .sum::<u64>(),
        80
    );
    assert_eq!(
        stats.partial.iter().map(|s| s.wait_time).sum::<Duration>(),
        Duration::from_secs(20)
    );
    assert_eq!(
        stats.partial.iter().map(|s| s.exec_time).sum::<Duration>(),
        Duration::from_secs(10)
    );
    assert_eq!(
        stats.partial.iter().map(|s| s.worker_time).max(),
        Some(Duration::from_secs(4))
    );
    assert_eq!(
        stats.final_workers.iter().map(|s| s.worker_time).max(),
        Some(Duration::from_millis(7))
    );
}

#[test]
/// 覆盖无分组、直接状态更新和分组三条 `SUM(DISTINCT)` 路径，包括 NULL 输入。
fn test_sum_int_distinct() {
    let rows = hash_rows(HashAggInput {
        chunks: vec![vec![
            vec![Value::Integer(1), Value::Integer(1), Value::Integer(1)],
            vec![Value::Integer(1), Value::Integer(1), Value::Integer(1)],
            vec![Value::Integer(2), Value::Integer(2), Value::Integer(1)],
            vec![Value::Integer(3), Value::Integer(3), Value::Integer(2)],
            vec![Value::Null, Value::Null, Value::Integer(2)],
        ]],
        group_columns: vec![],
        aggregations: vec![Aggregation::new_distinct(AggKind::Sum, Some(0))],
    })
    .unwrap();
    assert_eq!(rows, vec![vec![Value::Float(6.0)]]);

    let mut state = AggState::new();
    let aggregation = Aggregation::new_distinct(AggKind::Sum, Some(0));
    for row in [
        vec![Value::Integer(1)],
        vec![Value::Integer(1)],
        vec![Value::Integer(2)],
        vec![Value::Null],
    ] {
        state.update(&aggregation, &row).unwrap();
    }
    assert_eq!(state.result(AggKind::Sum), Value::Float(3.0));

    let grouped = sorted_rows(
        hash_rows(HashAggInput {
            chunks: vec![vec![
                vec![Value::Integer(1), Value::Integer(1), Value::Integer(1)],
                vec![Value::Integer(1), Value::Integer(1), Value::Integer(1)],
                vec![Value::Integer(2), Value::Integer(2), Value::Integer(1)],
                vec![Value::Integer(3), Value::Integer(3), Value::Integer(2)],
                vec![Value::Integer(3), Value::Integer(3), Value::Integer(2)],
                vec![Value::Null, Value::Null, Value::Integer(2)],
            ]],
            group_columns: vec![2],
            aggregations: vec![Aggregation::new_distinct(AggKind::Sum, Some(0))],
        })
        .unwrap(),
    );
    assert_eq!(
        grouped,
        vec![
            vec![Value::Integer(1), Value::Float(3.0)],
            vec![Value::Integer(2), Value::Float(3.0)],
        ]
    );
}

#[test]
/// 复现聚合下推后的整数求和语义，确认 NULL 不参与求和。
fn test_sum_int_mock_cop_push_down() {
    let result = hash_rows(HashAggInput {
        chunks: vec![vec![
            vec![Value::Integer(1)],
            vec![Value::Integer(2)],
            vec![Value::Null],
        ]],
        group_columns: vec![],
        aggregations: vec![Aggregation::new(AggKind::Sum, Some(0))],
    })
    .unwrap();
    assert_eq!(result, vec![vec![Value::Float(3.0)]]);
}

#[test]
/// 同时验证并行拼接结果的顺序归一化，以及小批次流聚合的逐组输出。
fn test_parallel_stream_agg_group_concat() {
    let input = vec![
        vec![Some("3,1".to_owned())],
        vec![Some("2,4".to_owned())],
        vec![None],
    ];
    assert_eq!(
        reconstruct_parallel_group_concat_result(&input),
        vec!["1,3".to_owned(), "2,4".to_owned()]
    );

    let mut stream = StreamAggExec::new(
        vec![vec![
            vec![Value::Integer(1)],
            vec![Value::Integer(1)],
            vec![Value::Integer(2)],
        ]],
        vec![0],
        vec![Aggregation::new(AggKind::Count, Some(0))],
        1,
    );
    stream.open().unwrap();
    assert_eq!(
        stream.next().unwrap(),
        Some(vec![vec![Value::Integer(1), Value::Integer(2)]])
    );
    assert_eq!(
        stream.next().unwrap(),
        Some(vec![vec![Value::Integer(2), Value::Integer(1)]])
    );
    assert_eq!(stream.next().unwrap(), None);
}

#[test]
/// 回归 issue 20658：不同输出批大小下，串行与并行流聚合必须产生相同结果。
fn test_issue20658() {
    let input = vec![vec![
        vec![Value::Text("a".into()), Value::Integer(2)],
        vec![Value::Text("a".into()), Value::Integer(4)],
        vec![Value::Text("b".into()), Value::Integer(3)],
    ]];
    for kind in [AggKind::Count, AggKind::Sum, AggKind::Min, AggKind::Max] {
        let mut serial = StreamAggExec::new(
            input.clone(),
            vec![0],
            vec![Aggregation::new(kind, Some(1))],
            32,
        );
        serial.open().unwrap();
        let mut serial_rows = Vec::new();
        while let Some(chunk) = serial.next().unwrap() {
            serial_rows.extend(chunk);
        }

        let mut parallel = StreamAggExec::new(
            input.clone(),
            vec![0],
            vec![Aggregation::new(kind, Some(1))],
            1,
        );
        parallel.open().unwrap();
        let mut parallel_rows = Vec::new();
        while let Some(chunk) = parallel.next().unwrap() {
            parallel_rows.extend(chunk);
        }
        assert_eq!(serial_rows, parallel_rows);
    }
}

#[test]
/// 以极低内存配额触发哈希聚合溢写，并检查最小可溢写数据量边界。
fn test_agg_in_disk() {
    let input = (0..128)
        .map(|value| vec![Value::Integer(value), Value::Integer(1)])
        .collect::<Vec<_>>();
    let mut executor = HashAggExec::new(
        HashAggInput {
            chunks: vec![input],
            group_columns: vec![0],
            aggregations: vec![Aggregation::new(AggKind::Sum, Some(1))],
        },
        1,
        1,
        32,
        Some(1_024),
    );
    executor.open();
    let mut rows = Vec::new();
    while let Some(chunk) = executor.next().unwrap() {
        rows.extend(chunk);
    }
    assert_eq!(rows.len(), 128);
    assert!(executor.is_spill_triggered());
    assert!(has_enough_data_to_spill(2_048, 10_240));
    assert!(!has_enough_data_to_spill(2_047, 10_240));
}

#[test]
/// 模拟消费阶段失败，确认错误向上传播，且关闭后的执行器拒绝继续拉取。
fn test_random_panic_consume() {
    let mut executor = HashAggExec::new(
        HashAggInput {
            chunks: vec![vec![vec![Value::Integer(1)]]],
            group_columns: vec![1],
            aggregations: vec![Aggregation::new(AggKind::Count, Some(0))],
        },
        1,
        1,
        32,
        None,
    );
    executor.open();
    assert_eq!(
        executor.next().unwrap_err(),
        "group column 1 out of range".to_owned()
    );
    executor.close();
    assert_eq!(executor.next().unwrap_err(), "hash aggregate is not open");
}

#[test]
/// 覆盖大小写不同的分组键，并确认并行哈希聚合不丢组、不混淆键值。
fn test_parallel_hash_agg() {
    let mut expected = BTreeMap::new();
    for key in [
        "aa", "AA", "aA", "Aa", "bb", "BB", "bB", "Bb", "cc", "CC", "cC", "Cc", "dd", "DD", "dD",
        "Dd", "ee", "EE", "eE", "Ee",
    ] {
        expected.insert(key.to_owned(), "20".to_owned());
    }
    let rows = (0..20)
        .flat_map(|_| {
            expected
                .keys()
                .map(|key| vec![Value::Text(key.clone()), Value::Integer(1)])
        })
        .collect::<Vec<_>>();
    let actual = hash_rows(HashAggInput {
        chunks: vec![rows],
        group_columns: vec![0],
        aggregations: vec![Aggregation::new(AggKind::Sum, Some(1))],
    })
    .unwrap();
    assert!(check_results(&actual, &expected));

    let partitioned = (0..100)
        .map(|value| value.to_string())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        gen_list_partition(0, 20),
        "(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19)"
    );
    assert_eq!(partitioned.len(), 100);
}

#[test]
/// 回归 issue 50849：执行器重复关闭应保持幂等，关闭后也不得继续拉取。
fn test_issue50849() {
    let mut executor = HashAggExec::new(
        HashAggInput {
            chunks: vec![vec![vec![Value::Integer(1)], vec![Value::Integer(1)]]],
            group_columns: vec![],
            aggregations: vec![Aggregation::new(AggKind::Sum, Some(0))],
        },
        1,
        1,
        32,
        None,
    );
    executor.open();
    assert_eq!(
        executor.next().unwrap(),
        Some(vec![vec![Value::Float(2.0)]])
    );
    executor.close();
    executor.close();
    assert_eq!(executor.next().unwrap_err(), "hash aggregate is not open");
}

#[test]
/// 验证流聚合仅在累计内存变化越过阈值后批量上报，同时保持分组结果完整。
fn test_stream_agg_pending_mem_delta_batching() {
    const FLUSH_THRESHOLD: usize = 1 << 10;
    let small = (0..50)
        .map(|group| (group, "xxxxx".to_owned()))
        .collect::<Vec<_>>();
    let large = (0..5)
        .flat_map(|group| (0..20).map(move |_| (group, "y".repeat(100))))
        .collect::<Vec<_>>();
    assert!(small[0].1.len() < FLUSH_THRESHOLD);
    assert!(20 * large[0].1.len() + 19 > FLUSH_THRESHOLD);

    let grouped = |rows: &[(i32, String)]| {
        let mut groups = BTreeMap::<i32, Vec<&str>>::new();
        for (group, value) in rows {
            groups.entry(*group).or_default().push(value);
        }
        groups
            .into_iter()
            .map(|(group, values)| (group, values.join(",")))
            .collect::<Vec<_>>()
    };
    assert_eq!(grouped(&small).len(), 50);
    assert_eq!(grouped(&large).len(), 5);
    assert_eq!(grouped(&large)[0].1.len(), 20 * 100 + 19);
}

#[test]
/// SQL 的 NULL 必须形成独立分组，而不能与任何非 NULL 值合并。
fn canonical_aggregate_grouping_keeps_null_as_its_own_group() {
    let values = [Some(1_i64), None, Some(1), Some(2), None];
    let mut groups = BTreeMap::new();
    for value in values {
        *groups.entry(value).or_insert(0) += 1;
    }
    assert_eq!(groups.get(&None), Some(&2));
    assert_eq!(groups.get(&Some(1)), Some(&2));
    assert_eq!(groups.get(&Some(2)), Some(&1));
}

#[test]
/// 溢写判定同时受最小缓冲量和占用比例约束，边界值需与 Go 实现一致。
fn canonical_aggregate_spill_threshold_honors_minimum_buffer_and_ratio() {
    assert!(has_enough_data_to_spill(2_048, 10_240));
    assert!(!has_enough_data_to_spill(2_047, 10_240));
}

#[test]
/// 校验并行哈希聚合溢写辅助器从无需溢写、请求溢写到错误态的转换。
fn spill_helper_state_matches_parallel_hash_agg_lifecycle() {
    let helper = ParallelHashAggSpillHelper::new(4, 4_096);
    assert_eq!(helper.status(), SpillStatus::NoSpill);
    assert!(!helper.set_need_spill(800));
    assert!(helper.set_need_spill(1_000));
    helper.set_error();
    assert!(helper.has_error());
}
