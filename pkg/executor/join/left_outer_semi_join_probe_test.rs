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
// 以及 spill/恢复后标记语义不变。块注释内保留 Go 版 nested-loop 期望结果草稿。

/*
// 可编译或可运行；它用于人工核对 left outer semi、semi、anti semi 共用 helper 的测试语义。

fn gen_left_outer_semi_join_result(
    t: &mut TestingDraft,
    sess_ctx: &SessionCtxDraft,
    left_filter: Option<CnfExprsDraft>,
    left_chunks: &[ChunkDraft],
    right_chunks: &[ChunkDraft],
    left_key_index: &[i32],
    right_key_index: &[i32],
    left_types: &[FieldTypeDraft],
    right_types: &[FieldTypeDraft],
    left_key_types: &[FieldTypeDraft],
    right_key_types: &[FieldTypeDraft],
    left_used_columns: &[i32],
    other_conditions: Option<CnfExprsDraft>,
    result_types: &[FieldTypeDraft],
) -> Vec<ChunkDraft> {
    gen_left_outer_semi_or_semi_join_or_left_outer_anti_semi_result_impl(
        t, sess_ctx, left_filter, left_chunks, right_chunks, left_key_index, right_key_index,
        left_types, right_types, left_key_types, right_key_types, left_used_columns,
        other_conditions, result_types, true, false,
    )
}

fn gen_semi_join_result(
    t: &mut TestingDraft,
    sess_ctx: &SessionCtxDraft,
    left_filter: Option<CnfExprsDraft>,
    left_chunks: &[ChunkDraft],
    right_chunks: &[ChunkDraft],
    left_key_index: &[i32],
    right_key_index: &[i32],
    left_types: &[FieldTypeDraft],
    right_types: &[FieldTypeDraft],
    left_key_types: &[FieldTypeDraft],
    right_key_types: &[FieldTypeDraft],
    left_used_columns: &[i32],
    other_conditions: Option<CnfExprsDraft>,
    result_types: &[FieldTypeDraft],
) -> Vec<ChunkDraft> {
    gen_left_outer_semi_or_semi_join_or_left_outer_anti_semi_result_impl(
        t, sess_ctx, left_filter, left_chunks, right_chunks, left_key_index, right_key_index,
        left_types, right_types, left_key_types, right_key_types, left_used_columns,
        other_conditions, result_types, false, false,
    )
}

// gen_left_outer_semi_or_semi_join_or_left_outer_anti_semi_result_impl 对应 Go 的 nested-loop 期望结果生成器。
#[allow(clippy::too_many_arguments)]
fn gen_left_outer_semi_or_semi_join_or_left_outer_anti_semi_result_impl(
    t: &mut TestingDraft,
    sess_ctx: &SessionCtxDraft,
    left_filter: Option<CnfExprsDraft>,
    left_chunks: &[ChunkDraft],
    right_chunks: &[ChunkDraft],
    left_key_index: &[i32],
    right_key_index: &[i32],
    left_types: &[FieldTypeDraft],
    right_types: &[FieldTypeDraft],
    left_key_types: &[FieldTypeDraft],
    right_key_types: &[FieldTypeDraft],
    left_used_columns: &[i32],
    other_conditions: Option<CnfExprsDraft>,
    result_types: &[FieldTypeDraft],
    is_left_outer: bool,
    is_anti: bool,
) -> Vec<ChunkDraft> {
    let mut filter_vector = Vec::new();
    let mut return_chunks = Vec::new();
    let mut result_chk = ChunkDraft::new(result_types, sess_ctx.max_chunk_size, sess_ctx.max_chunk_size);
    let shallow_row_types = concat_types(left_types, right_types);
    let mut shallow_row = MutRowDraft::from_types(&shallow_row_types);

    for left_chunk in left_chunks {
        if let Some(filter) = &left_filter {
            filter_vector = vectorized_filter(sess_ctx, filter, left_chunk, filter_vector);
            assert_no_error(t);
        }

        for left_index in 0..left_chunk.num_rows() {
            let mut filter_index = left_index;
            if let Some(sel) = left_chunk.sel() {
                filter_index = sel[left_index as usize];
            }

            if left_filter.is_some() && !filter_vector[filter_index as usize] {
                if is_left_outer {
                    // 左表 filter 过滤时，left outer semi/anti 仍输出左行和 matched flag。
                    append_to_result_chk(left_chunk.get_row(left_index), RowDraft::empty(), left_used_columns, &[], &mut result_chk);
                    result_chk.append_int64(left_used_columns.len(), if is_anti { 1 } else { 0 });
                }
                flush_if_full(&mut return_chunks, &mut result_chk, result_types, sess_ctx.max_chunk_size);
                continue;
            }

            let left_row = left_chunk.get_row(left_index);
            let mut has_match = false;
            let mut has_null = false;

            for right_chunk in right_chunks {
                for right_index in 0..right_chunk.num_rows() {
                    let right_row = right_chunk.get_row(right_index);
                    let mut valid = !contains_null_key(&left_row, left_key_index) && !contains_null_key(&right_row, right_key_index);
                    if valid {
                        valid = equal_chunk_row(sess_ctx, &left_row, left_key_types, left_key_index, &right_row, right_key_types, right_key_index);
                        assert_no_error(t);
                    }
                    if valid {
                        if let Some(conds) = &other_conditions {
                            // other condition 同时返回 matched 与 null；null 会影响 semi/anti flag 的三值逻辑。
                            let (matched, null) = eval_bool(sess_ctx, conds, &left_row, &right_row, &mut shallow_row);
                            assert_no_error(t);
                            valid = matched;
                            has_null = has_null || null;
                        }
                    }
                    if valid {
                        has_match = true;
                        break;
                    }
                }
                if has_match {
                    break;
                }
            }

            if is_left_outer {
                append_to_result_chk(left_row.clone(), RowDraft::empty(), left_used_columns, &[], &mut result_chk);
                if is_anti {
                    append_anti_flag(&mut result_chk, left_used_columns.len(), has_match, has_null);
                } else {
                    append_semi_flag(&mut result_chk, left_used_columns.len(), has_match, has_null);
                }
            } else if has_match {
                append_to_result_chk(left_row, RowDraft::empty(), left_used_columns, &[], &mut result_chk);
            }

            flush_if_full(&mut return_chunks, &mut result_chk, result_types, sess_ctx.max_chunk_size);
        }
    }
    if result_chk.num_rows() > 0 {
        return_chunks.push(result_chk);
    }
    return_chunks
}

fn append_anti_flag(result_chk: &mut ChunkDraft, col: usize, has_match: bool, has_null: bool) {
    if has_match {
        result_chk.append_int64(col, 0);
    } else if has_null {
        result_chk.append_null(col);
    } else {
        result_chk.append_int64(col, 1);
    }
}

fn append_semi_flag(result_chk: &mut ChunkDraft, col: usize, has_match: bool, has_null: bool) {
    if has_match {
        result_chk.append_int64(col, 1);
    } else if has_null {
        result_chk.append_null(col);
    } else {
        result_chk.append_int64(col, 0);
    }
}

fn test_left_outer_semi_or_semi_join_probe_basic(is_left_outer: bool, is_anti: bool) {
    let tiny_tp = not_null_field_type("mysql.TypeTiny");
    let int_tp = not_null_field_type("mysql.TypeLonglong");
    let uint_tp = field_type_with_flags("mysql.TypeLonglong", &["mysql.NotNullFlag", "mysql.UnsignedFlag"]);
    let string_tp = not_null_field_type("mysql.TypeVarString");
    let l_types = vec![int_tp.clone(), string_tp.clone(), uint_tp.clone(), string_tp.clone(), tiny_tp.clone()];
    let mut r_types = l_types.clone();
    r_types.extend(ret_types());
    let mut r_types1 = vec![uint_tp.clone(), string_tp.clone(), int_tp.clone(), string_tp.clone(), tiny_tp.clone()];
    r_types1.extend(r_types1.clone());

    let mut right_as_build_side = vec![true];
    if !is_left_outer {
        right_as_build_side.push(false);
    }
    let mut has_filter = vec![false];
    if is_left_outer {
        has_filter.push(true);
    }
    let join_type = semi_join_type(is_left_outer, is_anti);
    let simple_filter = create_simple_filter();

    let cases = vec![
        TestCase::new(vec![0], vec![0], vec![int_tp.clone()], vec![int_tp.clone()], l_types.clone(), r_types.clone(), None, Some(vec![]), None, None, None),
        TestCase::new(vec![0], vec![0], vec![int_tp.clone()], vec![int_tp.clone()], l_types.clone(), r_types.clone(), Some(vec![0, 1, 2, 3]), Some(vec![]), None, None, None),
        TestCase::new(vec![0], vec![0], vec![int_tp.clone()], vec![int_tp.clone()], l_types.clone(), r_types.clone(), Some(vec![]), Some(vec![]), None, None, None),
        TestCase::new(vec![0], vec![0], vec![int_tp.clone()], vec![int_tp.clone()], l_types.clone(), r_types.clone(), Some(vec![0, 2]), Some(vec![]), None, None, None),
        TestCase::new(vec![0], vec![0], vec![int_tp.clone()], vec![uint_tp.clone()], l_types.clone(), r_types1, Some(vec![0, 1, 2, 3]), Some(vec![]), None, None, None),
        TestCase::new(vec![0, 1], vec![0, 1], vec![int_tp.clone(), string_tp.clone()], vec![int_tp.clone(), string_tp.clone()], l_types.clone(), r_types.clone(), Some(vec![0, 1, 2, 3]), Some(vec![]), None, None, None),
    ];
    for case in cases {
        for right_build in &right_as_build_side {
            for test_filter in &has_filter {
                let left_filter = if *test_filter { Some(simple_filter.clone()) } else { None };
                // 每个 case 均跑普通类型和 nullable 类型两组，保持 Go 的覆盖矩阵。
                run_test_join_probe_case(&case, *right_build, left_filter.clone(), join_type, 4, 200, false, false);
                run_test_join_probe_case(&case, *right_build, left_filter, join_type, 4, 200, false, true);
            }
        }
    }
}

fn test_left_outer_semi_join_probe_all_join_keys(is_left_outer: bool, is_anti: bool) {
    let l_types = all_not_null_join_key_types();
    let r_types = l_types.clone();
    let l_used: Vec<i32> = (0..18).collect();
    let r_used = vec![];
    let join_type = semi_join_type(is_left_outer, is_anti);
    let mut right_as_build_side = vec![true];
    if !is_left_outer {
        right_as_build_side.push(false);
    }

    for i in 0..l_types.len() {
        for right_build in &right_as_build_side {
            test_join_probe(false, vec![i as i32], vec![i as i32], vec![l_types[i].clone()], vec![r_types[i].clone()], l_types.clone(), r_types.clone(), *right_build, l_used.clone(), r_used.clone(), None, None, None, None, None, 4, join_type, 100);
            test_join_probe(false, vec![i as i32], vec![i as i32], to_nullable_types(vec![l_types[i].clone()]), to_nullable_types(vec![r_types[i].clone()]), to_nullable_types(l_types.clone()), to_nullable_types(r_types.clone()), *right_build, l_used.clone(), r_used.clone(), None, None, None, None, None, 4, join_type, 100);
        }
    }

    for right_build in &right_as_build_side {
        for (left_idx, right_idx) in [(vec![1, 2], vec![1, 2]), (vec![1, 17], vec![1, 17]), (vec![1, 13], vec![1, 13]), (vec![1, 14], vec![1, 14])] {
            // 四组组合与 Go 相同，分别覆盖 composed key 的定长/变长、内联/非内联路径。
            let l_keys = left_idx.iter().map(|idx| l_types[*idx as usize].clone()).collect();
            let r_keys = right_idx.iter().map(|idx| r_types[*idx as usize].clone()).collect();
            test_join_probe(false, left_idx.clone(), right_idx.clone(), l_keys.clone(), r_keys.clone(), l_types.clone(), r_types.clone(), *right_build, l_used.clone(), r_used.clone(), None, None, None, None, None, 4, join_type, 100);
            test_join_probe(false, left_idx, right_idx, to_nullable_types(l_keys), to_nullable_types(r_keys), to_nullable_types(l_types.clone()), to_nullable_types(r_types.clone()), *right_build, l_used.clone(), r_used.clone(), None, None, None, None, None, 4, join_type, 100);
        }
    }
}

fn test_left_outer_semi_join_probe_other_condition(is_left_outer: bool, is_anti: bool) {
    let int_tp = not_null_field_type("mysql.TypeLonglong");
    let nullable_int_tp = field_type("mysql.TypeLonglong");
    let uint_tp = field_type_with_flags("mysql.TypeLonglong", &["mysql.NotNullFlag", "mysql.UnsignedFlag"]);
    let string_tp = not_null_field_type("mysql.TypeVarString");
    let l_types = vec![int_tp.clone(), int_tp.clone(), string_tp.clone(), uint_tp.clone(), string_tp.clone()];
    let mut r_types = l_types.clone();
    r_types.extend(r_types.clone());

    let other_condition = make_condition("ast.GT", 1, &nullable_int_tp, 8, &nullable_int_tp, false);
    let other_condition2 = make_condition("ast.EQ", 1, &nullable_int_tp, 8, &nullable_int_tp, true);
    let join_type = semi_join_type(is_left_outer, is_anti);
    let mut has_filter = vec![false];
    if is_left_outer {
        has_filter.push(true);
    }
    let mut right_as_build_side = vec![true];
    if !is_left_outer {
        right_as_build_side.push(false);
    }
    let simple_filter = create_simple_filter();
    let right_used = vec![];

    for right_build in &right_as_build_side {
        for test_filter in &has_filter {
            let left_filter = if *test_filter { Some(simple_filter.clone()) } else { None };
            // Go 对 GT 和 IN 子查询转换来的 EQ(InOperand=true) 两组 other condition 各跑 4 个用例。
            for cond in [other_condition.clone(), other_condition2.clone()] {
                test_join_probe(false, vec![0], vec![0], vec![int_tp.clone()], vec![int_tp.clone()], l_types.clone(), r_types.clone(), *right_build, vec![1, 2, 4], right_used.clone(), Some(vec![1]), Some(vec![3]), left_filter.clone(), None, Some(cond.clone()), 4, join_type, 200);
                test_join_probe(false, vec![0], vec![0], vec![int_tp.clone()], vec![int_tp.clone()], l_types.clone(), r_types.clone(), *right_build, vec![], right_used.clone(), Some(vec![1]), Some(vec![3]), left_filter.clone(), None, Some(cond.clone()), 4, join_type, 200);
                test_join_probe(false, vec![0], vec![0], vec![nullable_int_tp.clone()], vec![nullable_int_tp.clone()], to_nullable_types(l_types.clone()), to_nullable_types(r_types.clone()), *right_build, vec![1, 2, 4], right_used.clone(), Some(vec![1]), Some(vec![3]), left_filter.clone(), None, Some(cond.clone()), 4, join_type, 200);
                test_join_probe(false, vec![0], vec![0], vec![nullable_int_tp.clone()], vec![nullable_int_tp.clone()], to_nullable_types(l_types.clone()), to_nullable_types(r_types.clone()), *right_build, vec![], right_used.clone(), Some(vec![1]), Some(vec![3]), left_filter.clone(), None, Some(cond), 4, join_type, 200);
            }
        }
    }
}

fn test_left_outer_semi_join_probe_with_sel(is_left_outer: bool, is_anti: bool) {
    let int_tp = not_null_field_type("mysql.TypeLonglong");
    let nullable_int_tp = field_type("mysql.TypeLonglong");
    let uint_tp = field_type_with_flags("mysql.TypeLonglong", &["mysql.NotNullFlag", "mysql.UnsignedFlag"]);
    let nullable_uint_tp = field_type_with_flags("mysql.TypeLonglong", &["mysql.UnsignedFlag"]);
    let string_tp = not_null_field_type("mysql.TypeVarString");
    let l_types = vec![int_tp.clone(), int_tp.clone(), string_tp.clone(), uint_tp.clone(), string_tp.clone()];
    let mut r_types = l_types.clone();
    r_types.extend(r_types.clone());
    let other_condition = make_condition("ast.GT", 1, &nullable_int_tp, 8, &nullable_uint_tp, false);
    let join_type = semi_join_type(is_left_outer, is_anti);
    let mut right_as_build_side = vec![true];
    if !is_left_outer {
        right_as_build_side.push(false);
    }
    let mut has_filter = vec![false];
    if is_left_outer {
        has_filter.push(true);
    }
    let simple_filter = create_simple_filter();

    for right_build in &right_as_build_side {
        for use_filter in &has_filter {
            let left_filter = if *use_filter { Some(simple_filter.clone()) } else { None };
            // useSel=true 与 Go 的 WithSel 测试一致，覆盖 selection vector 路径。
            test_join_probe(true, vec![0], vec![0], vec![int_tp.clone()], vec![int_tp.clone()], l_types.clone(), r_types.clone(), *right_build, vec![1, 2, 4], vec![], Some(vec![1]), Some(vec![3]), left_filter.clone(), None, Some(other_condition.clone()), 4, join_type, 500);
            test_join_probe(true, vec![0], vec![0], vec![int_tp.clone()], vec![int_tp.clone()], l_types.clone(), r_types.clone(), *right_build, vec![], vec![], Some(vec![1]), Some(vec![3]), left_filter.clone(), None, Some(other_condition.clone()), 4, join_type, 500);
            test_join_probe(true, vec![0], vec![0], vec![nullable_int_tp.clone()], vec![nullable_int_tp.clone()], to_nullable_types(l_types.clone()), to_nullable_types(r_types.clone()), *right_build, vec![1, 2, 4], vec![], Some(vec![1]), Some(vec![3]), left_filter.clone(), None, Some(other_condition.clone()), 4, join_type, 500);
            test_join_probe(true, vec![0], vec![0], vec![nullable_int_tp.clone()], vec![nullable_int_tp.clone()], to_nullable_types(l_types.clone()), to_nullable_types(r_types.clone()), *right_build, vec![], vec![], Some(vec![1]), Some(vec![3]), left_filter.clone(), None, Some(other_condition.clone()), 4, join_type, 500);
        }
    }
}

#[test]
fn test_left_outer_semi_join_probe_basic() {
    test_left_outer_semi_or_semi_join_probe_basic(true, false);
}

#[test]
fn test_left_outer_semi_join_probe_all_join_keys() {
    test_left_outer_semi_join_probe_all_join_keys(true, false);
}

#[test]
fn test_left_outer_semi_join_probe_other_condition() {
    test_left_outer_semi_join_probe_other_condition(true, false);
}

#[test]
fn test_left_outer_semi_join_probe_with_sel() {
    test_left_outer_semi_join_probe_with_sel(true, false);
}

#[test]
fn test_left_outer_semi_join_build_result_fast_path() {
    test_left_outer_semi_join_or_left_outer_anti_semi_join_build_result_fast_path(false);
}

fn test_left_outer_semi_join_or_left_outer_anti_semi_join_build_result_fast_path(is_anti: bool) {
    let int_tp = not_null_field_type("mysql.TypeLonglong");
    let nullable_int_tp = field_type("mysql.TypeLonglong");
    let uint_tp = field_type_with_flags("mysql.TypeLonglong", &["mysql.NotNullFlag", "mysql.UnsignedFlag"]);
    let string_tp = not_null_field_type("mysql.TypeVarString");
    let l_types = vec![int_tp.clone(), int_tp.clone(), string_tp.clone(), uint_tp.clone(), string_tp.clone()];
    let mut r_types = l_types.clone();
    r_types.extend(r_types.clone());
    let other_condition = make_condition("ast.GT", 1, &nullable_int_tp, 8, &nullable_int_tp, false);
    let other_condition2 = make_condition("ast.EQ", 1, &nullable_int_tp, 8, &nullable_int_tp, true);
    let join_type = if is_anti { "base.AntiLeftOuterSemiJoin" } else { "base.LeftOuterSemiJoin" };
    let simple_filter = create_simple_filter();

    for test_filter in [false, true] {
        let left_filter = if test_filter { Some(simple_filter.clone()) } else { None };
        for cond in [Some(other_condition.clone()), Some(other_condition2.clone()), None] {
            // Go 注释：MockContext MaxChunkSize=32，输入 chunk size 小于 32 才能触发 build result fast path。
            test_join_probe(false, vec![0], vec![0], vec![int_tp.clone()], vec![int_tp.clone()], l_types.clone(), r_types.clone(), true, vec![1, 2, 4], vec![], Some(vec![1]), Some(vec![3]), left_filter.clone(), None, cond.clone(), 4, join_type, 30);
            test_join_probe(false, vec![0], vec![0], vec![int_tp.clone()], vec![int_tp.clone()], l_types.clone(), r_types.clone(), true, vec![], vec![], Some(vec![1]), Some(vec![3]), left_filter.clone(), None, cond.clone(), 4, join_type, 30);
            test_join_probe(true, vec![0], vec![0], vec![nullable_int_tp.clone()], vec![nullable_int_tp.clone()], to_nullable_types(l_types.clone()), to_nullable_types(r_types.clone()), true, vec![1, 2, 4], vec![], Some(vec![1]), Some(vec![3]), left_filter.clone(), None, cond.clone(), 4, join_type, 30);
            test_join_probe(true, vec![0], vec![0], vec![nullable_int_tp.clone()], vec![nullable_int_tp.clone()], to_nullable_types(l_types.clone()), to_nullable_types(r_types.clone()), true, vec![], vec![], Some(vec![1]), Some(vec![3]), left_filter.clone(), None, cond, 4, join_type, 30);
        }
    }
}

#[test]
fn test_left_outer_semi_join_spill() {
    test_left_outer_semi_join_or_left_outer_anti_semi_join_spill(false);
}

fn test_left_outer_semi_join_or_left_outer_anti_semi_join_spill(is_anti: bool) {
    // Go defer config.RestoreFunc() 并把 TempStoragePath 指向 t.TempDir；这里只记录资源收尾要求。
    let mut ctx = SessionCtxDraft { max_chunk_size: 32, init_chunk_size: 32 };
    let test_func_name = "util.GetFunctionName()";
    let (left_data_source, right_data_source) = build_left_and_right_data_source(&ctx, false);
    let (left_data_source_with_sel, right_data_source_with_sel) = build_left_and_right_data_source(&ctx, true);

    let int_tp = not_null_field_type("mysql.TypeLonglong");
    let string_tp = not_null_field_type("mysql.TypeVarString");
    let left_types = vec![int_tp.clone(), int_tp.clone(), int_tp.clone(), string_tp.clone(), int_tp.clone()];
    let right_types = vec![int_tp.clone(), int_tp.clone(), string_tp.clone(), int_tp.clone(), int_tp.clone()];
    let left_keys = vec![ColumnDraft::new(1, int_tp.clone()), ColumnDraft::new(3, string_tp.clone())];
    let right_keys = vec![ColumnDraft::new(0, int_tp.clone()), ColumnDraft::new(2, string_tp.clone())];
    let other_condition = make_condition("ast.GT", 1, &int_tp, 8, &int_tp, false);
    let join_type = if is_anti { "base.AntiLeftOuterSemiJoin" } else { "base.LeftOuterSemiJoin" };

    let params = vec![
        SpillTestParam::new(true, left_keys.clone(), right_keys.clone(), left_types.clone(), right_types.clone(), vec![0, 1, 3, 4], vec![], None, None, None, vec![2000000, 2000000, 3000000, 100000, 5000], test_func_name),
        SpillTestParam::new(true, left_keys.clone(), right_keys.clone(), left_types.clone(), right_types.clone(), vec![0, 1, 3, 4], vec![], Some(other_condition.clone()), Some(vec![1]), Some(vec![3]), vec![2000000, 2000000, 3300000, 100000, 5000], test_func_name),
    ];
    for param in params {
        test_spill(&mut ctx, join_type, &left_data_source, &right_data_source, param);
    }

    let params_with_sel = vec![
        SpillTestParam::new(true, left_keys.clone(), right_keys.clone(), left_types.clone(), right_types.clone(), vec![0, 1, 3, 4], vec![], None, None, None, vec![700000, 1000000, 1500000, 100000, 5000], test_func_name),
        SpillTestParam::new(true, left_keys, right_keys, left_types, right_types, vec![0, 1, 3, 4], vec![], Some(other_condition), Some(vec![1]), Some(vec![3]), vec![1000000, 1000000, 1700000, 100000, 5000], test_func_name),
    ];
    for param in params_with_sel {
        test_spill(&mut ctx, join_type, &left_data_source_with_sel, &right_data_source_with_sel, param);
    }
    check_no_leak_files(test_func_name);
}

#[derive(Clone)]
struct FieldTypeDraft { mysql_type: &'static str, flags: Vec<&'static str> }
fn field_type(mysql_type: &'static str) -> FieldTypeDraft { FieldTypeDraft { mysql_type, flags: Vec::new() } }
fn not_null_field_type(mysql_type: &'static str) -> FieldTypeDraft { field_type_with_flags(mysql_type, &["mysql.NotNullFlag"]) }
fn field_type_with_flags(mysql_type: &'static str, flags: &[&'static str]) -> FieldTypeDraft { FieldTypeDraft { mysql_type, flags: flags.to_vec() } }
fn ret_types() -> Vec<FieldTypeDraft> { vec![field_type("retTypes...")] }
fn all_not_null_join_key_types() -> Vec<FieldTypeDraft> {
    vec![
        not_null_field_type("mysql.TypeTiny"), not_null_field_type("mysql.TypeLonglong"),
        field_type_with_flags("mysql.TypeLonglong", &["mysql.UnsignedFlag", "mysql.NotNullFlag"]),
        not_null_field_type("mysql.TypeYear"), not_null_field_type("mysql.TypeDuration"),
        not_null_field_type("mysql.TypeEnum"), field_type_with_flags("mysql.TypeEnum", &["mysql.EnumSetAsIntFlag", "mysql.NotNullFlag"]),
        not_null_field_type("mysql.TypeSet"), not_null_field_type("mysql.TypeBit"), not_null_field_type("mysql.TypeJSON"),
        not_null_field_type("mysql.TypeFloat"), not_null_field_type("mysql.TypeDouble"), not_null_field_type("mysql.TypeVarString"),
        not_null_field_type("mysql.TypeDatetime"), not_null_field_type("mysql.TypeNewDecimal"), not_null_field_type("mysql.TypeTimestamp"),
        not_null_field_type("mysql.TypeDate"), not_null_field_type("mysql.TypeBlob"),
    ]
}
fn to_nullable_types(mut types: Vec<FieldTypeDraft>) -> Vec<FieldTypeDraft> {
    for tp in &mut types { tp.flags.retain(|flag| *flag != "mysql.NotNullFlag"); }
    types
}
fn semi_join_type(is_left_outer: bool, is_anti: bool) -> &'static str {
    if is_left_outer {
        if is_anti { "base.AntiLeftOuterSemiJoin" } else { "base.LeftOuterSemiJoin" }
    } else {
        "base.SemiJoin"
    }
}

#[derive(Clone)]
struct CnfExprsDraft(&'static str);
struct TestingDraft;
struct SessionCtxDraft { max_chunk_size: i32, init_chunk_size: i32 }
#[derive(Clone)]
struct RowDraft;
struct MutRowDraft;
#[derive(Clone)]
struct ChunkDraft { rows: i32, selection: Option<Vec<i32>> }
#[derive(Clone)]
struct DataSourceDraft;
#[derive(Clone)]
struct ColumnDraft { index: i32, ret_type: FieldTypeDraft }
struct SpillTestParam;

impl RowDraft { fn empty() -> Self { Self } }
impl MutRowDraft { fn from_types(_types: &[FieldTypeDraft]) -> Self { Self } }
impl ChunkDraft {
    fn new(_types: &[FieldTypeDraft], _init: i32, _max: i32) -> Self { Self { rows: 0, selection: None } }
    fn num_rows(&self) -> i32 { self.rows }
    fn sel(&self) -> Option<&Vec<i32>> { self.selection.as_ref() }
    fn get_row(&self, _idx: i32) -> RowDraft { RowDraft }
    fn append_int64(&mut self, _col: usize, _value: i64) {}
    fn append_null(&mut self, _col: usize) {}
}
impl ColumnDraft { fn new(index: i32, ret_type: FieldTypeDraft) -> Self { Self { index, ret_type } } }
impl SpillTestParam {
    #[allow(clippy::too_many_arguments)]
    fn new(_right_as_build: bool, _left_keys: Vec<ColumnDraft>, _right_keys: Vec<ColumnDraft>, _left_types: Vec<FieldTypeDraft>, _right_types: Vec<FieldTypeDraft>, _left_used: Vec<i32>, _right_used: Vec<i32>, _other_condition: Option<CnfExprsDraft>, _left_used_by_other: Option<Vec<i32>>, _right_used_by_other: Option<Vec<i32>>, _expected: Vec<i64>, _name: &'static str) -> Self { Self }
}
fn concat_types(left: &[FieldTypeDraft], right: &[FieldTypeDraft]) -> Vec<FieldTypeDraft> { let mut out = left.to_vec(); out.extend_from_slice(right); out }
fn vectorized_filter(_ctx: &SessionCtxDraft, _filter: &CnfExprsDraft, _chunk: &ChunkDraft, old: Vec<bool>) -> Vec<bool> { old }
fn assert_no_error(_t: &mut TestingDraft) {}
fn append_to_result_chk(_left: RowDraft, _right: RowDraft, _left_used: &[i32], _right_used: &[i32], _result: &mut ChunkDraft) {}
fn flush_if_full(_return_chunks: &mut Vec<ChunkDraft>, _result: &mut ChunkDraft, _types: &[FieldTypeDraft], _max_chunk_size: i32) {}
fn contains_null_key(_row: &RowDraft, _key_index: &[i32]) -> bool { false }
fn equal_chunk_row(_ctx: &SessionCtxDraft, _left: &RowDraft, _left_types: &[FieldTypeDraft], _left_idx: &[i32], _right: &RowDraft, _right_types: &[FieldTypeDraft], _right_idx: &[i32]) -> bool { true }
fn eval_bool(_ctx: &SessionCtxDraft, _conds: &CnfExprsDraft, _left: &RowDraft, _right: &RowDraft, _shallow: &mut MutRowDraft) -> (bool, bool) { (true, false) }
fn create_simple_filter() -> CnfExprsDraft { CnfExprsDraft("createSimpleFilter(t)") }
fn make_condition(_op: &'static str, _l_idx: i32, _l_tp: &FieldTypeDraft, _r_idx: i32, _r_tp: &FieldTypeDraft, _in_operand: bool) -> CnfExprsDraft { CnfExprsDraft("expression.NewFunction(mock.NewContext(), op, ...)") }
fn build_left_and_right_data_source(_ctx: &SessionCtxDraft, _with_sel: bool) -> (DataSourceDraft, DataSourceDraft) { (DataSourceDraft, DataSourceDraft) }
fn test_spill(_ctx: &mut SessionCtxDraft, _join_type: &'static str, _left: &DataSourceDraft, _right: &DataSourceDraft, _param: SpillTestParam) {}
fn check_no_leak_files(_test_func_name: &'static str) {}

struct TestCase {
    left_key_index: Vec<i32>,
    right_key_index: Vec<i32>,
    left_key_types: Vec<FieldTypeDraft>,
    right_key_types: Vec<FieldTypeDraft>,
    left_types: Vec<FieldTypeDraft>,
    right_types: Vec<FieldTypeDraft>,
    left_used: Option<Vec<i32>>,
    right_used: Option<Vec<i32>>,
    left_used_by_other_condition: Option<Vec<i32>>,
    right_used_by_other_condition: Option<Vec<i32>>,
    other_condition: Option<CnfExprsDraft>,
}

impl TestCase {
    fn new(left_key_index: Vec<i32>, right_key_index: Vec<i32>, left_key_types: Vec<FieldTypeDraft>, right_key_types: Vec<FieldTypeDraft>, left_types: Vec<FieldTypeDraft>, right_types: Vec<FieldTypeDraft>, left_used: Option<Vec<i32>>, right_used: Option<Vec<i32>>, left_used_by_other_condition: Option<Vec<i32>>, right_used_by_other_condition: Option<Vec<i32>>, other_condition: Option<CnfExprsDraft>) -> Self {
        Self { left_key_index, right_key_index, left_key_types, right_key_types, left_types, right_types, left_used, right_used, left_used_by_other_condition, right_used_by_other_condition, other_condition }
    }
}

fn run_test_join_probe_case(case: &TestCase, right_build: bool, left_filter: Option<CnfExprsDraft>, join_type: &'static str, partition_number: i32, row_count: i32, use_sel: bool, nullable: bool) {
    let left_key_types = if nullable { to_nullable_types(case.left_key_types.clone()) } else { case.left_key_types.clone() };
    let right_key_types = if nullable { to_nullable_types(case.right_key_types.clone()) } else { case.right_key_types.clone() };
    let left_types = if nullable { to_nullable_types(case.left_types.clone()) } else { case.left_types.clone() };
    let right_types = if nullable { to_nullable_types(case.right_types.clone()) } else { case.right_types.clone() };
    test_join_probe(use_sel, case.left_key_index.clone(), case.right_key_index.clone(), left_key_types, right_key_types, left_types, right_types, right_build, case.left_used.clone().unwrap_or_default(), case.right_used.clone().unwrap_or_default(), case.left_used_by_other_condition.clone(), case.right_used_by_other_condition.clone(), left_filter, None, case.other_condition.clone(), partition_number, join_type, row_count);
}

#[allow(clippy::too_many_arguments)]
fn test_join_probe(
    _use_sel: bool,
    _left_key_index: Vec<i32>,
    _right_key_index: Vec<i32>,
    _left_key_types: Vec<FieldTypeDraft>,
    _right_key_types: Vec<FieldTypeDraft>,
    _left_types: Vec<FieldTypeDraft>,
    _right_types: Vec<FieldTypeDraft>,
    _right_as_build: bool,
    _left_used: Vec<i32>,
    _right_used: Vec<i32>,
    _left_used_by_other_condition: Option<Vec<i32>>,
    _right_used_by_other_condition: Option<Vec<i32>>,
    _left_filter: Option<CnfExprsDraft>,
    _right_filter: Option<CnfExprsDraft>,
    _other_condition: Option<CnfExprsDraft>,
    _partition_number: i32,
    _join_type: &'static str,
    _row_count: i32,
) {
}
*/

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
