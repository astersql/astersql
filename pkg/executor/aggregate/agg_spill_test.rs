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

#[test]
fn spilling_creates_a_disk_file_and_releases_memory_partitions() {
    let helper = ParallelHashAggSpillHelper::new(4, 1);
    let mut map = AggMap::new();
    let aggregation = Aggregation::new_distinct(AggKind::Count, Some(0));
    let mut state = AggState::new();
    state
        .update(&aggregation, &vec![Value::Text("payload".repeat(4096))])
        .unwrap();
    map.insert(vec![42], (vec![Value::Integer(42)], vec![state]));
    helper.spill(map).unwrap();
    assert!(
        helper.disk_bytes() > 0,
        "spill must write the partial states to disk"
    );
    assert_eq!(
        helper.buffered_groups(),
        0,
        "spilled groups must not remain in memory"
    );
}

#[test]
fn check_chunk_spill_uses_nonempty_and_used_bytes_or_full_rows() {
    use super::agg_hash_partial_worker::{SPILL_CHUNK_SIZE_THRESHOLD, check_chunk_spill};
    use astersql_util_serialization::types;
    let ty = types::NewFieldType(253);
    let baseline = astersql_util_chunk::NewChunkWithCapacity(vec![ty.clone()], 2).UsedMemoryUsage();
    let column_count = (SPILL_CHUNK_SIZE_THRESHOLD / baseline + 1) as usize;
    let wide = astersql_util_chunk::NewChunkWithCapacity(vec![ty.clone(); column_count], 2);
    assert!(wide.UsedMemoryUsage() >= SPILL_CHUNK_SIZE_THRESHOLD);
    assert_eq!(wide.NumRows(), 0);
    assert!(!check_chunk_spill(&wide));
    let mut large = astersql_util_chunk::NewChunkWithCapacity(vec![ty.clone()], 2);
    large.AppendBytes(0, &vec![0; SPILL_CHUNK_SIZE_THRESHOLD as usize]);
    assert!(!large.IsFull());
    assert!(check_chunk_spill(&large));
    large.Reset();
    large.AppendBytes(0, b"small");
    assert!(large.MemoryUsage() >= SPILL_CHUNK_SIZE_THRESHOLD);
    assert!(!check_chunk_spill(&large));
    let mut full = astersql_util_chunk::NewChunkWithCapacity(vec![ty], 1);
    full.AppendBytes(0, b"value");
    assert!(full.UsedMemoryUsage() < SPILL_CHUNK_SIZE_THRESHOLD);
    assert!(full.IsFull());
    assert!(check_chunk_spill(&full));
}

#[test]
fn distinct_growth_in_one_group_spills_and_merges_overlapping_workers() {
    let chunks = (0..4)
        .map(|batch| {
            (0..200)
                .map(|value| {
                    vec![
                        Value::Integer(0),
                        Value::Text(format!("{value:04}-{}", "x".repeat(32))),
                        Value::Integer((value + batch) % 200),
                    ]
                })
                .collect()
        })
        .collect();
    let input = HashAggInput {
        chunks,
        group_columns: vec![0],
        aggregations: vec![
            Aggregation::new_distinct(AggKind::Count, Some(1)),
            Aggregation::new_distinct(AggKind::Sum, Some(2)),
        ],
    };
    let mut executor = HashAggExec::new(input, 4, 3, 2, Some(10_000));
    executor.open();
    assert_eq!(
        executor.next().unwrap().unwrap(),
        vec![vec![
            Value::Integer(0),
            Value::Integer(200),
            Value::Float(19_900.0)
        ]]
    );
    assert!(executor.is_spill_triggered());
    assert!(executor.next().unwrap().is_none());
    executor.close();
}

#[test]
fn typed_worker_spills_one_large_row_and_flushes_each_partition_before_reuse() {
    use super::agg_hash_partial_worker::{PartialResultSpill, TypedPartialResultMap};
    use astersql_executor_aggfuncs::func_count_distinct::CountDistinctString;
    use astersql_executor_aggfuncs::{PartialResult, StateSerializer};
    let function = StateSerializer {
        ordinal: 0,
        template: CountDistinctString::default(),
    };
    let mut storage = PartialResultSpill::new(4, 1, 1024);
    let mut map = TypedPartialResultMap::new();
    for index in 0..12_u8 {
        let mut state = CountDistinctString::default();
        state.update([Some(vec![index; 1_100_000]), Some(vec![0, index, 255])]);
        map.insert(vec![index], vec![Box::new(state) as PartialResult]);
    }
    storage.spill_maps(vec![map], &[&function]).unwrap();
    assert!(storage.disk_bytes() > 13_000_000);
    assert_eq!(storage.chunk_counts().iter().sum::<usize>(), 12);
    let mut seen = 0;
    for partition in (0..4).rev() {
        let (maps, memory) = storage.restore_partition(partition, &[&function]).unwrap();
        for map in maps {
            for (key, states) in map {
                assert_eq!(murmur3_sum32(&key) as usize % 4, partition);
                let state = states[0].downcast_ref::<CountDistinctString>().unwrap();
                assert_eq!(state.count(), 2);
                seen += 1;
            }
        }
        if memory > 0 {
            assert!(memory >= 1_100_000);
        }
    }
    assert_eq!(seen, 12);
    assert!(storage.is_empty());
}

#[test]
fn typed_spill_io_error_and_serializer_panic_cleanup_files_and_allow_retry() {
    use super::agg_hash_partial_worker::{PartialResultSpill, TypedPartialResultMap};
    use astersql_executor_aggfuncs::func_count_distinct::CountDistinctInt;
    use astersql_executor_aggfuncs::{PartialResult, Serializer, StateSerializer};
    struct BrokenWriter;
    impl astersql_util_chunk::io::Writer for BrokenWriter {
        fn Write(&mut self, _: &[u8]) -> Result<usize, astersql_util_chunk::errors::Error> {
            Err(astersql_util_chunk::errors::New(
                "injected aggregate IO failure",
            ))
        }
    }
    impl astersql_util_chunk::io::WriteCloser for BrokenWriter {
        fn Close(&mut self) -> Result<(), astersql_util_chunk::errors::Error> {
            Ok(())
        }
    }
    let map = || {
        let mut state = CountDistinctInt::default();
        state.update([Some(1), Some(-2)]);
        TypedPartialResultMap::from([(vec![42], vec![Box::new(state) as PartialResult])])
    };
    let function = StateSerializer {
        ordinal: 0,
        template: CountDistinctInt::default(),
    };
    let mut storage = PartialResultSpill::new(1, 1, 1);
    storage.files[0].initDiskFile().unwrap();
    let path = storage.files[0].dataFile.file.as_ref().unwrap().Name();
    storage.files[0].dataFile.writer = Some(Box::new(BrokenWriter));
    assert!(
        storage
            .spill_maps(vec![map()], &[&function])
            .unwrap_err()
            .contains("injected aggregate IO failure")
    );
    assert!(storage.is_empty());
    drop(storage);
    assert!(!std::path::Path::new(&path).exists());

    struct PanicSerializer;
    impl Serializer for PanicSerializer {
        fn serialize_partial_result(
            &self,
            _: &PartialResult,
            _: &mut astersql_util_chunk::Chunk,
            _: &mut astersql_executor_aggfuncs::SerializeHelper,
        ) {
            panic!("injected serializer panic");
        }
        fn deserialize_partial_result(
            &self,
            _: &astersql_util_chunk::Chunk,
        ) -> (Vec<PartialResult>, i64) {
            panic!("injected restore panic");
        }
    }
    let mut storage = PartialResultSpill::new(1, 1, 1);
    assert!(
        storage
            .spill_maps(vec![map()], &[&PanicSerializer])
            .unwrap_err()
            .contains("injected serializer panic")
    );
    storage.spill_maps(vec![map()], &[&function]).unwrap();
    let path = storage.files[0].dataFile.file.as_ref().unwrap().Name();
    assert!(std::path::Path::new(&path).exists());
    assert!(storage.restore_partition(0, &[&PanicSerializer]).is_err());
    assert!(!std::path::Path::new(&path).exists());
    assert!(storage.is_empty());
}

#[test]
fn parallel_typed_distinct_matrix_matches_no_spill_results_for_fifty_thousand_rows() {
    use super::agg_hash_partial_worker::{PartialResultSpill, TypedPartialResultMap};
    use a::builder::{AggImplementation as A, BuiltAggFunc, ValueKind as K};
    use a::func_avg::DistinctFloatAvg;
    use a::func_count_distinct::{
        ApproxCountDistinct, CountDistinctMulti, CountDistinctReal, DistinctValue,
        update_distinct_real,
    };
    use a::func_first_row::FirstRow;
    use a::func_sum::DistinctFloatSum;
    use a::func_varpop::DistinctVariance;
    use astersql_executor_aggfuncs as a;
    fn functions() -> Vec<Box<dyn a::Serializer>> {
        [
            A::FirstRow(K::String),
            A::CountOriginalDistinct(K::Float64),
            A::CountOriginalDistinctMulti,
            A::SumOriginalDistinctFloat64,
            A::AvgOriginalDistinctFloat64,
            A::VarPopOriginalDistinct,
            A::StddevSampOriginalDistinct,
            A::ApproxCountDistinctOriginal,
        ]
        .into_iter()
        .enumerate()
        .map(|(ordinal, implementation)| {
            BuiltAggFunc {
                implementation,
                ordinal,
                argument_count: 1,
                order_by: vec![],
                separator: None,
                max_len: None,
                default_value: None,
            }
            .spill_function()
            .unwrap()
            .0
        })
        .collect()
    }
    fn map(worker: usize, rows: usize) -> TypedPartialResultMap {
        let mut groups = TypedPartialResultMap::new();
        for i in 0..rows {
            let group = (i % 7) as u8;
            let key = vec![group];
            let states = groups.entry(key).or_insert_with(|| {
                vec![
                    Box::new(FirstRow::<String>::default()),
                    Box::new(CountDistinctReal::default()),
                    Box::new(CountDistinctMulti::default()),
                    Box::new(DistinctFloatSum::default()),
                    Box::new(DistinctFloatAvg::default()),
                    Box::new(DistinctVariance::default()),
                    Box::new(DistinctVariance::default()),
                    Box::new(ApproxCountDistinct::default()),
                ]
            });
            let value = ((i + worker * 113) % 1000) as f64 / 10.0;
            states[0]
                .downcast_mut::<FirstRow<String>>()
                .unwrap()
                .update([Some(format!("group-{group}"))]);
            update_distinct_real(
                states[1].downcast_mut::<CountDistinctReal>().unwrap(),
                [Some(value)],
            );
            states[2]
                .downcast_mut::<CountDistinctMulti>()
                .unwrap()
                .update([vec![
                    Some(DistinctValue::String(vec![group])),
                    Some(DistinctValue::Real(value)),
                ]])
                .unwrap();
            states[3]
                .downcast_mut::<DistinctFloatSum>()
                .unwrap()
                .update([Some(value)]);
            states[4]
                .downcast_mut::<DistinctFloatAvg>()
                .unwrap()
                .update([Some(value)]);
            states[5]
                .downcast_mut::<DistinctVariance>()
                .unwrap()
                .update([Some(value)]);
            states[6]
                .downcast_mut::<DistinctVariance>()
                .unwrap()
                .update([Some(value)]);
            states[7]
                .downcast_mut::<ApproxCountDistinct>()
                .unwrap()
                .insert_hash64(value.to_bits());
        }
        groups
    }
    fn merge(into: &mut TypedPartialResultMap, input: TypedPartialResultMap) {
        for (key, states) in input {
            if let Some(destination) = into.get_mut(&key) {
                for (source, destination) in states.iter().zip(destination.iter_mut()) {
                    a::merge_spilled_partial_result(source, destination).unwrap();
                }
            } else {
                into.insert(key, states);
            }
        }
    }
    let mut baseline = TypedPartialResultMap::new();
    for worker in 0..5 {
        merge(&mut baseline, map(worker, 10_000));
    }
    let storage = std::sync::Mutex::new(PartialResultSpill::new(256, 8, 256));
    std::thread::scope(|scope| {
        let mut threads = Vec::new();
        for worker in 0..5 {
            let storage = &storage;
            threads.push(scope.spawn(move || {
                let functions = functions();
                let references = functions.iter().map(|f| f.as_ref()).collect::<Vec<_>>();
                let input = map(worker, 10_000);
                if worker == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                storage
                    .lock()
                    .unwrap()
                    .spill_maps(vec![input], &references)
                    .unwrap();
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }
    });
    let functions = functions();
    let references = functions.iter().map(|f| f.as_ref()).collect::<Vec<_>>();
    let mut storage = storage.into_inner().unwrap();
    assert!(storage.disk_bytes() > 0);
    let paths = storage
        .files
        .iter()
        .filter_map(|f| f.dataFile.file.as_ref().map(|f| f.Name()))
        .collect::<Vec<_>>();
    let mut actual = TypedPartialResultMap::new();
    for partition in (0..256).rev() {
        let (map, _) = storage
            .restore_merged_partition(partition, &references)
            .unwrap();
        merge(&mut actual, map);
    }
    assert_eq!(actual.len(), baseline.len());
    for (key, expected) in baseline {
        let values = actual.remove(&key).unwrap();
        assert_eq!(
            values[0]
                .downcast_ref::<FirstRow<String>>()
                .unwrap()
                .value(),
            expected[0]
                .downcast_ref::<FirstRow<String>>()
                .unwrap()
                .value()
        );
        assert_eq!(
            values[1]
                .downcast_ref::<CountDistinctReal>()
                .unwrap()
                .count(),
            expected[1]
                .downcast_ref::<CountDistinctReal>()
                .unwrap()
                .count()
        );
        assert_eq!(
            values[2]
                .downcast_ref::<CountDistinctMulti>()
                .unwrap()
                .count(),
            expected[2]
                .downcast_ref::<CountDistinctMulti>()
                .unwrap()
                .count()
        );
        let floats = |states: &[a::PartialResult]| {
            [
                states[3]
                    .downcast_ref::<DistinctFloatSum>()
                    .unwrap()
                    .value()
                    .unwrap(),
                states[4]
                    .downcast_ref::<DistinctFloatAvg>()
                    .unwrap()
                    .result()
                    .unwrap(),
                states[5]
                    .downcast_ref::<DistinctVariance>()
                    .unwrap()
                    .population_variance()
                    .unwrap(),
                states[6]
                    .downcast_ref::<DistinctVariance>()
                    .unwrap()
                    .sample_variance()
                    .unwrap()
                    .sqrt(),
            ]
        };
        for (value, expected) in floats(&values).into_iter().zip(floats(&expected)) {
            assert!((value - expected).abs() <= 1e-6_f64.max(expected.abs() * 1e-12));
        }
        assert_eq!(
            values[7]
                .downcast_ref::<ApproxCountDistinct>()
                .unwrap()
                .estimate(),
            expected[7]
                .downcast_ref::<ApproxCountDistinct>()
                .unwrap()
                .estimate()
        );
    }
    assert!(actual.is_empty());
    assert!(storage.is_empty());
    for path in paths {
        assert!(!std::path::Path::new(&path).exists());
    }
}
