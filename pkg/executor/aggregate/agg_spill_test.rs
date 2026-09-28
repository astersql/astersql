// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 并行 Hash 聚合 spill（落盘）辅助器的单元测试。
//
// 验证内存超限触发 spill、按分区写出/恢复 AggState，以及
// `has_enough_data_to_spill` 阈值判定。

use std::collections::BTreeMap;

use super::agg_hash_executor::{HashAggExec, HashAggInput};
use super::agg_hash_partial_worker::murmur3_sum32;
use super::agg_spill::{ParallelHashAggSpillHelper, SpillStatus, has_enough_data_to_spill};
use super::agg_stream_executor::StreamAggExec;
use super::agg_util::{AggKind, AggMap, AggState, Aggregation, Chunk, Value};

#[test]
/// 构造 SUM 聚合态 → 触发 spill → 逐分区 restore，断言条数与空盘状态。
fn aggregate_spill_partitions_and_restores_real_states() {
    let aggregation = Aggregation::new(AggKind::Sum, Some(0));
    let mut state = AggState::new();
    state
        .update(&aggregation, &vec![Value::Integer(7)])
        .unwrap();
    let mut map = AggMap::new();
    map.insert(vec![1], (vec![Value::Integer(1)], vec![state]));
    let helper = ParallelHashAggSpillHelper::new(4, 4096);
    // 内存占用达到 limit 时置 NeedSpill，随后 spill 写入分区。
    assert!(helper.set_need_spill(4096));
    assert_eq!(helper.spill(map).unwrap(), 1);
    assert_eq!(helper.status(), SpillStatus::Triggered);
    let mut restored = 0;
    // 按分区游标依次恢复，累计 map 条目数应等于 spill 前的 1 条。
    while let Some(partition) = helper.next_partition() {
        restored += helper
            .restore_partition(partition)
            .unwrap()
            .into_iter()
            .map(|map| map.len())
            .sum::<usize>();
    }
    assert_eq!(restored, 1);
    assert!(helper.is_empty());
    assert!(has_enough_data_to_spill(4096, 20_000));
    assert!(!has_enough_data_to_spill(999, 20_000));
    let _: BTreeMap<Vec<u8>, _> = AggMap::new();
}

#[test]
fn spill_uses_go_partition_order_and_worker_hash() {
    let helper = ParallelHashAggSpillHelper::new(4, 100);
    let mut input = AggMap::new();
    for key in [b"alpha".to_vec(), b"beta".to_vec(), b"gamma".to_vec()] {
        input.insert(key, (Vec::new(), Vec::new()));
    }

    helper.spill(input).unwrap();

    for expected_partition in (0..4).rev() {
        assert_eq!(helper.next_partition(), Some(expected_partition));
        for map in helper.restore_partition(expected_partition).unwrap() {
            assert!(
                map.keys()
                    .all(|key| { murmur3_sum32(key) as usize % 4 == expected_partition })
            );
        }
    }
    assert_eq!(helper.next_partition(), None);
}

#[test]
fn hash_aggregate_matches_grouped_results_and_chunking() {
    let input = HashAggInput {
        chunks: vec![
            vec![
                vec![Value::Text("a".into()), Value::Integer(2)],
                vec![Value::Text("b".into()), Value::Integer(5)],
            ],
            vec![vec![Value::Text("a".into()), Value::Integer(3)]],
        ],
        group_columns: vec![0],
        aggregations: vec![
            Aggregation::new(AggKind::Sum, Some(1)),
            Aggregation::new(AggKind::Count, Some(1)),
            Aggregation::new(AggKind::Min, Some(1)),
            Aggregation::new(AggKind::Max, Some(1)),
        ],
    };
    let mut exec = HashAggExec::new(input, 2, 3, 1, None);
    exec.open();
    let mut rows = Vec::new();
    while let Some(chunk) = exec.next().unwrap() {
        rows.extend(chunk);
    }
    rows.sort_by(|left, right| match (&left[0], &right[0]) {
        (Value::Text(left), Value::Text(right)) => left.cmp(right),
        _ => std::cmp::Ordering::Equal,
    });
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0], Value::Text("a".into()));
    assert_eq!(rows[0][1], Value::Float(5.0));
    assert_eq!(rows[0][2], Value::Integer(2));
    assert_eq!(rows[0][3], Value::Integer(2));
    assert_eq!(rows[0][4], Value::Integer(3));
    assert_eq!(rows[1][0], Value::Text("b".into()));
    assert_eq!(rows[1][1], Value::Float(5.0));
    assert_eq!(rows[1][2], Value::Integer(1));
}

#[test]
fn hash_aggregate_empty_input_returns_default_group() {
    let input = HashAggInput {
        chunks: vec![Chunk::new()],
        group_columns: Vec::new(),
        aggregations: vec![
            Aggregation::new(AggKind::Count, None),
            Aggregation::new(AggKind::Sum, Some(0)),
        ],
    };
    let mut exec = HashAggExec::new(input, 1, 1, 8, None);
    exec.open();
    let result = exec.next().unwrap().unwrap();
    assert_eq!(result, vec![vec![Value::Integer(0), Value::Null]]);
    assert!(exec.next().unwrap().is_none());
}

#[test]
fn stream_aggregate_preserves_sorted_group_boundaries() {
    let input = vec![vec![
        vec![Value::Text("a".into()), Value::Integer(1)],
        vec![Value::Text("a".into()), Value::Integer(4)],
        vec![Value::Text("b".into()), Value::Integer(2)],
    ]];
    let mut exec = StreamAggExec::new(
        input,
        vec![0],
        vec![Aggregation::new(AggKind::Sum, Some(1))],
        1,
    );
    exec.open().unwrap();
    assert_eq!(
        exec.next().unwrap().unwrap(),
        vec![vec![Value::Text("a".into()), Value::Float(5.0)]]
    );
    assert_eq!(
        exec.next().unwrap().unwrap(),
        vec![vec![Value::Text("b".into()), Value::Float(2.0)]]
    );
    assert!(exec.next().unwrap().is_none());
}
