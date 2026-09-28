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

// Left Outer Join probe 单元测试。

use crate::base_join_probe::{HashJoinContext, new_join_probe};
use crate::joiner::{JoinType, Joiner, Predicate, Row};
use crate::row_table_builder::Value;
use std::sync::Arc;

/// 构造单列 Int 行，便于断言连接结果。
fn row(value: i64) -> Row {
    vec![Value::Int(value)]
}

/// 构造 Left Outer Join probe；`right_as_build` 控制 build 侧方向。
fn left_outer_probe(
    build: Vec<Row>,
    right_as_build: bool,
    condition: Vec<Predicate>,
) -> Box<dyn crate::base_join_probe::Probe> {
    left_outer_probe_with(build, right_as_build, condition, vec![0], vec![0], 32)
}

fn left_outer_probe_with(
    build: Vec<Row>,
    right_as_build: bool,
    condition: Vec<Predicate>,
    build_key_indices: Vec<usize>,
    probe_key_indices: Vec<usize>,
    max_chunk_size: usize,
) -> Box<dyn crate::base_join_probe::Probe> {
    let joiner = Joiner::new(
        JoinType::LeftOuter,
        false,
        row(-1),
        condition,
        None,
        false,
        max_chunk_size,
    )
    .unwrap();
    let context = HashJoinContext::new(
        build,
        build_key_indices,
        probe_key_indices,
        joiner,
        right_as_build,
        true,
        max_chunk_size,
    );
    new_join_probe(context, 0, JoinType::LeftOuter, right_as_build, false).unwrap()
}

/// 匹配成功输出左右列；未匹配左行补默认 inner（-1）。
#[test]
fn left_outer_emits_matches_and_default_inner_rows() {
    let mut probe = left_outer_probe(vec![row(1), row(3)], true, vec![]);
    probe.set_chunk_for_probe(vec![row(1), row(2)]).unwrap();
    assert_eq!(
        probe.probe().rows,
        vec![
            vec![Value::Int(1), Value::Int(1)],
            vec![Value::Int(2), Value::Int(-1)],
        ]
    );
}

/// other condition 拒绝匹配时，outer 行仍保留并以默认 inner 输出。
#[test]
fn left_outer_other_condition_preserves_rejected_outer_row() {
    let condition: Predicate = Arc::new(|joined| {
        Ok(Some(
            matches!((&joined[0], &joined[1]), (Value::Int(left), Value::Int(right)) if left > right),
        ))
    });
    let mut probe = left_outer_probe(vec![row(2)], true, vec![condition]);
    probe.set_chunk_for_probe(vec![row(2)]).unwrap();
    assert_eq!(
        probe.probe().rows,
        vec![vec![Value::Int(2), Value::Int(-1)]]
    );
}

/// Outer 侧作为 build 时，probe 后扫描未匹配 build 行并补默认 probe 列。
#[test]
fn left_outer_with_outer_build_scans_unmatched_build_rows() {
    let mut probe = left_outer_probe(vec![row(1), row(2)], false, vec![]);
    probe.set_chunk_for_probe(vec![row(1), row(4)]).unwrap();
    assert_eq!(probe.probe().rows, vec![vec![Value::Int(1), Value::Int(1)]]);
    assert!(probe.need_scan_row_table());
    probe.init_for_scan_row_table();
    assert_eq!(
        probe.scan_row_table().rows,
        vec![vec![Value::Int(2), Value::Int(-1)]]
    );
    assert!(probe.is_scan_row_table_done());
}

/// Spill 剩余 probe 行后，恢复 chunk 仍按 Left Outer 语义输出。
#[test]
fn left_outer_spills_remaining_and_accepts_restored_chunk() {
    let mut probe = left_outer_probe(vec![row(7)], true, vec![]);
    probe.set_chunk_for_probe(vec![row(7), row(8)]).unwrap();
    assert_eq!(
        probe.spill_remaining_probe_chunks(),
        vec![vec![row(7), row(8)]]
    );
    probe.set_restored_chunk_for_probe(vec![row(8)]).unwrap();
    assert_eq!(
        probe.probe().rows,
        vec![vec![Value::Int(8), Value::Int(-1)]]
    );
}

#[test]
fn left_outer_requires_every_composite_key_and_never_matches_null() {
    let mut probe = left_outer_probe_with(
        vec![
            vec![Value::Int(1), Value::Text("a".into())],
            vec![Value::Int(1), Value::Text("b".into())],
            vec![Value::Null, Value::Text("a".into())],
        ],
        true,
        vec![],
        vec![0, 1],
        vec![0, 1],
        32,
    );
    probe
        .set_chunk_for_probe(vec![
            vec![Value::Int(1), Value::Text("b".into())],
            vec![Value::Int(1), Value::Text("c".into())],
            vec![Value::Null, Value::Text("a".into())],
        ])
        .unwrap();

    assert_eq!(
        probe.probe().rows,
        vec![
            vec![
                Value::Int(1),
                Value::Text("b".into()),
                Value::Int(1),
                Value::Text("b".into()),
            ],
            vec![Value::Int(1), Value::Text("c".into()), Value::Int(-1),],
            vec![Value::Null, Value::Text("a".into()), Value::Int(-1)],
        ]
    );
}

#[test]
fn left_outer_resumes_duplicate_matches_at_chunk_boundary() {
    let mut probe = left_outer_probe_with(
        vec![row(1), row(1), row(1)],
        true,
        vec![],
        vec![0],
        vec![0],
        2,
    );
    probe.set_chunk_for_probe(vec![row(1), row(2)]).unwrap();

    assert_eq!(probe.probe().rows.len(), 2);
    assert!(!probe.is_current_chunk_probe_done());
    assert_eq!(
        probe.probe().rows,
        vec![
            vec![Value::Int(1), Value::Int(1)],
            vec![Value::Int(2), Value::Int(-1)],
        ]
    );
    assert!(probe.is_current_chunk_probe_done());
}

#[test]
fn left_outer_propagates_other_condition_error_without_fabricating_a_miss() {
    let condition: Predicate = Arc::new(|_| Err("condition failed".into()));
    let mut probe = left_outer_probe(vec![row(1)], true, vec![condition]);
    probe.set_chunk_for_probe(vec![row(1)]).unwrap();

    let result = probe.probe();
    assert!(result.rows.is_empty());
    assert_eq!(result.error.as_deref(), Some("condition failed"));
}

#[test]
fn left_outer_reset_clears_partial_probe_and_scan_state() {
    let mut probe = left_outer_probe_with(vec![row(1), row(2)], false, vec![], vec![0], vec![0], 1);
    probe.set_chunk_for_probe(vec![row(1)]).unwrap();
    assert_eq!(probe.probe().rows, vec![vec![Value::Int(1), Value::Int(1)]]);
    probe.init_for_scan_row_table();
    assert_eq!(
        probe.scan_row_table().rows,
        vec![vec![Value::Int(2), Value::Int(-1)]]
    );

    probe.reset_probe();
    assert!(probe.is_current_chunk_probe_done());
    probe.set_chunk_for_probe(vec![row(9)]).unwrap();
    assert!(probe.probe().rows.is_empty());
    probe.init_for_scan_row_table();
    assert_eq!(
        probe.scan_row_table().rows,
        vec![vec![Value::Int(2), Value::Int(-1)]]
    );
}
