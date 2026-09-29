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

// Right Outer Join probe 单元测试。
//
// 覆盖匹配输出、未匹配时补默认左列、outer build 扫描未匹配右行，
// 以及 other condition 拒绝时仍保留 outer 行。

use crate::base_join_probe::{HashJoinContext, Probe, new_join_probe};
use crate::hash_join_v2::{HashJoinCtxV2, HashJoinV2Exec};
use crate::joiner::{JoinType, Joiner, Predicate, Row};
use crate::row_table_builder::{Chunk, Value};
use std::sync::Arc;

/// 构造单列 Int 行。
fn row(value: i64) -> Row {
    vec![Value::Int(value)]
}

/// 构造 Right Outer Join probe；默认左列为 -1。
fn right_outer_probe(
    build: Vec<Row>,
    right_as_build: bool,
    conditions: Vec<Predicate>,
) -> Box<dyn Probe> {
    right_outer_probe_with_capacity(build, right_as_build, conditions, 32)
}

fn right_outer_probe_with_capacity(
    build: Vec<Row>,
    right_as_build: bool,
    conditions: Vec<Predicate>,
    max_chunk_size: usize,
) -> Box<dyn Probe> {
    right_outer_probe_with(
        build,
        right_as_build,
        conditions,
        vec![0],
        vec![0],
        None,
        max_chunk_size,
    )
}

fn right_outer_probe_with(
    build: Vec<Row>,
    right_as_build: bool,
    conditions: Vec<Predicate>,
    build_key_indices: Vec<usize>,
    probe_key_indices: Vec<usize>,
    children_used: Option<[Vec<usize>; 2]>,
    max_chunk_size: usize,
) -> Box<dyn Probe> {
    let joiner = Joiner::new(
        JoinType::RightOuter,
        true,
        row(-1),
        conditions,
        children_used,
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
    new_join_probe(context, 1, JoinType::RightOuter, right_as_build, false).unwrap()
}

/// 匹配成功拼左右列；未匹配右行补默认左列（-1）。
#[test]
fn right_outer_emits_matches_and_default_left_rows() {
    let mut probe = right_outer_probe(vec![row(1), row(3)], false, vec![]);
    probe.set_chunk_for_probe(vec![row(1), row(2)]).unwrap();
    assert_eq!(
        probe.probe().rows,
        vec![
            vec![Value::Int(1), Value::Int(1)],
            vec![Value::Int(-1), Value::Int(2)],
        ]
    );
}

/// 右表为 outer build 时，扫描未匹配 build 行并补默认左列。
#[test]
fn right_outer_with_outer_build_scans_unmatched_right_rows() {
    let mut probe = right_outer_probe(vec![row(1), row(2)], true, vec![]);
    probe.set_chunk_for_probe(vec![row(1), row(4)]).unwrap();
    assert_eq!(probe.probe().rows, vec![vec![Value::Int(1), Value::Int(1)]]);
    probe.init_for_scan_row_table();
    assert_eq!(
        probe.scan_row_table().rows,
        vec![vec![Value::Int(-1), Value::Int(2)]]
    );
}

/// other condition 拒绝匹配时，outer（右）行仍保留并以默认左列输出。
#[test]
fn right_outer_condition_rejection_keeps_outer_row() {
    let condition: Predicate = Arc::new(|joined| {
        Ok(Some(
            matches!((&joined[0], &joined[1]), (Value::Int(left), Value::Int(right)) if left < right),
        ))
    });
    let mut probe = right_outer_probe(vec![row(4)], false, vec![condition]);
    probe.set_chunk_for_probe(vec![row(4)]).unwrap();
    assert_eq!(
        probe.probe().rows,
        vec![vec![Value::Int(-1), Value::Int(4)]]
    );
}

#[test]
fn right_outer_probe_respects_capacity_with_duplicate_inner_rows() {
    let mut probe = right_outer_probe_with_capacity(vec![row(1), row(1)], false, vec![], 1);
    probe.set_chunk_for_probe(vec![row(1)]).unwrap();

    assert_eq!(probe.probe().rows, [vec![Value::Int(1), Value::Int(1)]]);
    assert!(!probe.is_current_chunk_probe_done());
    assert_eq!(probe.probe().rows, [vec![Value::Int(1), Value::Int(1)]]);
    assert!(probe.is_current_chunk_probe_done());
}

#[test]
fn right_outer_requires_every_composite_key_and_never_matches_null() {
    let mut probe = right_outer_probe_with(
        vec![
            vec![Value::Int(1), Value::Text("a".into())],
            vec![Value::Int(1), Value::Text("b".into())],
            vec![Value::Null, Value::Text("a".into())],
        ],
        false,
        vec![],
        vec![0, 1],
        vec![0, 1],
        None,
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
            vec![Value::Int(-1), Value::Int(1), Value::Text("c".into())],
            vec![Value::Int(-1), Value::Null, Value::Text("a".into())],
        ]
    );
}

#[test]
fn right_outer_supported_key_types_and_int_uint_compatibility_match_go() {
    let values = vec![
        Value::Bool(true),
        Value::Int(-7),
        Value::UInt(7),
        Value::Float(7.25),
        Value::Bytes(vec![0, 1, 2]),
        Value::Text("seven".into()),
    ];
    for value in values {
        let build = vec![vec![value.clone()]];
        let mut probe =
            right_outer_probe_with(build.clone(), false, vec![], vec![0], vec![0], None, 32);
        probe.set_chunk_for_probe(build.clone()).unwrap();
        assert_eq!(probe.probe().rows, vec![vec![value.clone(), value]]);
    }

    let mut signed_unsigned = right_outer_probe_with(
        vec![vec![Value::Int(7)]],
        false,
        vec![],
        vec![0],
        vec![0],
        None,
        32,
    );
    signed_unsigned
        .set_chunk_for_probe(vec![vec![Value::UInt(7)]])
        .unwrap();
    assert_eq!(
        signed_unsigned.probe().rows,
        vec![vec![Value::Int(7), Value::UInt(7)]]
    );
}

#[test]
fn right_outer_used_column_matrix_preserves_go_projection_contract() {
    for (left_used, right_used, expected) in [
        (
            vec![0, 1],
            vec![],
            vec![Value::Int(1), Value::Text("left".into())],
        ),
        (
            vec![],
            vec![0, 1],
            vec![Value::Int(1), Value::Text("right".into())],
        ),
        (vec![], vec![], vec![]),
        (
            vec![1],
            vec![1],
            vec![Value::Text("left".into()), Value::Text("right".into())],
        ),
    ] {
        let mut probe = right_outer_probe_with(
            vec![vec![Value::Int(1), Value::Text("left".into())]],
            false,
            vec![],
            vec![0],
            vec![0],
            Some([left_used, right_used]),
            32,
        );
        probe
            .set_chunk_for_probe(vec![vec![Value::Int(1), Value::Text("right".into())]])
            .unwrap();
        assert_eq!(probe.probe().rows, vec![expected]);
    }
}

#[test]
fn right_outer_propagates_other_condition_error_without_fabricating_a_miss() {
    let condition: Predicate = Arc::new(|_| Err("condition failed".into()));
    let mut probe = right_outer_probe(vec![row(1)], false, vec![condition]);
    probe.set_chunk_for_probe(vec![row(1)]).unwrap();

    let result = probe.probe();
    assert!(result.rows.is_empty());
    assert_eq!(result.error.as_deref(), Some("condition failed"));
}

#[test]
fn right_outer_spill_restore_and_reset_preserve_outer_semantics() {
    let mut probe = right_outer_probe_with_capacity(vec![row(1), row(2)], true, vec![], 1);
    probe.set_chunk_for_probe(vec![row(1), row(9)]).unwrap();
    assert_eq!(probe.probe().rows, vec![vec![Value::Int(1), Value::Int(1)]]);
    assert_eq!(probe.spill_remaining_probe_chunks(), vec![vec![row(9)]]);

    probe.set_restored_chunk_for_probe(vec![row(9)]).unwrap();
    assert!(probe.probe().rows.is_empty());
    probe.init_for_scan_row_table();
    assert_eq!(
        probe.scan_row_table().rows,
        vec![vec![Value::Int(-1), Value::Int(2)]]
    );
    assert!(probe.is_scan_row_table_done());

    probe.reset_probe();
    assert!(probe.is_current_chunk_probe_done());
    probe.set_chunk_for_probe(vec![row(2)]).unwrap();
    assert_eq!(probe.probe().rows, vec![vec![Value::Int(2), Value::Int(2)]]);
    probe.init_for_scan_row_table();
    assert!(probe.scan_row_table().rows.is_empty());
}

#[test]
fn hash_join_v2_right_outer_keeps_only_right_side_unmatched_rows() {
    let context = HashJoinCtxV2::new(
        JoinType::RightOuter,
        vec![0],
        vec![0],
        true,
        false,
        1,
        32,
        None,
    )
    .unwrap();
    let joiner = Joiner::new(JoinType::RightOuter, true, row(-1), vec![], None, false, 32).unwrap();
    let build: Chunk = vec![row(1), row(2)];
    let probe: Chunk = vec![row(1), row(3)];
    let mut exec = HashJoinV2Exec::new(context, joiner, vec![build], vec![probe]).unwrap();

    assert_eq!(
        exec.execute_all().unwrap(),
        vec![
            vec![Value::Int(1), Value::Int(1)],
            vec![Value::Int(-1), Value::Int(2)]
        ]
    );
}
