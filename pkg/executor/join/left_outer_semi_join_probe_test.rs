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

// Left Outer Semi Join probe 单元测试。
//
// 验证匹配标记列（true/false/NULL）、重复 build key 只输出一行 outer，
// 以及 spill/恢复后标记语义不变。

use crate::base_join_probe::{HashJoinContext, Probe, new_join_probe};
use crate::joiner::{JoinType, Joiner, Predicate, Row};
use crate::row_table_builder::Value;
use std::sync::Arc;

/// 构造单列 Int 行。
fn row(value: i64) -> Row {
    vec![Value::Int(value)]
}

/// 构造 Left Outer Semi Join probe（右表为 build）。
fn marker_probe(build: Vec<Row>, conditions: Vec<Predicate>) -> Box<dyn Probe> {
    let joiner = Joiner::new(
        JoinType::LeftOuterSemi,
        false,
        vec![],
        conditions,
        None,
        false,
        32,
    )
    .unwrap();
    let context = HashJoinContext::new(build, vec![0], vec![0], joiner, true, true, 32);
    new_join_probe(context, 0, JoinType::LeftOuterSemi, true, false).unwrap()
}

fn null_aware_anti_marker_probe(build: Vec<Row>) -> Box<dyn Probe> {
    let joiner = Joiner::new(
        JoinType::AntiLeftOuterSemi,
        false,
        vec![],
        vec![],
        None,
        true,
        32,
    )
    .unwrap();
    let context = HashJoinContext::new(build, vec![0], vec![0], joiner, true, true, 32);
    new_join_probe(context, 0, JoinType::AntiLeftOuterSemi, true, true).unwrap()
}

/// 匹配行追加 true 标记，未匹配追加 false。
#[test]
fn left_outer_semi_appends_true_and_false_markers() {
    let mut probe = marker_probe(vec![row(1), row(3)], vec![]);
    probe.set_chunk_for_probe(vec![row(1), row(2)]).unwrap();
    assert_eq!(
        probe.probe().rows,
        vec![
            vec![Value::Int(1), Value::Bool(true)],
            vec![Value::Int(2), Value::Bool(false)],
        ]
    );
}

/// other condition 返回 NULL 时，标记列为 NULL（三值逻辑）。
#[test]
fn left_outer_semi_condition_null_appends_null_marker() {
    let condition: Predicate = Arc::new(|_| Ok(None));
    let mut probe = marker_probe(vec![row(1)], vec![condition]);
    probe.set_chunk_for_probe(vec![row(1)]).unwrap();
    assert_eq!(probe.probe().rows, vec![vec![Value::Int(1), Value::Null]]);
}

/// 多个相同 build key 只为每个 outer 行输出一次标记结果。
#[test]
fn left_outer_semi_duplicate_build_keys_emit_one_outer_row() {
    let mut probe = marker_probe(vec![row(5), row(5), row(5)], vec![]);
    probe.set_chunk_for_probe(vec![row(5)]).unwrap();
    assert_eq!(
        probe.probe().rows,
        vec![vec![Value::Int(5), Value::Bool(true)]]
    );
}

/// Spill 后恢复的 probe 行仍正确输出 false 标记。
#[test]
fn left_outer_semi_restored_and_spilled_rows_keep_markers() {
    let mut probe = marker_probe(vec![row(9)], vec![]);
    probe.set_chunk_for_probe(vec![row(8), row(9)]).unwrap();
    assert_eq!(
        probe.spill_remaining_probe_chunks(),
        vec![vec![row(8), row(9)]]
    );
    probe.set_restored_chunk_for_probe(vec![row(8)]).unwrap();
    assert_eq!(
        probe.probe().rows,
        vec![vec![Value::Int(8), Value::Bool(false)]]
    );
}

#[test]
fn null_aware_anti_marker_reports_unknown_for_null_key_set() {
    let mut probe = null_aware_anti_marker_probe(vec![row(1), vec![Value::Null]]);
    probe
        .set_chunk_for_probe(vec![row(2), vec![Value::Null]])
        .unwrap();

    assert_eq!(
        probe.probe().rows,
        vec![
            vec![Value::Int(2), Value::Null],
            vec![Value::Null, Value::Null],
        ]
    );
}

#[test]
/// A normal left outer semi join treats NULL keys as unmatched and emits a false marker.
fn left_outer_semi_null_key_is_not_a_match() {
    let mut probe = marker_probe(vec![vec![Value::Null], row(1)], vec![]);
    probe
        .set_chunk_for_probe(vec![vec![Value::Null], row(1), row(2)])
        .unwrap();
    assert_eq!(
        probe.probe().rows,
        [
            vec![Value::Null, Value::Bool(false)],
            vec![Value::Int(1), Value::Bool(true)],
            vec![Value::Int(2), Value::Bool(false)],
        ]
    );
}

#[test]
#[should_panic(expected = "should not reach here")]
fn left_outer_semi_init_for_scan_row_table_panics_like_go() {
    let mut probe = marker_probe(vec![], vec![]);
    probe.init_for_scan_row_table();
}

#[test]
#[should_panic(expected = "should not reach here")]
fn left_outer_semi_is_scan_row_table_done_panics_like_go() {
    let probe = marker_probe(vec![], vec![]);
    let _ = probe.is_scan_row_table_done();
}
