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

// Semi Join（半连接）探测逻辑的单元测试。
//
// 覆盖：仅输出命中的 probe 行、重复 build 键不重复外表行、other condition / NULL
// 结果拒绝候选，以及左侧 build 时只扫描已使用的 build 行。

use crate::base_join_probe::{HashJoinContext, Probe, new_join_probe};
use crate::joiner::{JoinType, Joiner, Predicate, Row};
use crate::row_table_builder::Value;
use std::sync::Arc;

/// 构造两列整数行，用作连接键与载荷。
fn row(key: i64, payload: i64) -> Row {
    vec![Value::Int(key), Value::Int(payload)]
}

/// 构造 Semi Join 探测器：`right_as_build` 决定 build 侧，`conditions` 为 other condition。
fn semi_probe(build: Vec<Row>, right_as_build: bool, conditions: Vec<Predicate>) -> Box<dyn Probe> {
    let joiner = Joiner::new(
        JoinType::Semi,
        false,
        vec![],
        conditions,
        Some([vec![0, 1], vec![0, 1]]),
        false,
        32,
    )
    .unwrap();
    let context = HashJoinContext::new(build, vec![0], vec![0], joiner, right_as_build, true, 32);
    new_join_probe(context, 2, JoinType::Semi, right_as_build, false).unwrap()
}

/// 右侧 build：只输出在 build 侧存在匹配的 probe 行。
#[test]
fn semi_join_returns_only_matched_probe_rows() {
    let mut probe = semi_probe(vec![row(1, 10), row(3, 30)], true, vec![]);
    probe
        .set_chunk_for_probe(vec![row(1, 100), row(2, 200)])
        .unwrap();
    // 键 1 命中，键 2 未命中，半连接只保留键 1。
    assert_eq!(probe.probe().rows, vec![row(1, 100)]);
}

/// 同一 build 键多行时，外表行仍只输出一次（半连接去重语义）。
#[test]
fn semi_join_duplicate_build_keys_do_not_duplicate_outer_row() {
    let mut probe = semi_probe(vec![row(5, 1), row(5, 2)], true, vec![]);
    probe.set_chunk_for_probe(vec![row(5, 9)]).unwrap();
    assert_eq!(probe.probe().rows, vec![row(5, 9)]);
}

/// other condition 为假或 NULL（三值逻辑）时应拒绝该候选匹配。
#[test]
fn semi_join_condition_and_null_result_reject_candidates() {
    // 载荷列比较：left > right 为真；相等返回 NULL；其余为假。
    let condition: Predicate = Arc::new(|joined| match (&joined[1], &joined[3]) {
        (Value::Int(left), Value::Int(right)) if left > right => Ok(Some(true)),
        (Value::Int(left), Value::Int(right)) if left == right => Ok(None),
        _ => Ok(Some(false)),
    });
    let mut probe = semi_probe(vec![row(1, 10), row(2, 20)], true, vec![condition]);
    probe
        .set_chunk_for_probe(vec![row(1, 11), row(2, 20)])
        .unwrap();
    // 11>10 命中；20==20 为 NULL，半连接不接受。
    assert_eq!(probe.probe().rows, vec![row(1, 11)]);
}

/// 左侧 build：探测阶段不产出行，随后扫描 row table 只输出已标记使用的 build 行。
#[test]
fn semi_join_with_left_build_scans_only_used_build_rows() {
    let mut probe = semi_probe(vec![row(1, 10), row(2, 20)], false, vec![]);
    probe
        .set_restored_chunk_for_probe(vec![row(1, 100), row(3, 300)])
        .unwrap();
    // 左侧 build 时 probe() 只打 used 标记，结果行延后到 scan_row_table。
    assert!(probe.probe().rows.is_empty());
    assert!(probe.need_scan_row_table());
    probe.init_for_scan_row_table();
    assert_eq!(probe.scan_row_table().rows, vec![row(1, 10)]);
}

#[test]
/// NULL keys never satisfy ordinary semi-join equality, even when both sides are NULL.
fn semi_join_null_keys_are_not_equal() {
    let mut probe = semi_probe(
        vec![vec![Value::Null, Value::Int(10)], row(1, 20)],
        true,
        vec![],
    );
    probe
        .set_chunk_for_probe(vec![vec![Value::Null, Value::Int(100)], row(1, 200)])
        .unwrap();
    assert_eq!(probe.probe().rows, [row(1, 200)]);
}

#[test]
/// A left-build semi join marks only matching build rows before the row-table scan.
fn semi_join_left_build_skips_unmatched_duplicate_rows() {
    let mut probe = semi_probe(vec![row(4, 40), row(4, 41), row(5, 50)], false, vec![]);
    probe.set_chunk_for_probe(vec![row(4, 400)]).unwrap();
    assert!(probe.probe().rows.is_empty());
    probe.init_for_scan_row_table();
    assert_eq!(probe.scan_row_table().rows, [row(4, 40), row(4, 41)]);
}

#[test]
#[should_panic(expected = "should not reach here")]
fn semi_join_right_build_init_for_scan_row_table_panics_like_go() {
    let mut probe = semi_probe(vec![row(1, 10)], true, vec![]);
    probe.init_for_scan_row_table();
}

#[test]
#[should_panic(expected = "should not reach here")]
fn semi_join_right_build_is_scan_row_table_done_panics_like_go() {
    let probe = semi_probe(vec![row(1, 10)], true, vec![]);
    probe.is_scan_row_table_done();
}

#[test]
#[should_panic(expected = "should not reach here")]
fn semi_join_right_build_scan_row_table_panics_like_go() {
    let mut probe = semi_probe(vec![row(1, 10)], true, vec![]);
    probe.scan_row_table();
}
