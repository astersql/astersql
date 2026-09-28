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
// 以及 other condition 拒绝时仍保留 outer 行。块注释内保留 Go 版测试矩阵草稿。

/*
// right outer join probe 如何用 nested loop 生成期望结果，并覆盖 build side、过滤器、nullable key 和 selection vector。

// genRightOuterJoinResult 对应 Go 的 nested loop 期望结果生成器。
pub fn gen_right_outer_join_result(
    t: &mut testing::T,
    sess_ctx: sessionctx::Context,
    right_filter: Option<expression::CNFExprs>,
    left_chunks: Vec<chunk::Chunk>,
    right_chunks: Vec<chunk::Chunk>,
    left_key_index: Vec<i32>,
    right_key_index: Vec<i32>,
    left_types: Vec<types::FieldType>,
    right_types: Vec<types::FieldType>,
    left_key_types: Vec<types::FieldType>,
    right_key_types: Vec<types::FieldType>,
    left_used_columns: Vec<i32>,
    right_used_columns: Vec<i32>,
    other_conditions: Option<expression::CNFExprs>,
    result_types: Vec<types::FieldType>,
) -> Vec<chunk::Chunk> {
    let mut filter_vector: Vec<bool> = Vec::new();
    let mut return_chks: Vec<chunk::Chunk> = Vec::with_capacity(1);
    let mut result_chk = chunk::New(&result_types, sess_ctx.GetSessionVars().MaxChunkSize, sess_ctx.GetSessionVars().MaxChunkSize);
    let mut shallow_row_types = left_types.clone();
    shallow_row_types.extend(right_types.clone());
    let mut shallow_row = chunk::MutRowFromTypes(shallow_row_types);
    let empty_row = chunk::Row::default();

    // right outer join 以右表为输出基准；右侧过滤失败时也要构造左空右实的结果行。
    for right_chunk in right_chunks {
        if let Some(filter) = &right_filter {
            filter_vector = expression::VectorizedFilter(
                sess_ctx.GetExprCtx().GetEvalCtx(),
                sess_ctx.GetSessionVars().EnableVectorizedExpression,
                filter,
                chunk::NewIterator4Chunk(&right_chunk),
                filter_vector,
            ).expect("Go require.NoError: right filter evaluation");
        }
        for right_index in 0..right_chunk.NumRows() {
            let filter_index = right_chunk.Sel().map(|sel| sel[right_index]).unwrap_or(right_index);
            if right_filter.is_some() && !filter_vector[filter_index] {
                appendToResultChk(empty_row, right_chunk.GetRow(right_index), &left_used_columns, &right_used_columns, &mut result_chk);
                flush_full_chunk(&mut return_chks, &mut result_chk, &result_types, &sess_ctx);
                continue;
            }

            let right_row = right_chunk.GetRow(right_index);
            let mut has_at_least_one_match = false;
            for left_chunk in &left_chunks {
                for left_index in 0..left_chunk.NumRows() {
                    flush_full_chunk(&mut return_chks, &mut result_chk, &result_types, &sess_ctx);
                    let left_row = left_chunk.GetRow(left_index);
                    let mut valid = !containsNullKey(left_row, &left_key_index)
                        && !containsNullKey(right_row, &right_key_index);
                    if valid {
                        // Go 使用 codec.EqualChunkRow 做类型感知比较，保留错误断言语义。
                        valid = codec::EqualChunkRow(
                            sess_ctx.GetSessionVars().StmtCtx.TypeCtx(),
                            left_row,
                            &left_key_types,
                            &left_key_index,
                            right_row,
                            &right_key_types,
                            &right_key_index,
                        ).expect("Go require.NoError: key compare");
                    }
                    if valid {
                        if let Some(cond) = &other_conditions {
                            valid = evalOtherCondition(&sess_ctx, left_row, right_row, &mut shallow_row, cond)
                                .expect("Go require.NoError: other condition");
                        }
                    }
                    if valid {
                        has_at_least_one_match = true;
                        appendToResultChk(left_row, right_row, &left_used_columns, &right_used_columns, &mut result_chk);
                    }
                }
            }
            if !has_at_least_one_match {
                // right outer join 未匹配时输出空左行；这正是 right probe 和 inner probe 的关键差异。
                appendToResultChk(empty_row, right_row, &left_used_columns, &right_used_columns, &mut result_chk);
                flush_full_chunk(&mut return_chks, &mut result_chk, &result_types, &sess_ctx);
            }
        }
    }
    if result_chk.NumRows() > 0 {
        return_chks.push(result_chk);
    }
    return_chks
}

#[test]
pub fn test_right_outer_join_probe_basic() {
    // todo test nullable type after builder support nullable type
    let (tiny_tp, int_tp, uint_tp, string_tp) = basic_probe_types();
    let l_types = vec![int_tp.clone(), string_tp.clone(), uint_tp.clone(), string_tp.clone(), tiny_tp.clone()];
    let mut r_types = vec![int_tp.clone(), string_tp.clone(), uint_tp.clone(), string_tp.clone(), tiny_tp.clone()];
    r_types.extend(retTypes);
    let mut r_types1 = vec![uint_tp.clone(), string_tp.clone(), int_tp.clone(), string_tp.clone(), tiny_tp.clone()];
    r_types1.extend(r_types1.clone());
    let right_as_build_side = [true, false];
    let partition_number = 3;
    let simple_filter = createSimpleFilter(&mut testing::T::new());
    let has_filter = [false, true];
    let test_cases = vec![
        // normal case
        test_case(vec![0], vec![0], vec![int_tp.clone()], vec![int_tp.clone()], l_types.clone(), r_types.clone(), None, None, None, None, None),
        // rightUsed is empty
        test_case(vec![0], vec![0], vec![int_tp.clone()], vec![int_tp.clone()], l_types.clone(), r_types.clone(), Some(vec![0, 1, 2, 3]), Some(vec![]), None, None, None),
        // leftUsed is empty
        test_case(vec![0], vec![0], vec![int_tp.clone()], vec![int_tp.clone()], l_types.clone(), r_types.clone(), Some(vec![]), Some(vec![0, 1, 2, 3]), None, None, None),
        // both left/right Used are empty
        test_case(vec![0], vec![0], vec![int_tp.clone()], vec![int_tp.clone()], l_types.clone(), r_types.clone(), Some(vec![]), Some(vec![]), None, None, None),
        // both left/right used is part of all columns
        test_case(vec![0], vec![0], vec![int_tp.clone()], vec![int_tp.clone()], l_types.clone(), r_types.clone(), Some(vec![0, 2]), Some(vec![1, 3]), None, None, None),
        // int join uint
        test_case(vec![0], vec![0], vec![int_tp.clone()], vec![uint_tp.clone()], l_types.clone(), r_types1.clone(), Some(vec![0, 1, 2, 3]), Some(vec![0, 1, 2, 3]), None, None, None),
        // multiple join keys
        test_case(vec![0, 1], vec![0, 1], vec![int_tp.clone(), string_tp.clone()], vec![int_tp.clone(), string_tp.clone()], l_types.clone(), r_types.clone(), Some(vec![0, 1, 2, 3]), Some(vec![0, 1, 2, 3]), None, None, None),
    ];

    for tc in test_cases {
        for right_build in right_as_build_side {
            for test_filter in has_filter {
                let right_filter = test_filter.then(|| simple_filter.clone());
                // 每个 case 同时跑 not-null 和 nullable 版本，保持 Go 对 nullable key 的覆盖矩阵。
                testJoinProbe(&mut testing::T::new(), false, tc.leftKeyIndex.clone(), tc.rightKeyIndex.clone(), tc.leftKeyTypes.clone(), tc.rightKeyTypes.clone(), tc.leftTypes.clone(), tc.rightTypes.clone(), right_build, tc.leftUsed.clone(), tc.rightUsed.clone(), tc.leftUsedByOtherCondition.clone(), tc.rightUsedByOtherCondition.clone(), None, right_filter.clone(), tc.otherCondition.clone(), partition_number, base::RightOuterJoin, 200);
                testJoinProbe(&mut testing::T::new(), false, tc.leftKeyIndex.clone(), tc.rightKeyIndex.clone(), toNullableTypes(tc.leftKeyTypes.clone()), toNullableTypes(tc.rightKeyTypes.clone()), toNullableTypes(tc.leftTypes.clone()), toNullableTypes(tc.rightTypes.clone()), right_build, tc.leftUsed.clone(), tc.rightUsed.clone(), tc.leftUsedByOtherCondition.clone(), tc.rightUsedByOtherCondition.clone(), None, right_filter, tc.otherCondition.clone(), partition_number, base::RightOuterJoin, 200);
            }
        }
    }
}

#[test]
pub fn test_right_outer_join_probe_all_join_keys() {
    // 覆盖所有可作为 join key 的类型，包括 enum/set/bit/json/decimal/time/blob。
    let all = all_join_key_field_types();
    let l_types = all.clone();
    let r_types = all.clone();
    let l_used: Vec<i32> = (0..l_types.len() as i32).collect();
    let r_used = l_used.clone();
    let right_as_build_side = [true, false];
    for i in 0..l_types.len() {
        let l_key_types = vec![l_types[i].clone()];
        let r_key_types = vec![r_types[i].clone()];
        for right_as_build in right_as_build_side {
            testJoinProbe(&mut testing::T::new(), false, vec![i as i32], vec![i as i32], l_key_types.clone(), r_key_types.clone(), l_types.clone(), r_types.clone(), right_as_build, l_used.clone(), r_used.clone(), None, None, None, None, None, 3, base::RightOuterJoin, 100);
            testJoinProbe(&mut testing::T::new(), false, vec![i as i32], vec![i as i32], toNullableTypes(l_key_types.clone()), toNullableTypes(r_key_types.clone()), toNullableTypes(l_types.clone()), toNullableTypes(r_types.clone()), right_as_build, l_used.clone(), r_used.clone(), None, None, None, None, None, 3, base::RightOuterJoin, 100);
        }
    }
    // 组合键按 Go 顺序覆盖 fixed/variable、inlined/not-inlined 四类序列化路径。
    let composed_cases = vec![(1, 2), (1, 17), (1, 13), (1, 14)];
    for (a, b) in composed_cases {
        for right_as_build in right_as_build_side {
            let l_key_types = vec![l_types[a].clone(), l_types[b].clone()];
            let r_key_types = vec![r_types[a].clone(), r_types[b].clone()];
            testJoinProbe(&mut testing::T::new(), false, vec![a as i32, b as i32], vec![a as i32, b as i32], l_key_types.clone(), r_key_types.clone(), l_types.clone(), r_types.clone(), right_as_build, l_used.clone(), r_used.clone(), None, None, None, None, None, 3, base::RightOuterJoin, 100);
            testJoinProbe(&mut testing::T::new(), false, vec![a as i32, b as i32], vec![a as i32, b as i32], toNullableTypes(l_key_types), toNullableTypes(r_key_types), toNullableTypes(l_types.clone()), toNullableTypes(r_types.clone()), right_as_build, l_used.clone(), r_used.clone(), None, None, None, None, None, 3, base::RightOuterJoin, 100);
        }
    }
}

#[test]
pub fn test_right_outer_join_probe_other_condition() {
    // other condition 使用左第 1 列和右第 3 列参与比较，输出列矩阵同时覆盖空输出。
    let (int_tp, nullable_int_tp, uint_tp, string_tp) = right_outer_condition_types();
    let mut r_types = vec![int_tp.clone(), int_tp.clone(), string_tp.clone(), uint_tp.clone(), string_tp.clone()];
    r_types.extend(retTypes);
    let l_types = vec![int_tp.clone(), int_tp.clone(), string_tp.clone(), uint_tp.clone(), string_tp.clone()];
    let other_condition = new_gt_condition(1, 8);
    for right_build in [false, true] {
        for test_filter in [false, true] {
            let right_filter = test_filter.then(|| createSimpleFilter(&mut testing::T::new()));
            testJoinProbe(&mut testing::T::new(), false, vec![0], vec![0], vec![int_tp.clone()], vec![int_tp.clone()], l_types.clone(), r_types.clone(), right_build, Some(vec![1, 2, 4]), Some(vec![0]), Some(vec![1]), Some(vec![3]), None, right_filter.clone(), Some(other_condition.clone()), 3, base::RightOuterJoin, 200);
            testJoinProbe(&mut testing::T::new(), false, vec![0], vec![0], vec![nullable_int_tp.clone()], vec![nullable_int_tp.clone()], toNullableTypes(l_types.clone()), toNullableTypes(r_types.clone()), right_build, None, None, Some(vec![1]), Some(vec![3]), None, right_filter, Some(other_condition.clone()), 3, base::RightOuterJoin, 200);
        }
    }
}

#[test]
pub fn test_right_outer_join_probe_with_sel() {
    // hasSel=true 额外验证 chunk selection 下 filter index 和 row index 的映射。
    let (int_tp, nullable_int_tp, nullable_uint_tp, uint_tp, string_tp) = right_outer_sel_types();
    let l_types = vec![int_tp.clone(), int_tp.clone(), string_tp.clone(), uint_tp.clone(), string_tp.clone()];
    let mut r_types = l_types.clone();
    r_types.extend(retTypes);
    let other_condition = new_gt_condition_with_types(1, nullable_int_tp.clone(), 8, nullable_uint_tp.clone());
    for right_build in [false, true] {
        for use_filter in [false, true] {
            let right_filter = use_filter.then(|| createSimpleFilter(&mut testing::T::new()));
            testJoinProbe(&mut testing::T::new(), true, vec![0], vec![0], vec![int_tp.clone()], vec![int_tp.clone()], l_types.clone(), r_types.clone(), right_build, Some(vec![1, 2, 4]), Some(vec![0]), Some(vec![1]), Some(vec![3]), None, right_filter.clone(), Some(other_condition.clone()), 3, base::RightOuterJoin, 500);
            testJoinProbe(&mut testing::T::new(), true, vec![0], vec![0], vec![nullable_int_tp.clone()], vec![nullable_int_tp.clone()], toNullableTypes(l_types.clone()), toNullableTypes(r_types.clone()), right_build, None, None, Some(vec![1]), Some(vec![3]), None, right_filter, Some(other_condition.clone()), 3, base::RightOuterJoin, 500);
        }
    }
}
*/

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
