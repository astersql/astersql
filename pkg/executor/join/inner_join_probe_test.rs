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

// Hash Join V2 Inner Join 探测路径的单元测试。
//
// 覆盖：重复 key 匹配与 hash miss、other condition 过滤与 chunk 容量切分、
// restore/spill 后剩余探测行保留，以及探测 key 列越界拒绝。


use crate::base_join_probe::{HashJoinContext, Probe, new_join_probe};
use crate::joiner::{JoinType, Joiner, Predicate, Row};
use crate::row_table_builder::Value;
use std::sync::Arc;

/// 将整型切片转为测试用探测/构建行。
fn row(values: &[i64]) -> Row {
    values.iter().copied().map(Value::Int).collect()
}

/// 构造以列 0 为 join key 的 Inner Join Probe，可注入 other condition 与 chunk 容量。
fn inner_probe(build: Vec<Row>, condition: Vec<Predicate>, max_chunk: usize) -> Box<dyn Probe> {
    let joiner = Joiner::new(
        JoinType::Inner,
        false,
        Vec::new(),
        condition,
        None,
        false,
        max_chunk,
    )
    .unwrap();
    let context = HashJoinContext::new(build, vec![0], vec![0], joiner, true, true, max_chunk);
    new_join_probe(context, 0, JoinType::Inner, true, false).unwrap()
}

/// 重复 key 应产出多行匹配；hash miss（key=2）不产生结果。
#[test]
fn inner_join_probe_matches_duplicates_and_rejects_hash_misses() {
    let mut probe = inner_probe(
        vec![row(&[1, 10]), row(&[1, 11]), row(&[3, 30])],
        Vec::new(),
        32,
    );
    probe
        .set_chunk_for_probe(vec![row(&[1, 100]), row(&[2, 200])])
        .unwrap();
    assert_eq!(
        probe.probe().rows,
        [row(&[1, 100, 1, 10]), row(&[1, 100, 1, 11]),]
    );
    assert!(probe.is_current_chunk_probe_done());
}

#[test]
fn inner_join_probe_respects_chunk_capacity_with_duplicate_matches() {
    let mut probe = inner_probe(vec![row(&[1, 10]), row(&[1, 11])], Vec::new(), 1);
    probe.set_chunk_for_probe(vec![row(&[1, 100])]).unwrap();

    assert_eq!(probe.probe().rows, [row(&[1, 100, 1, 10])]);
    assert!(!probe.is_current_chunk_probe_done());
    assert_eq!(probe.probe().rows, [row(&[1, 100, 1, 11])]);
    assert!(probe.is_current_chunk_probe_done());
}

/// other condition 过滤后仅保留满足谓词的行；max_chunk=1 时分多次 probe 输出。
#[test]
fn inner_join_probe_applies_other_condition_and_chunk_capacity() {
    let condition: Predicate = Arc::new(|joined| match (&joined[3], &joined[1]) {
        (Value::Int(build), Value::Int(probe)) => Ok(Some(build < probe)),
        _ => Ok(None),
    });
    let mut probe = inner_probe(
        vec![row(&[1, 10]), row(&[1, 20]), row(&[2, 5])],
        vec![condition],
        1,
    );
    probe
        .set_chunk_for_probe(vec![row(&[1, 15]), row(&[2, 9])])
        .unwrap();
    assert_eq!(probe.probe().rows, [row(&[1, 15, 1, 10])]);
    assert_eq!(probe.probe().rows, [row(&[2, 9, 2, 5])]);
}

/// restore 路径探测一行后，spill 应保留剩余探测行。
#[test]
fn inner_join_restored_and_spill_paths_preserve_remaining_rows() {
    let mut probe = inner_probe(vec![row(&[1])], Vec::new(), 1);
    probe
        .set_restored_chunk_for_probe(vec![row(&[1]), row(&[2]), row(&[3])])
        .unwrap();
    assert_eq!(probe.probe().rows, [row(&[1, 1])]);
    assert_eq!(
        probe.spill_remaining_probe_chunks(),
        [vec![row(&[2]), row(&[3])]]
    );
}

#[test]
#[should_panic(expected = "should not reach here")]
fn inner_join_scan_row_table_panics_like_go() {
    inner_probe(Vec::new(), Vec::new(), 1).scan_row_table();
}

#[test]
#[should_panic(expected = "should not reach here")]
fn inner_join_init_for_scan_row_table_panics_like_go() {
    inner_probe(Vec::new(), Vec::new(), 1).init_for_scan_row_table();
}

#[test]
#[should_panic(expected = "should not reach here")]
fn inner_join_is_scan_row_table_done_panics_like_go() {
    inner_probe(Vec::new(), Vec::new(), 1).is_scan_row_table_done();
}

/// 探测侧 key 列下标越界时应拒绝 set_chunk_for_probe。
#[test]
fn inner_join_probe_rejects_out_of_range_probe_key() {
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
    let context = HashJoinContext::new(vec![row(&[1])], vec![0], vec![1], joiner, true, true, 8);
    let mut probe = new_join_probe(context, 0, JoinType::Inner, true, false).unwrap();
    assert!(probe.set_chunk_for_probe(vec![row(&[1])]).is_err());
}
