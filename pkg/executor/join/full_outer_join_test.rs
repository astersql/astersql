// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use crate::hash_join_base::HashJoinContextBase;
use crate::hash_join_v1::{HashJoinCtxV1, HashJoinV1Exec};
use crate::joiner::{JoinType, Joiner, Predicate, Row};
use crate::row_table_builder::{Chunk, Value};
use std::sync::Arc;

fn full_outer_executor(
    build_is_left: bool,
    build_chunks: Vec<Chunk>,
    probe_chunks: Vec<Chunk>,
    conditions: Vec<Predicate>,
    build_filter: Vec<Predicate>,
    probe_filter: Vec<Predicate>,
) -> HashJoinV1Exec {
    let context = HashJoinCtxV1 {
        base: HashJoinContextBase::default(),
        join_type: JoinType::FullOuter,
        build_key_indices: vec![0],
        probe_key_indices: vec![0],
        null_aware: false,
        build_side_is_outer: false,
        concurrency: 1,
        max_chunk_size: 32,
    };
    let (build_type, build_outer_is_right, probe_type, probe_outer_is_right) = if build_is_left {
        (JoinType::LeftOuter, false, JoinType::RightOuter, true)
    } else {
        (JoinType::RightOuter, true, JoinType::LeftOuter, false)
    };
    let build_joiner = Joiner::new(
        build_type,
        build_outer_is_right,
        vec![Value::Null, Value::Null],
        conditions,
        None,
        false,
        32,
    )
    .expect("build-side outer joiner");
    let probe_joiner = Joiner::new(
        probe_type,
        probe_outer_is_right,
        vec![Value::Null, Value::Null],
        Vec::new(),
        None,
        false,
        32,
    )
    .expect("probe-side outer joiner");
    HashJoinV1Exec::new_full_outer(
        context,
        build_joiner,
        probe_joiner,
        build_filter,
        probe_filter,
        build_chunks,
        probe_chunks,
    )
    .expect("build full outer hash join")
}

fn sorted(mut rows: Vec<Row>) -> Vec<Row> {
    rows.sort_by_key(|row| format!("{row:?}"));
    rows
}

#[test]
fn full_outer_join_preserves_unmatched_rows_from_both_sides() {
    let mut executor = full_outer_executor(
        true,
        vec![vec![
            vec![Value::Int(1), Value::Text("left-one".into())],
            vec![Value::Int(2), Value::Text("left-two".into())],
        ]],
        vec![vec![
            vec![Value::Int(2), Value::Text("right-two".into())],
            vec![Value::Int(3), Value::Text("right-three".into())],
        ]],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let rows = executor.execute_all().expect("execute hash join");
    let expected = vec![
        vec![
            Value::Int(1),
            Value::Text("left-one".into()),
            Value::Null,
            Value::Null,
        ],
        vec![
            Value::Int(2),
            Value::Text("left-two".into()),
            Value::Int(2),
            Value::Text("right-two".into()),
        ],
        vec![
            Value::Null,
            Value::Null,
            Value::Int(3),
            Value::Text("right-three".into()),
        ],
    ];
    assert_eq!(sorted(rows), sorted(expected));
}

#[test]
fn full_outer_join_marks_only_build_rows_that_pass_other_conditions() {
    let greater: Predicate = Arc::new(|joined| match (&joined[1], &joined[3]) {
        (Value::Int(left), Value::Int(right)) => Ok(Some(left > right)),
        _ => Ok(None),
    });
    let mut executor = full_outer_executor(
        true,
        vec![vec![
            vec![Value::Int(1), Value::Int(1)],
            vec![Value::Int(1), Value::Int(10)],
        ]],
        vec![vec![vec![Value::Int(1), Value::Int(5)]]],
        vec![greater],
        Vec::new(),
        Vec::new(),
    );
    assert_eq!(
        sorted(executor.execute_all().expect("execute hash join")),
        sorted(vec![
            vec![Value::Int(1), Value::Int(1), Value::Null, Value::Null],
            vec![Value::Int(1), Value::Int(10), Value::Int(1), Value::Int(5),],
        ])
    );
}

#[test]
fn full_outer_join_preserves_rows_rejected_by_each_side_filter() {
    let positive: Predicate = Arc::new(|row| match row[1] {
        Value::Int(value) => Ok(Some(value > 0)),
        _ => Ok(None),
    });
    let mut executor = full_outer_executor(
        true,
        vec![vec![
            vec![Value::Int(1), Value::Int(-1)],
            vec![Value::Int(2), Value::Int(2)],
        ]],
        vec![vec![
            vec![Value::Int(1), Value::Int(1)],
            vec![Value::Int(2), Value::Int(-2)],
        ]],
        Vec::new(),
        vec![positive.clone()],
        vec![positive],
    );
    assert_eq!(
        sorted(executor.execute_all().expect("execute hash join")),
        sorted(vec![
            vec![Value::Int(1), Value::Int(-1), Value::Null, Value::Null],
            vec![Value::Null, Value::Null, Value::Int(1), Value::Int(1)],
            vec![Value::Int(2), Value::Int(2), Value::Null, Value::Null],
            vec![Value::Null, Value::Null, Value::Int(2), Value::Int(-2)],
        ])
    );
}

#[test]
fn full_outer_join_supports_right_side_build_and_spill() {
    let mut executor = full_outer_executor(
        false,
        vec![vec![
            vec![Value::Int(2), Value::Text("right-two".into())],
            vec![Value::Int(3), Value::Text("right-three".into())],
        ]],
        vec![vec![
            vec![Value::Int(1), Value::Text("left-one".into())],
            vec![Value::Int(2), Value::Text("left-two".into())],
        ]],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    executor.SetMemoryLimit(Some(1));
    assert_eq!(
        sorted(executor.execute_all().expect("execute spilled hash join")),
        sorted(vec![
            vec![
                Value::Int(1),
                Value::Text("left-one".into()),
                Value::Null,
                Value::Null
            ],
            vec![
                Value::Int(2),
                Value::Text("left-two".into()),
                Value::Int(2),
                Value::Text("right-two".into()),
            ],
            vec![
                Value::Null,
                Value::Null,
                Value::Int(3),
                Value::Text("right-three".into())
            ],
        ])
    );
    assert!(executor.IsSpillTriggered());
    assert!(executor.DiskBytes() > 0);
}
