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

// Row Table Builder 单元测试。
//
// 覆盖 join key 序列化与分区、过滤行丢弃/保留、恢复 chunk 分区形状校验、
// 最大元素长度检查，以及非 2 的幂分区数拒绝。块注释内保留 Go 版测试草稿。

/*
// rowTableBuilder 如何序列化 join key、保存输出列、处理过滤行、保持 row pointer 对齐并初始化 partition 信息。

// createSimpleFilter 对应 Go helper：构造 col0 > 10000 的 CNF 表达式。
pub fn create_simple_filter(t: &mut testing::T) -> expression::CNFExprs {
    let tiny_tp = types::NewFieldType(mysql::TypeTiny);
    let int_tp = types::NewFieldType(mysql::TypeLonglong);
    let a = expression::Column { Index: 0, RetType: int_tp.clone() };
    let mut d = types::Datum::default();
    d.SetMinNotNull();
    d.SetValueWithDefaultCollation(10_000_i64);
    let b = expression::Constant { RetType: int_tp, Value: d };
    let sf = expression::NewFunction(mock::NewContext(), ast::GT, tiny_tp, a, b)
        .expect("Go require.NoError: create simple filter");
    let mut filter = expression::CNFExprs::new();
    filter.push(sf);
    filter
}

// createImpossibleFilter 对应 Go helper：构造 col0 > 10000 且 col0 < 5000 的不可能过滤器。
pub fn create_impossible_filter(t: &mut testing::T) -> expression::CNFExprs {
    let tiny_tp = types::NewFieldType(mysql::TypeTiny);
    let int_tp = types::NewFieldType(mysql::TypeLonglong);
    let a = expression::Column { Index: 0, RetType: int_tp.clone() };
    let mut filter = expression::CNFExprs::new();
    for (op, value) in [(ast::GT, 10_000_i64), (ast::LT, 5_000_i64)] {
        let mut d = types::Datum::default();
        d.SetMinNotNull();
        d.SetValueWithDefaultCollation(value);
        let b = expression::Constant { RetType: int_tp.clone(), Value: d };
        filter.push(expression::NewFunction(mock::NewContext(), op, tiny_tp.clone(), a.clone(), b)
            .expect("Go require.NoError: create impossible filter"));
    }
    filter
}

// checkRowLocationAlignment 对应 Go helper：检查 rowTable 中每个 row pointer 都满足 8 字节对齐。
pub fn check_row_location_alignment(t: &mut testing::T, row_tables: Vec<Option<RowTable>>) {
    for rt in row_tables.into_iter().flatten() {
        for seg in rt.segments {
            for index in 0..seg.rowStartOffset.len() {
                let row_location = seg.getRowPointer(index);
                // Go 把 unsafe.Pointer 转 uintptr 后取模；不解引用，只保留对齐断言。
                require::Equal(t, 0_u64, (row_location as usize as u64) % 8, "row location must be 8 byte aligned");
            }
        }
    }
}

// checkKeys 对应 Go 的 join key 序列化测试：构造 chunk、执行 builder，并比较 row table 中保存的 key bytes。
pub fn check_keys(
    t: &mut testing::T,
    with_sel_col: bool,
    build_filter: Option<expression::CNFExprs>,
    build_key_index: Vec<i32>,
    build_types: Vec<types::FieldType>,
    build_key_types: Vec<types::FieldType>,
    probe_key_types: Vec<types::FieldType>,
    keep_filtered_rows: bool,
) {
    let meta = newTableMeta(build_key_index.clone(), build_types.clone(), build_key_types.clone(), probe_key_types, None, vec![1], false);
    let mut build_schema = expression::Schema::default();
    for tp in &build_types {
        build_schema.Append(expression::Column { RetType: tp.clone(), ..Default::default() });
    }
    let has_nullable_key = build_key_types.iter().any(|tp| !mysql::HasNotNullFlag(tp.GetFlag()));
    let mut chk = testutil::GenRandomChunks(build_types.clone(), 2049);
    if with_sel_col {
        let sel: Vec<i32> = (0..chk.NumRows()).filter(|i| i % 3 != 0).collect();
        chk.SetSel(sel);
    }
    let mut hash_join_ctx = HashJoinCtxV2 { hashTableMeta: meta.clone(), BuildFilter: build_filter.clone(), ..Default::default() };
    hash_join_ctx.Concurrency = 1;
    hash_join_ctx.SetupPartitionInfo();
    hash_join_ctx.initHashTableContext();
    hash_join_ctx.SessCtx = mock::NewContext();
    let mut builder = createRowTableBuilder(build_key_index, build_key_types, hash_join_ctx.partitionNumber, has_nullable_key, build_filter.is_some(), keep_filtered_rows, meta.nullMapLength);
    let err = builder.processOneChunk(chk.clone(), hash_join_ctx.SessCtx.GetSessionVars().StmtCtx.TypeCtx(), &mut hash_join_ctx, 0);
    require::NoError(t, err, "processOneChunk returns error");
    require::Equal(t, chk.NumRows(), builder.usedRows.len());

    let row_tables = hash_join_ctx.hashTableContext.rowTables[0].clone();
    check_row_location_alignment(t, row_tables.clone());
    if keep_filtered_rows {
        // keepFilteredRows=true 时，过滤行和 null key 行也写入 row table，但 validJoinKeyPos 只记录可用于 join 的行。
        require::Equal(t, builder.usedRows.len() as u64, row_tables[0].rowCount());
        let mut valid_join_key_row_index = 0;
        for (logical_index, physical_index) in builder.usedRows.iter().enumerate() {
            if builder.is_filtered_or_null(*physical_index) {
                continue;
            }
            let valid_key_pos = row_tables[0].getValidJoinKeyPos(valid_join_key_row_index);
            require::Equal(t, logical_index as i32, valid_key_pos, "valid key pos not match");
            let row_start = row_tables[0].getRowPointer(valid_key_pos as usize);
            require::NotEqual(t, std::ptr::null_mut::<u8>(), row_start, "row start must not be nil");
            require::Equal(t, builder.serializedKeyVectorBuffer[logical_index].clone(), meta.getKeyBytes(row_start), "key not match");
            valid_join_key_row_index += 1;
        }
        require::Equal(t, -1, row_tables[0].getValidJoinKeyPos(valid_join_key_row_index), "validKeyPos must be -1 at the end of test");
    } else {
        // keepFilteredRows=false 时，过滤行不会写入 row table，rowIndex 只随有效行前进。
        let mut row_index = 0;
        for (logical_index, physical_index) in builder.usedRows.iter().enumerate() {
            if builder.is_filtered_or_null(*physical_index) {
                continue;
            }
            let row_start = row_tables[0].getRowPointer(row_index);
            require::NotEqual(t, std::ptr::null_mut::<u8>(), row_start, "row start must not be nil");
            require::Equal(t, builder.serializedKeyVectorBuffer[logical_index].clone(), meta.getKeyBytes(row_start), "key not match");
            row_index += 1;
        }
        require::Equal(t, std::ptr::null_mut::<u8>(), row_tables[0].getRowPointer(row_index), "row start must be nil at the end of the test");
    }
}

#[test]
pub fn test_key() {
    let int_tp = types::NewFieldType(mysql::TypeLonglong);
    let mut uint_tp = types::NewFieldType(mysql::TypeLonglong);
    uint_tp.AddFlag(mysql::UnsignedFlag);
    let string_tp = types::NewFieldType(mysql::TypeVarString);
    let binary_string_tp = types::NewFieldType(mysql::TypeBlob);
    let mut not_null_int_tp = types::NewFieldType(mysql::TypeLonglong);
    not_null_int_tp.SetFlag(mysql::NotNullFlag);
    let filter = create_simple_filter(&mut testing::T::new());

    let cases = vec![
        // inlined fixed length keys
        key_case(vec![0], vec![int_tp.clone(), uint_tp.clone(), uint_tp.clone()], vec![int_tp.clone()], vec![int_tp.clone()], None),
        key_case(vec![0], vec![not_null_int_tp.clone(), uint_tp.clone(), uint_tp.clone()], vec![not_null_int_tp.clone()], vec![not_null_int_tp.clone()], None),
        // inlined variable length keys
        key_case(vec![0, 1], vec![binary_string_tp.clone(), int_tp.clone(), uint_tp.clone()], vec![binary_string_tp.clone(), int_tp.clone()], vec![binary_string_tp.clone(), int_tp.clone()], None),
        // not inlined fixed/variable length keys
        key_case(vec![0], vec![int_tp.clone(), uint_tp.clone(), uint_tp.clone()], vec![int_tp.clone()], vec![uint_tp.clone()], None),
        key_case(vec![0, 1], vec![string_tp.clone(), int_tp.clone(), uint_tp.clone()], vec![string_tp.clone(), int_tp.clone()], vec![string_tp.clone(), int_tp.clone()], None),
        key_case(vec![2, 0, 1], vec![string_tp.clone(), int_tp.clone(), binary_string_tp.clone(), uint_tp.clone()], vec![binary_string_tp.clone(), string_tp.clone(), int_tp.clone()], vec![binary_string_tp.clone(), string_tp.clone(), int_tp.clone()], None),
    ];
    for case in cases {
        check_keys(&mut testing::T::new(), false, None, case.buildKeyIndex.clone(), case.buildTypes.clone(), case.buildKeyTypes.clone(), case.probeKeyTypes.clone(), false);
        check_keys(&mut testing::T::new(), false, None, case.buildKeyIndex, case.buildTypes, case.buildKeyTypes, case.probeKeyTypes, true);
    }
    // Go 额外覆盖 selection vector 和 build filter；这里保留两类开关组合。
    for with_sel in [true, false] {
        for keep in [true, false] {
            check_keys(&mut testing::T::new(), with_sel, Some(filter.clone()), vec![0], vec![not_null_int_tp.clone(), uint_tp.clone(), uint_tp.clone()], vec![not_null_int_tp.clone()], vec![not_null_int_tp.clone()], keep);
        }
    }
}

// checkColumnResult 对应 Go helper：把 row table 反序列化后的列与原 chunk 逐列比较。
pub fn check_column_result(
    t: &mut testing::T,
    builder: &RowTableBuilder,
    keep_filtered_rows: bool,
    result: &chunk::Chunk,
    expected: &chunk::Chunk,
    ctx: &HashJoinCtxV2,
    for_other_condition: bool,
) {
    if keep_filtered_rows {
        require::Equal(t, expected.NumRows(), result.NumRows());
    }
    let meta = &ctx.hashTableMeta;
    let column_iter = if for_other_condition {
        meta.rowColumnsOrder[..meta.columnCountNeededForOtherCondition].to_vec()
    } else {
        ctx.LUsed.clone()
    };
    for (index, org_index) in column_iter.iter().enumerate() {
        let result_col = result.Column(if for_other_condition { *org_index } else { index as i32 });
        require::Equal(t, result.NumRows(), result_col.Rows());
        let mut result_index = 0;
        for (logical_index, physical_index) in builder.usedRows.iter().enumerate() {
            if !keep_filtered_rows && builder.is_filtered_or_null(*physical_index) {
                continue;
            }
            let is_null = expected.GetRow(logical_index).IsNull(*org_index);
            require::Equal(t, is_null, result_col.IsNull(result_index), "data null flag not match");
            if !is_null {
                require::Equal(t, expected.GetRow(logical_index).GetRaw(*org_index), result_col.GetRaw(result_index), "data raw bytes not match");
            }
            result_index += 1;
        }
        require::Equal(t, result_index, result.NumRows());
    }
}

// checkColumns 对应 Go 的输出列反序列化测试，覆盖 other condition 临时列和最终结果列两条路径。
pub fn check_columns(
    t: &mut testing::T,
    with_sel_col: bool,
    build_filter: Option<expression::CNFExprs>,
    build_key_index: Vec<i32>,
    build_types: Vec<types::FieldType>,
    build_key_types: Vec<types::FieldType>,
    probe_key_types: Vec<types::FieldType>,
    keep_filtered_rows: bool,
    columns_used_by_other_condition: Option<Vec<i32>>,
    output_columns: Vec<i32>,
    need_used_flag: bool,
) {
    let meta = newTableMeta(build_key_index.clone(), build_types.clone(), build_key_types.clone(), probe_key_types, columns_used_by_other_condition.clone(), output_columns.clone(), need_used_flag);
    let result_types: Vec<_> = output_columns.iter().map(|idx| build_types[*idx as usize].clone()).collect();
    let has_nullable_key = build_key_types.iter().any(|tp| !mysql::HasNotNullFlag(tp.GetFlag()));
    let mut builder = createRowTableBuilder(build_key_index, build_key_types, 1, has_nullable_key, build_filter.is_some(), keep_filtered_rows, meta.nullMapLength);
    let mut chk = testutil::GenRandomChunks(build_types.clone(), 2049);
    if with_sel_col {
        chk.SetSel((0..chk.NumRows()).filter(|i| i % 3 != 0).collect());
    }
    let mut hash_join_ctx = HashJoinCtxV2 { hashTableMeta: meta.clone(), BuildFilter: build_filter, LUsedInOtherCondition: columns_used_by_other_condition.unwrap_or_default(), LUsed: output_columns, ..Default::default() };
    hash_join_ctx.Concurrency = 1;
    hash_join_ctx.SetupPartitionInfo();
    hash_join_ctx.initHashTableContext();
    hash_join_ctx.SessCtx = mock::NewContext();
    require::NoError(t, builder.processOneChunk(chk.clone(), hash_join_ctx.SessCtx.GetSessionVars().StmtCtx.TypeCtx(), &mut hash_join_ctx, 0), "processOneChunk returns error");
    let row_tables = hash_join_ctx.hashTableContext.rowTables[0].clone();
    check_row_location_alignment(t, row_tables.clone());
    let mut mock_join_prober = newMockJoinProbe(&hash_join_ctx);
    let mut result_chunk = chunk::NewEmptyChunk(result_types);
    result_chunk.SetInCompleteChunk(true);
    let mut tmp_chunk = chunk::NewEmptyChunk(build_types);
    tmp_chunk.SetInCompleteChunk(true);
    let has_other_condition_columns = !hash_join_ctx.LUsedInOtherCondition.is_empty();

    let mut row_index = 0;
    for (logical_index, physical_index) in builder.usedRows.iter().enumerate() {
        if !keep_filtered_rows && builder.is_filtered_or_null(*physical_index) {
            continue;
        }
        let pointer_index = if keep_filtered_rows { logical_index } else { row_index };
        let row_start = row_tables[0].getRowPointer(pointer_index);
        require::NotEqual(t, std::ptr::null_mut::<u8>(), row_start, "row start must not be nil");
        // Go 根据是否存在 other condition 列决定先写 tmpChunk 还是直接写 resultChunk。
        if has_other_condition_columns {
            mock_join_prober.appendBuildRowToCachedBuildRowsV1(0, row_start, &mut tmp_chunk, 0, true);
        } else {
            mock_join_prober.appendBuildRowToCachedBuildRowsV1(0, row_start, &mut result_chunk, 0, false);
        }
        row_index += 1;
    }
    if mock_join_prober.nextCachedBuildRowIndex > 0 {
        mock_join_prober.batchConstructBuildRows(if has_other_condition_columns { &mut tmp_chunk } else { &mut result_chunk }, 0, has_other_condition_columns);
    }
    if has_other_condition_columns {
        check_column_result(t, &builder, keep_filtered_rows, &tmp_chunk, &chk, &hash_join_ctx, true);
        mock_join_prober.selected = vec![true; tmp_chunk.NumRows()];
        require::NoError(t, mock_join_prober.buildResultAfterOtherCondition(&mut result_chunk, &tmp_chunk));
        check_column_result(t, &builder, keep_filtered_rows, &result_chunk, &chk, &hash_join_ctx, false);
    } else {
        check_column_result(t, &builder, keep_filtered_rows, &result_chunk, &chk, &hash_join_ctx, false);
    }
}

#[test]
pub fn test_columns_basic() {
    let types = column_basic_types();
    let column_used_by_other_conditions = vec![Some(vec![2, 3]), Some(vec![0, 2]), None];
    let output_columns = vec![vec![0, 1, 2, 3, 4, 5], vec![2, 3, 4, 5, 1, 0]];
    let filters = vec![Some(create_simple_filter(&mut testing::T::new())), None];
    for other_condition in column_used_by_other_conditions {
        for all_columns in output_columns.clone() {
            for keep in [true, false] {
                for used_flag in [true, false] {
                    for build_filter in filters.clone() {
                        for with_sel in [true, false] {
                            check_columns(&mut testing::T::new(), with_sel, build_filter.clone(), vec![0], types.not_null_build.clone(), types.not_null_key.clone(), types.not_null_key.clone(), keep, other_condition.clone(), all_columns.clone(), used_flag);
                            check_columns(&mut testing::T::new(), with_sel, build_filter.clone(), vec![0], types.nullable_build.clone(), types.nullable_key.clone(), types.nullable_key.clone(), keep, other_condition.clone(), all_columns.clone(), used_flag);
                        }
                    }
                }
            }
        }
    }
}

#[test]
pub fn test_columns_all_data_types() {
    let build_types = all_join_key_field_types_without_blob();
    let build_key_index = vec![0];
    let build_key_types = vec![build_types[0].clone()];
    let probe_key_types = build_key_types.clone();
    let output_columns: Vec<i32> = (0..build_types.len() as i32).collect();
    for keep in [true, false] {
        for used_flag in [true, false] {
            check_columns(&mut testing::T::new(), false, None, build_key_index.clone(), build_types.clone(), build_key_types.clone(), probe_key_types.clone(), keep, None, output_columns.clone(), used_flag);
            check_columns(&mut testing::T::new(), false, None, build_key_index.clone(), toNullableTypes(build_types.clone()), toNullableTypes(build_key_types.clone()), toNullableTypes(probe_key_types.clone()), keep, None, output_columns.clone(), used_flag);
        }
    }
}

#[test]
pub fn test_balance_of_filtered_rows() {
    // 所有行都被 impossible filter 过滤时，keepFilteredRows=true 仍需要按 partition 均衡保存。
    let int_tp = types::NewFieldType(mysql::TypeLonglong);
    let string_tp = types::NewFieldType(mysql::TypeVarString);
    let binary_string_tp = types::NewFieldType(mysql::TypeBlob);
    let build_types = vec![int_tp.clone(), string_tp, binary_string_tp];
    let build_key_types = vec![int_tp.clone()];
    let probe_key_types = vec![int_tp];
    let meta = newTableMeta(vec![0], build_types.clone(), build_key_types.clone(), probe_key_types, None, vec![], false);
    let build_filter = create_impossible_filter(&mut testing::T::new());
    let chk = testutil::GenRandomChunks(build_types.clone(), 3000);
    let mut hash_join_ctx = HashJoinCtxV2 { hashTableMeta: meta.clone(), BuildFilter: Some(build_filter), ..Default::default() };
    hash_join_ctx.Concurrency = 4;
    hash_join_ctx.SetupPartitionInfo();
    hash_join_ctx.initHashTableContext();
    hash_join_ctx.SessCtx = mock::NewContext();
    let mut builder = createRowTableBuilder(vec![0], build_key_types, hash_join_ctx.partitionNumber, true, true, true, meta.nullMapLength);
    require::NoError(&mut testing::T::new(), builder.processOneChunk(chk, hash_join_ctx.SessCtx.GetSessionVars().StmtCtx.TypeCtx(), &mut hash_join_ctx, 0));
    for i in 0..hash_join_ctx.partitionNumber as usize {
        require::Equal(&mut testing::T::new(), 3000 / hash_join_ctx.partitionNumber, hash_join_ctx.hashTableContext.rowTables[0][i].rowCount());
    }
}

#[test]
pub fn test_unalignment_load() {
    // Go 通过 unsafe 从非对齐地址读取 uint64/uint32/uint8；只保留字节布局和断言意图。
    let unalign_data: Vec<u8> = (0..20).map(|i| i as u8).collect();
    let mut align_data = Vec::new();
    for i in 0..10_u8 {
        align_data.extend([i, i + 1, i + 2, i + 3, i + 4, i + 5, i + 6, i + 7]);
    }
    require::True(&mut testing::T::new(), align_data.as_ptr() as usize % 4 == 0);
    for i in 0..10 {
        let v1 = unsafe_load_u64(&unalign_data[i..]);
        let v2 = unsafe_load_u64(&align_data[i * 8..]);
        require::Equal(&mut testing::T::new(), v1, v2);
        require::Equal(&mut testing::T::new(), unsafe_load_u32(&unalign_data[i..]), unsafe_load_u32(&align_data[i * 8..]));
        require::Equal(&mut testing::T::new(), unalign_data[i], align_data[i * 8]);
    }
}

#[test]
pub fn test_setup_partition_info() {
    struct TestCase { concurrency: u32, partition_number: u32, partition_mask_offset: i32 }
    let test_cases = vec![
        TestCase { concurrency: 1, partition_number: 1, partition_mask_offset: 64 },
        TestCase { concurrency: 2, partition_number: 2, partition_mask_offset: 63 },
        TestCase { concurrency: 3, partition_number: 4, partition_mask_offset: 62 },
        TestCase { concurrency: 4, partition_number: 4, partition_mask_offset: 62 },
        TestCase { concurrency: 5, partition_number: 8, partition_mask_offset: 61 },
        TestCase { concurrency: 8, partition_number: 8, partition_mask_offset: 61 },
        TestCase { concurrency: 9, partition_number: 16, partition_mask_offset: 60 },
        TestCase { concurrency: 16, partition_number: 16, partition_mask_offset: 60 },
        TestCase { concurrency: 100, partition_number: 16, partition_mask_offset: 60 },
    ];
    for test in test_cases {
        let mut hash_join_ctx = HashJoinCtxV2::default();
        hash_join_ctx.Concurrency = test.concurrency;
        hash_join_ctx.SetupPartitionInfo();
        require::Equal(&mut testing::T::new(), test.partition_number, hash_join_ctx.partitionNumber);
        require::Equal(&mut testing::T::new(), test.partition_mask_offset, hash_join_ctx.partitionMaskOffset);
    }
}
*/

use crate::join_table_meta::{FieldKind, FieldType, new_table_meta};
use crate::row_table_builder::{
    Chunk, RowTableBuilder, Value, calculate_fake_length, calculate_row_data_length,
};

/// 构造可空/非空的有符号整型字段类型。
fn int_type(nullable: bool) -> FieldType {
    FieldType {
        kind: FieldKind::SignedInt,
        fixed_length: Some(8),
        nullable,
    }
}

/// 构造可空/非空的文本字段类型（utf8mb4_bin）。
fn text_type(nullable: bool) -> FieldType {
    FieldType {
        kind: FieldKind::Text {
            collation: "utf8mb4_bin".into(),
        },
        fixed_length: None,
        nullable,
    }
}

/// 构造含 Int + Text 两列的 Join 表元数据。
fn meta(need_used_flag: bool) -> crate::join_table_meta::JoinTableMeta {
    let types = vec![int_type(true), text_type(true)];
    new_table_meta(
        &[0],
        &types,
        &[types[0].clone()],
        &[types[0].clone()],
        &[1],
        &[0, 1],
        need_used_flag,
    )
    .unwrap()
}

/// 含有效 key、NULL key、NULL 文本列的三行测试 chunk。
fn chunk() -> Chunk {
    vec![
        vec![Value::Int(1), Value::Text("one".into())],
        vec![Value::Null, Value::Text("null".into())],
        vec![Value::Int(3), Value::Null],
    ]
}

/// 序列化 key、分区写入后行字节按 8 对齐，有效 key 计数正确。
#[test]
fn row_table_builder_serializes_keys_partitions_and_aligns_rows() {
    let metadata = meta(true);
    let mut builder = RowTableBuilder::new(vec![0], 4, true, false, true, 1).unwrap();
    let table = builder.process_chunk(&chunk(), &metadata, None, 0).unwrap();
    assert_eq!(table.row_count(), 3);
    assert_eq!(table.valid_key_count(), 2);
    assert_eq!(builder.serialized_keys.len(), 3);
    assert_eq!(builder.partition_indices.len(), 3);
    assert!(
        table
            .segments()
            .iter()
            .flat_map(|segment| &segment.rows)
            .all(|row| row.bytes.len() % 8 == 0)
    );
}

/// `keep_filtered_rows` 控制过滤失败行是丢弃还是保留（valid_keys 标记）。
#[test]
fn row_table_builder_filter_can_drop_or_preserve_invalid_rows() {
    let metadata = meta(false);
    let filter = [true, false, true];
    let mut dropping = RowTableBuilder::new(vec![0], 2, true, true, false, 1).unwrap();
    let dropped = dropping
        .process_chunk(&chunk(), &metadata, Some(&filter), 0)
        .unwrap();
    assert_eq!(dropped.row_count(), 2);
    assert_eq!(dropped.valid_key_count(), 2);

    let mut keeping = RowTableBuilder::new(vec![0], 2, true, true, true, 1).unwrap();
    let kept = keeping
        .process_chunk(&chunk(), &metadata, Some(&filter), 0)
        .unwrap();
    assert_eq!(kept.row_count(), 3);
    assert_eq!(kept.valid_key_count(), 2);
    assert_eq!(keeping.valid_keys, vec![true, false, true]);
}

/// Go 对保留的过滤行使用递增假 hash，使它们在所有分区间轮询分布。
#[test]
fn filtered_rows_are_balanced_across_partitions_like_go() {
    let metadata = meta(false);
    let rows = (0..8)
        .map(|value| vec![Value::Int(value), Value::Text(value.to_string())])
        .collect::<Vec<_>>();
    let filter = [false; 8];
    let mut builder = RowTableBuilder::new(vec![0], 4, false, true, true, 1).unwrap();

    let table = builder
        .process_chunk(&rows, &metadata, Some(&filter), 0)
        .unwrap();

    assert_eq!(builder.hash_values, vec![0, 1, 2, 3, 0, 1, 2, 3]);
    assert_eq!(builder.partition_indices, vec![0, 1, 2, 3, 0, 1, 2, 3]);
    assert_eq!(
        table
            .segments()
            .iter()
            .map(|segment| segment.rows.len())
            .collect::<Vec<_>>(),
        vec![2, 2, 2, 2]
    );
}

/// Null map 的 bit 下标来自 row table 中的保存列顺序，而不是原 schema 下标。
#[test]
fn sparse_saved_columns_use_compact_null_map_positions() {
    let types = (0..10).map(|_| int_type(true)).collect::<Vec<_>>();
    let metadata = new_table_meta(
        &[0],
        &types,
        &[types[0].clone()],
        &[types[0].clone()],
        &[],
        &[9],
        false,
    )
    .unwrap();
    let mut row = vec![Value::Int(0); 10];
    row[9] = Value::Null;
    let mut builder = RowTableBuilder::new(vec![0], 1, false, false, false, 1).unwrap();

    let table = builder
        .process_chunk(&vec![row], &metadata, None, 0)
        .unwrap();
    let encoded = &table.segments()[0].rows[0];

    let saved_position = metadata
        .row_columns_order
        .iter()
        .position(|column| *column == 9)
        .unwrap();
    assert!(metadata.is_column_null(encoded, saved_position));
}

/// 恢复 chunk 时分区数不匹配应报错；匹配后可重新计算分区下标。
#[test]
fn restored_chunk_checks_partition_shape_and_regenerates_partition() {
    let metadata = meta(false);
    let mut builder = RowTableBuilder::new(vec![0], 4, true, false, true, 1).unwrap();
    assert!(
        builder
            .process_restored_chunk(&chunk(), &metadata, 2)
            .is_err()
    );
    let restored = builder
        .process_restored_chunk(&chunk(), &metadata, 4)
        .unwrap();
    assert_eq!(restored.row_count(), 3);
    let (hash, partition) = builder.regenerate_hash_and_partition(0xf0, 4).unwrap();
    assert_eq!(hash, 0xf0);
    assert_eq!(partition, 3);
}

/// 最大元素长度检查与 8 字节假填充对齐一致。
#[test]
fn row_table_builder_reports_maximum_element_size() {
    let metadata = meta(false);
    let builder = RowTableBuilder::new(vec![0], 1, true, false, true, 1).unwrap();
    let rows = chunk();
    let (fits, maximum) = builder.check_max_element_size(&rows, &metadata, usize::MAX);
    assert!(fits);
    assert!(maximum >= calculate_row_data_length(&metadata, &rows[0]));
    assert!(
        !builder
            .check_max_element_size(&rows, &metadata, maximum - 1)
            .0
    );
    assert_eq!((maximum + calculate_fake_length(maximum)) % 8, 0);
}

/// 分区数必须为非零 2 的幂，否则构造失败。
#[test]
fn row_table_builder_rejects_non_power_of_two_partition_count() {
    assert!(RowTableBuilder::new(vec![0], 0, false, false, false, 0).is_err());
    assert!(RowTableBuilder::new(vec![0], 3, false, false, false, 0).is_err());
}

#[test]
/// Rows with fewer columns than the metadata schema fail before producing a partial table.
fn row_table_builder_rejects_short_rows_without_partial_state() {
    let metadata = meta(false);
    let mut builder = RowTableBuilder::new(vec![0], 2, true, false, true, 1).unwrap();
    let short = vec![vec![Value::Int(1)]];
    assert!(builder.process_chunk(&short, &metadata, None, 0).is_err());
    assert_eq!(builder.hash_values.len(), 1);
    assert_eq!(builder.valid_keys.len(), 1);
}
