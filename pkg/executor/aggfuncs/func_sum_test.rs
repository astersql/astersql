// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// SUM 聚合函数测试。
//
// 块注释记录 Go `TestMergePartialResult4Sum` / `TestSum` / `TestMemSum`
// 及整数滑窗用例的映射；可执行测试覆盖本模块实际提供的 SUM 状态契约。

/*
// 以下为 Go 聚合框架场景的历史映射记录；对应可执行覆盖位于本文件及 crate 场景测试。
// test_merge_partial_result4_sum 对应 Go 的 TestMergePartialResult4Sum。
// 它覆盖 decimal、double、signed int 和 unsigned int 四类 sum partial result 合并结果。
#[test]
fn test_merge_partial_result4_sum() {
    let mut tests = vec![
        buildAggTester(
            ast::AggFuncSum,
            mysql::TypeNewDecimal,
            0,
            5,
            types::NewDecFromInt(10),
            types::NewDecFromInt(9),
            types::NewDecFromInt(19),
        ),
        buildAggTester(ast::AggFuncSum, mysql::TypeDouble, 0, 5, 10.0, 9.0, 19.0),
        buildAggTester(ast::AggFuncSumInt, mysql::TypeLonglong, 0, 5, 10, 9, 19),
    ];

    // Go 中单独构造 unsigned longlong FieldType 并追加用例；这里保留 AddFlag 的副作用语义。
    let mut unsigned_type = types::NewFieldType(mysql::TypeLonglong);
    unsigned_type.AddFlag(mysql::UnsignedFlag);
    tests.push(buildAggTesterWithFieldType(
        ast::AggFuncSumInt,
        unsigned_type,
        None::<()>,
        5,
        10_u64,
        9_u64,
        19_u64,
    ));

    for (i, test) in tests.into_iter().enumerate() {
        // Go t.Run 使用 funcName_i 命名子测试；保留索引，实际子测试框架后续再接。
        let _case_name = format!("{}_{}", test.funcName, i);
        testMergePartialResult(test);
    }
}

// test_sum 对应 Go 的 TestSum。
// 该测试验证普通 sum 聚合在 nil 初始状态下的最终结果，unsigned 分支仍通过 FieldType 标记表达。
#[test]
fn test_sum() {
    let mut tests = vec![
        buildAggTester(
            ast::AggFuncSum,
            mysql::TypeNewDecimal,
            0,
            5,
            None::<()>,
            types::NewDecFromInt(10),
        ),
        buildAggTester(ast::AggFuncSum, mysql::TypeDouble, 0, 5, None::<()>, 10.0),
        buildAggTester(ast::AggFuncSumInt, mysql::TypeLonglong, 0, 5, None::<()>, 10),
    ];

    let mut unsigned_type = types::NewFieldType(mysql::TypeLonglong);
    unsigned_type.AddFlag(mysql::UnsignedFlag);
    tests.push(buildAggTesterWithFieldType(
        ast::AggFuncSumInt,
        unsigned_type,
        None::<()>,
        5,
        None::<()>,
        10_u64,
    ));

    for (i, test) in tests.into_iter().enumerate() {
        let _case_name = format!("{}_{}", test.funcName, i);
        // testAggFunc 对应 Go 公共聚合校验器，负责输入行生成和最终结果比较。
        testAggFunc(test);
    }
}

// test_mem_sum 对应 Go 的 TestMemSum。
// 这里保留普通 sum 与 distinct sum 的 partial result 大小、hash set 桶内存和 update mem delta 生成器。
#[test]
fn test_mem_sum() {
    let tests = vec![
        buildAggMemTester(
            ast::AggFuncSum,
            mysql::TypeDouble,
            0,
            5,
            aggfuncs::DefPartialResult4SumFloat64Size,
            defaultUpdateMemDeltaGens,
            false,
        ),
        buildAggMemTester(
            ast::AggFuncSum,
            mysql::TypeNewDecimal,
            0,
            5,
            aggfuncs::DefPartialResult4SumDecimalSize,
            defaultUpdateMemDeltaGens,
            false,
        ),
        buildAggMemTester(
            ast::AggFuncSumInt,
            mysql::TypeLonglong,
            0,
            5,
            aggfuncs::DefPartialResult4SumInt64Size,
            defaultUpdateMemDeltaGens,
            false,
        ),
        buildAggMemTester(
            ast::AggFuncSum,
            mysql::TypeDouble,
            0,
            5,
            aggfuncs::DefPartialResult4SumDistinctFloat64Size
                + hack::DefBucketMemoryUsageForSetFloat64,
            distinctUpdateMemDeltaGens,
            true,
        ),
        buildAggMemTester(
            ast::AggFuncSum,
            mysql::TypeNewDecimal,
            mysql::TypeNewDecimal,
            5,
            aggfuncs::DefPartialResult4SumDistinctDecimalSize
                + hack::DefBucketMemoryUsageForSetString,
            distinctUpdateMemDeltaGens,
            true,
        ),
        buildAggMemTester(
            ast::AggFuncSumInt,
            mysql::TypeLonglong,
            0,
            5,
            aggfuncs::DefPartialResult4SumDistinctInt64Size
                + hack::DefBucketMemoryUsageForSetInt64,
            distinctUpdateMemDeltaGens,
            true,
        ),
    ];

    for (i, test) in tests.into_iter().enumerate() {
        let _case_name = format!("{}_{}", test.aggTest.funcName, i);
        // testAggMemFunc 对应 Go 内存增量校验器，不在这里实际采样内存。
        testAggMemFunc(test);
    }
}

// test_slide_sum_uint_process_out_window_first_to_avoid_overflow 对应 Go 的无符号滑动窗口溢出测试。
// 核心语义是先移出 out-window 值再加入 in-window 值，避免 maxUint64-1 + 2 的中间溢出。
#[test]
fn test_slide_sum_uint_process_out_window_first_to_avoid_overflow() {
    let ctx = mock::NewContext();

    let mut unsigned_type = types::NewFieldType(mysql::TypeLonglong);
    unsigned_type.AddFlag(mysql::UnsignedFlag);

    // Prepare input rows:
    // - last window: [maxUint64-1]
    // - next window: [2]
    // If we update by adding "in-window" values first, `maxUint64-1 + 2` will overflow even though the final
    // result after removing the outgoing value does not overflow.
    let mut src_chk = chunk::NewChunkWithCapacity(vec![unsigned_type], 2);
    let max_uint64 = !0_u64;
    src_chk.AppendUint64(0, max_uint64 - 1);
    src_chk.AppendUint64(0, 2);

    // Go 使用 expression.Column 指向第 0 列；这里保留 RetType 与 Index 的对应关系。
    let args = vec![expression::Column {
        RetType: unsigned_type,
        Index: 0,
    }];
    let desc = aggregation::NewAggFuncDesc(ctx, ast::AggFuncSumInt, args, false)
        .expect("Go require.NoError(t, err) for NewAggFuncDesc");

    let agg_func = aggfuncs::Build(ctx, desc, 0);
    // Go 类型断言为 SlidingWindowAggFunc；保留断言失败即测试失败的语义。
    let sliding_agg_func = agg_func
        .as_sliding_window()
        .expect("Go require.True(t, ok) for SlidingWindowAggFunc");

    let pr = agg_func.AllocPartialResult().0;
    agg_func.ResetPartialResult(pr);
    agg_func
        .UpdatePartialResult(ctx, vec![src_chk.GetRow(0)], pr)
        .expect("Go require.NoError(t, err) for initial UpdatePartialResult");

    let get_row = |i: u64| src_chk.GetRow(i as i32);
    // Slide 参数保持 原样：lastStart、lastEnd、shiftStart、shiftEnd 都描述窗口移动边界。
    sliding_agg_func
        .Slide(ctx, get_row, 0, 1, 1, 1, pr)
        .expect("Go require.NoError(t, err) for Slide");

    let mut result_chk = chunk::NewChunkWithCapacity(vec![desc.RetTp], 1);
    agg_func
        .AppendFinalResult2Chunk(ctx, pr, result_chk)
        .expect("Go require.NoError(t, err) for AppendFinalResult2Chunk");
    let result_row = result_chk.GetRow(0);
    require::False(result_row.IsNull(0));
    require::Equal(2_u64, result_row.GetUint64(0));
}

// test_slide_sum_int_process_out_window_first_to_avoid_overflow 对应 Go 的有符号滑动窗口溢出测试。
// 与无符号分支相同，测试意图是验证 Slide 先减旧窗口值，避免 maxInt64-1 + 2 的临时溢出。
#[test]
fn test_slide_sum_int_process_out_window_first_to_avoid_overflow() {
    let ctx = mock::NewContext();

    let signed_type = types::NewFieldType(mysql::TypeLonglong);

    // Prepare input rows:
    // - last window: [maxInt64-1]
    // - next window: [2]
    // If we update by adding "in-window" values first, `maxInt64-1 + 2` will overflow even though the final
    // result after removing the outgoing value does not overflow.
    let mut src_chk = chunk::NewChunkWithCapacity(vec![signed_type], 2);
    let max_int64 = (!0_u64 >> 1) as i64;
    src_chk.AppendInt64(0, max_int64 - 1);
    src_chk.AppendInt64(0, 2);

    let args = vec![expression::Column {
        RetType: signed_type,
        Index: 0,
    }];
    let desc = aggregation::NewAggFuncDesc(ctx, ast::AggFuncSumInt, args, false)
        .expect("Go require.NoError(t, err) for NewAggFuncDesc");

    let agg_func = aggfuncs::Build(ctx, desc, 0);
    let sliding_agg_func = agg_func
        .as_sliding_window()
        .expect("Go require.True(t, ok) for SlidingWindowAggFunc");

    let pr = agg_func.AllocPartialResult().0;
    agg_func.ResetPartialResult(pr);
    agg_func
        .UpdatePartialResult(ctx, vec![src_chk.GetRow(0)], pr)
        .expect("Go require.NoError(t, err) for initial UpdatePartialResult");

    let get_row = |i: u64| src_chk.GetRow(i as i32);
    sliding_agg_func
        .Slide(ctx, get_row, 0, 1, 1, 1, pr)
        .expect("Go require.NoError(t, err) for Slide");

    let mut result_chk = chunk::NewChunkWithCapacity(vec![desc.RetTp], 1);
    agg_func
        .AppendFinalResult2Chunk(ctx, pr, result_chk)
        .expect("Go require.NoError(t, err) for AppendFinalResult2Chunk");
    let result_row = result_chk.GetRow(0);
    require::False(result_row.IsNull(0));
    require::Equal(2_i64, result_row.GetInt64(0));
}
*/

/// 验证 FloatSum 累加、merge 与滑动窗口更新。
///
/// 路径：update(1,NULL,2,3)→6 → merge(4)→10 → slide 移出 1,2 移入 8 →15。
#[test]
fn float_sum_merges_and_slides_non_null_values() {
    let mut sum = crate::func_sum::FloatSum::default();
    sum.update([Some(1.0), None, Some(2.0), Some(3.0)]);
    assert_eq!(sum.value(), Some(6.0));
    let mut other = crate::func_sum::FloatSum::default();
    other.update([Some(4.0)]);
    sum.merge(&other);
    assert_eq!(sum.value(), Some(10.0));
    sum.slide([Some(1.0), Some(2.0)], [Some(8.0)]);
    assert_eq!(sum.value(), Some(15.0));
}

/// Go 的普通 Float SUM 逐项执行 `sum += value`，不会启用补偿求和。
#[test]
fn float_sum_preserves_go_sequential_addition() {
    let mut sum = crate::func_sum::FloatSum::default();
    sum.update([Some(1.0e16), Some(1.0), Some(-1.0e16)]);
    assert_eq!(sum.value(), Some(0.0));
}

/// Go 的 Float SUM 滑窗先加入窗口尾部，再移除窗口头部；浮点顺序可观察。
#[test]
fn float_sum_slide_preserves_go_update_order() {
    let mut sum = crate::func_sum::FloatSum::default();
    sum.update([Some(1.0e16)]);
    sum.slide([Some(1.0e16)], [Some(1.0)]);
    assert_eq!(sum.value(), Some(0.0));
}

/// Go Decimal SUM 也先加入 incoming；因此中间溢出必须在移除 outgoing 前返回。
#[test]
fn decimal_sum_slide_reports_go_ordered_intermediate_overflow() {
    use crate::func_sum::{Decimal, DecimalSum};

    let mut sum = DecimalSum::default();
    sum.update([Some(Decimal::new(i128::MAX - 1, 0))]).unwrap();
    let error = sum
        .slide(
            [Some(Decimal::new(i128::MAX - 1, 0))],
            [Some(Decimal::new(2, 0))],
        )
        .unwrap_err();
    assert!(error.0.contains("out of range"));
}

/// Go `map[float64]` 合并正负零，但因为 NaN 不等于自身而保留每次插入。
#[test]
fn distinct_float_sum_matches_go_float_key_equality() {
    let mut sum = crate::func_sum::DistinctFloatSum::default();
    sum.update([Some(0.0), Some(-0.0)]);
    assert_eq!(sum.len(), 1);

    let nan = f64::from_bits(0x7ff8_0000_0000_0001);
    sum.update([Some(nan), Some(nan)]);
    assert_eq!(sum.len(), 3);
    assert!(sum.value().unwrap().is_nan());
}

/// Go `MyDecimal::ToHashKey` 去除尾随零，数值相同但 scale 不同的值只保留一个。
#[test]
fn distinct_decimal_sum_matches_go_normalized_hash_key() {
    use crate::func_sum::{Decimal, DistinctDecimalSum};

    let mut sum = DistinctDecimalSum::default();
    sum.update([Some(Decimal::new(10, 1)), Some(Decimal::new(100, 2))]);
    assert_eq!(sum.len(), 1);
    assert_eq!(sum.value().unwrap(), Some(Decimal::new(10, 1)));
}
