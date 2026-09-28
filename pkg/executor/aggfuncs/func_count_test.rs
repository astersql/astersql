// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// COUNT / COUNT(DISTINCT) / APPROX_COUNT_DISTINCT 单元测试。
//
// 块注释内保留 Go 迁移草稿（merge、多类型执行、内存、WriteTime、benchmark）；
// 可执行部分覆盖 COUNT 的 NULL/partial/slide，以及多列 DISTINCT 跳过含 NULL 行。

/*
// 这段逻辑覆盖 COUNT、APPROX_COUNT_DISTINCT 的 partial result、执行、内存、时间编码和 benchmark 用例。

// gen_approx_distinct_merge_partial_result 对应 Go 的 genApproxDistinctMergePartialResult。
// Go 代码用小端 uint64 字节做 farm.Hash64，再插入 ApproxCountDistinct partial result 后序列化为 string。
pub fn gen_approx_distinct_merge_partial_result(begin: u64, end: u64) -> String {
    let mut partial = aggfuncs::NewPartialResult4ApproxCountDistinct();
    let mut encoded_bytes = [0_u8; 8];

    for i in begin..end {
        // encoding/binary.LittleEndian.PutUint64 在 实现中用 to_le_bytes 表达。
        encoded_bytes.copy_from_slice(&i.to_le_bytes());
        let hash = farm::Hash64(&encoded_bytes);
        partial.InsertHash64(hash);
    }

    // Go 返回 string(o.Serialize())；这里保留从序列化字节到字符串的占位转换。
    String::from_utf8_lossy(&partial.Serialize()).to_string()
}

// test_merge_partial_result_4_count 对应 Go 的 TestMergePartialResult4Count。
#[test]
fn test_merge_partial_result_4_count() {
    let tester = buildAggTester(ast::AggFuncCount, mysql::TypeLonglong, 0, 5, 5, 3, 8);
    testMergePartialResult(tester);

    let tester = buildAggTester(
        ast::AggFuncApproxCountDistinct,
        mysql::TypeLonglong,
        0,
        5,
        gen_approx_distinct_merge_partial_result(0, 5),
        gen_approx_distinct_merge_partial_result(2, 5),
        5,
    );
    // 第二个用例保留 Go 中两个近似去重 partial result 合并后基数为 5 的语义。
    testMergePartialResult(tester);
}

// test_count 对应 Go 的 TestCount，按原顺序覆盖单参数 COUNT、多参数 COUNT、重复单参路径和近似去重。
#[test]
fn test_count() {
    let tests = vec![
        buildAggTester(ast::AggFuncCount, mysql::TypeLonglong, 0, 5, 0, 5),
        buildAggTester(ast::AggFuncCount, mysql::TypeFloat, 0, 5, 0, 5),
        buildAggTester(ast::AggFuncCount, mysql::TypeDouble, 0, 5, 0, 5),
        buildAggTester(ast::AggFuncCount, mysql::TypeNewDecimal, 0, 5, 0, 5),
        buildAggTester(ast::AggFuncCount, mysql::TypeString, 0, 5, 0, 5),
        buildAggTester(ast::AggFuncCount, mysql::TypeDate, 0, 5, 0, 5),
        buildAggTester(ast::AggFuncCount, mysql::TypeDuration, 0, 5, 0, 5),
        buildAggTester(ast::AggFuncCount, mysql::TypeJSON, 0, 5, 0, 5),
    ];
    for (i, test) in tests.into_iter().enumerate() {
        // Go 的 t.Run(fmt.Sprintf("%s_%d", test.funcName, i), ...) 在这里用注释保留子测试命名语义。
        let _case_name = format!("{}_{}", test.funcName, i);
        testAggFunc(test);
    }

    let tests2 = vec![
        buildMultiArgsAggTester(
            ast::AggFuncCount,
            vec![mysql::TypeLonglong, mysql::TypeLonglong],
            mysql::TypeLonglong,
            5,
            0,
            5,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncCount,
            vec![mysql::TypeFloat, mysql::TypeFloat],
            mysql::TypeLonglong,
            5,
            0,
            5,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncCount,
            vec![mysql::TypeDouble, mysql::TypeDouble],
            mysql::TypeLonglong,
            5,
            0,
            5,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncCount,
            vec![mysql::TypeNewDecimal, mysql::TypeNewDecimal],
            mysql::TypeLonglong,
            5,
            0,
            5,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncCount,
            vec![mysql::TypeString, mysql::TypeString],
            mysql::TypeLonglong,
            5,
            0,
            5,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncCount,
            vec![mysql::TypeDate, mysql::TypeDate],
            mysql::TypeLonglong,
            5,
            0,
            5,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncCount,
            vec![mysql::TypeDuration, mysql::TypeDuration],
            mysql::TypeLonglong,
            5,
            0,
            5,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncCount,
            vec![mysql::TypeJSON, mysql::TypeJSON],
            mysql::TypeLonglong,
            5,
            0,
            5,
        ),
    ];
    for (i, test) in tests2.into_iter().enumerate() {
        let _case_name = format!("{}_{}", test.funcName, i);
        // mock.NewContext 是 Go helper 的执行上下文；这里只保留该外部依赖入口。
        testMultiArgsAggFunc(mock::NewContext(), test);
    }

    let tests3 = vec![
        buildAggTester(ast::AggFuncCount, mysql::TypeLonglong, 0, 5, 0, 5),
        buildAggTester(ast::AggFuncCount, mysql::TypeFloat, 0, 5, 0, 5),
        buildAggTester(ast::AggFuncCount, mysql::TypeDouble, 0, 5, 0, 5),
        buildAggTester(ast::AggFuncCount, mysql::TypeNewDecimal, 0, 5, 0, 5),
        buildAggTester(ast::AggFuncCount, mysql::TypeString, 0, 5, 0, 5),
        buildAggTester(ast::AggFuncCount, mysql::TypeDate, 0, 5, 0, 5),
        buildAggTester(ast::AggFuncCount, mysql::TypeDuration, 0, 5, 0, 5),
        buildAggTester(ast::AggFuncCount, mysql::TypeJSON, 0, 5, 0, 5),
    ];
    for test in tests3 {
        // 保留 Go 文件中第二组单参数 COUNT 回归路径。
        testAggFunc(test);
    }

    let tests4 = vec![
        buildMultiArgsAggTester(
            ast::AggFuncApproxCountDistinct,
            vec![mysql::TypeLonglong, mysql::TypeLonglong],
            mysql::TypeLonglong,
            5,
            0,
            5,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncApproxCountDistinct,
            vec![mysql::TypeFloat, mysql::TypeFloat],
            mysql::TypeLonglong,
            5,
            0,
            5,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncApproxCountDistinct,
            vec![mysql::TypeDouble, mysql::TypeDouble],
            mysql::TypeLonglong,
            5,
            0,
            5,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncApproxCountDistinct,
            vec![mysql::TypeNewDecimal, mysql::TypeNewDecimal],
            mysql::TypeLonglong,
            5,
            0,
            5,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncApproxCountDistinct,
            vec![mysql::TypeString, mysql::TypeString],
            mysql::TypeLonglong,
            5,
            0,
            5,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncApproxCountDistinct,
            vec![mysql::TypeDate, mysql::TypeDate],
            mysql::TypeLonglong,
            5,
            0,
            5,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncApproxCountDistinct,
            vec![mysql::TypeDuration, mysql::TypeDuration],
            mysql::TypeLonglong,
            5,
            0,
            5,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncApproxCountDistinct,
            vec![mysql::TypeJSON, mysql::TypeJSON],
            mysql::TypeLonglong,
            5,
            0,
            5,
        ),
    ];
    for (i, test) in tests4.into_iter().enumerate() {
        let _case_name = format!("{}_{}", test.funcName, i);
        testMultiArgsAggFunc(mock::NewContext(), test);
    }
}

// test_mem_count 对应 Go 的 TestMemCount，保留普通 COUNT、DISTINCT COUNT 和 APPROX_COUNT_DISTINCT 的内存用例。
#[test]
fn test_mem_count() {
    let tests = vec![
        buildAggMemTester(
            ast::AggFuncCount,
            mysql::TypeLonglong,
            0,
            5,
            aggfuncs::DefPartialResult4CountSize,
            defaultUpdateMemDeltaGens,
            false,
        ),
        buildAggMemTester(
            ast::AggFuncCount,
            mysql::TypeFloat,
            0,
            5,
            aggfuncs::DefPartialResult4CountSize,
            defaultUpdateMemDeltaGens,
            false,
        ),
        buildAggMemTester(
            ast::AggFuncCount,
            mysql::TypeDouble,
            0,
            5,
            aggfuncs::DefPartialResult4CountSize,
            defaultUpdateMemDeltaGens,
            false,
        ),
        buildAggMemTester(
            ast::AggFuncCount,
            mysql::TypeNewDecimal,
            0,
            5,
            aggfuncs::DefPartialResult4CountSize,
            defaultUpdateMemDeltaGens,
            false,
        ),
        buildAggMemTester(
            ast::AggFuncCount,
            mysql::TypeString,
            0,
            5,
            aggfuncs::DefPartialResult4CountSize,
            defaultUpdateMemDeltaGens,
            false,
        ),
        buildAggMemTester(
            ast::AggFuncCount,
            mysql::TypeDate,
            0,
            5,
            aggfuncs::DefPartialResult4CountSize,
            defaultUpdateMemDeltaGens,
            false,
        ),
        buildAggMemTester(
            ast::AggFuncCount,
            mysql::TypeDuration,
            0,
            5,
            aggfuncs::DefPartialResult4CountSize,
            defaultUpdateMemDeltaGens,
            false,
        ),
        buildAggMemTester(
            ast::AggFuncCount,
            mysql::TypeLonglong,
            0,
            5,
            aggfuncs::DefPartialResult4CountDistinctIntSize + hack::DefBucketMemoryUsageForSetInt64,
            distinctUpdateMemDeltaGens,
            true,
        ),
        buildAggMemTester(
            ast::AggFuncCount,
            mysql::TypeFloat,
            0,
            5,
            aggfuncs::DefPartialResult4CountDistinctRealSize
                + hack::DefBucketMemoryUsageForSetFloat64,
            distinctUpdateMemDeltaGens,
            true,
        ),
        buildAggMemTester(
            ast::AggFuncCount,
            mysql::TypeDouble,
            0,
            5,
            aggfuncs::DefPartialResult4CountDistinctRealSize
                + hack::DefBucketMemoryUsageForSetFloat64,
            distinctUpdateMemDeltaGens,
            true,
        ),
        buildAggMemTester(
            ast::AggFuncCount,
            mysql::TypeNewDecimal,
            0,
            5,
            aggfuncs::DefPartialResult4CountDistinctDecimalSize
                + hack::DefBucketMemoryUsageForSetString,
            distinctUpdateMemDeltaGens,
            true,
        ),
        buildAggMemTester(
            ast::AggFuncCount,
            mysql::TypeString,
            0,
            5,
            aggfuncs::DefPartialResult4CountDistinctStringSize
                + hack::DefBucketMemoryUsageForSetString,
            distinctUpdateMemDeltaGens,
            true,
        ),
        buildAggMemTester(
            ast::AggFuncCount,
            mysql::TypeDate,
            0,
            5,
            aggfuncs::DefPartialResult4CountWithDistinctSize
                + hack::DefBucketMemoryUsageForSetString,
            distinctUpdateMemDeltaGens,
            true,
        ),
        buildAggMemTester(
            ast::AggFuncCount,
            mysql::TypeDuration,
            0,
            5,
            aggfuncs::DefPartialResult4CountDistinctDurationSize
                + hack::DefBucketMemoryUsageForSetInt64,
            distinctUpdateMemDeltaGens,
            true,
        ),
        buildAggMemTester(
            ast::AggFuncCount,
            mysql::TypeJSON,
            0,
            5,
            aggfuncs::DefPartialResult4CountWithDistinctSize
                + hack::DefBucketMemoryUsageForSetString,
            distinctUpdateMemDeltaGens,
            true,
        ),
        buildAggMemTester(
            ast::AggFuncApproxCountDistinct,
            mysql::TypeLonglong,
            0,
            5,
            aggfuncs::DefPartialResult4ApproxCountDistinctSize,
            approxCountDistinctUpdateMemDeltaGens,
            true,
        ),
        buildAggMemTester(
            ast::AggFuncApproxCountDistinct,
            mysql::TypeString,
            0,
            5,
            aggfuncs::DefPartialResult4ApproxCountDistinctSize,
            approxCountDistinctUpdateMemDeltaGens,
            true,
        ),
    ];

    for (i, test) in tests.into_iter().enumerate() {
        // Go 子测试名读取 test.aggTest.funcName；这里保留该字段访问，提示后续接线要维持嵌套结构。
        let _case_name = format!("{}_{}", test.aggTest.funcName, i);
        testAggMemFunc(test);
    }
}

// test_write_time 对应 Go 的 TestWriteTime，验证 WriteTime 会覆盖初始化为 255 的缓冲区。
#[test]
fn test_write_time() {
    let tt = types::ParseDate(types::DefaultStmtNoWarningContext, "2020-11-11")
        .expect("Go require.NoError 对应这里的解析成功断言");

    let mut buf = [255_u8; 16];
    aggfuncs::WriteTime(&mut buf, tt);
    for byte in buf {
        // Go require.False(t, buf[i] == uint8(255)) 表示每个位置都应被时间编码写入。
        assert!(byte != 255);
    }
}

// benchmark_count 对应 Go 的 BenchmarkCount。
// 这里不运行 Rust benchmark，仅保存单参数、多参数和近似去重三组 benchmark 的调用顺序。
pub fn benchmark_count() {
    let ctx = mock::NewContext();
    let row_num = 50_000;
    let tests = vec![
        buildAggTester(
            ast::AggFuncCount,
            mysql::TypeLonglong,
            0,
            row_num,
            0,
            row_num,
        ),
        buildAggTester(ast::AggFuncCount, mysql::TypeFloat, 0, row_num, 0, row_num),
        buildAggTester(ast::AggFuncCount, mysql::TypeDouble, 0, row_num, 0, row_num),
        buildAggTester(
            ast::AggFuncCount,
            mysql::TypeNewDecimal,
            0,
            row_num,
            0,
            row_num,
        ),
        buildAggTester(ast::AggFuncCount, mysql::TypeString, 0, row_num, 0, row_num),
        buildAggTester(ast::AggFuncCount, mysql::TypeDate, 0, row_num, 0, row_num),
        buildAggTester(
            ast::AggFuncCount,
            mysql::TypeDuration,
            0,
            row_num,
            0,
            row_num,
        ),
        buildAggTester(ast::AggFuncCount, mysql::TypeJSON, 0, row_num, 0, row_num),
    ];
    for test in tests {
        benchmarkAggFunc(&ctx, test);
    }

    let tests2 = vec![
        buildMultiArgsAggTester(
            ast::AggFuncCount,
            vec![mysql::TypeLonglong, mysql::TypeLonglong],
            mysql::TypeLonglong,
            row_num,
            0,
            row_num,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncCount,
            vec![mysql::TypeFloat, mysql::TypeFloat],
            mysql::TypeLonglong,
            row_num,
            0,
            row_num,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncCount,
            vec![mysql::TypeDouble, mysql::TypeDouble],
            mysql::TypeLonglong,
            row_num,
            0,
            row_num,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncCount,
            vec![mysql::TypeNewDecimal, mysql::TypeNewDecimal],
            mysql::TypeLonglong,
            row_num,
            0,
            row_num,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncCount,
            vec![mysql::TypeString, mysql::TypeString],
            mysql::TypeLonglong,
            row_num,
            0,
            row_num,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncCount,
            vec![mysql::TypeDate, mysql::TypeDate],
            mysql::TypeLonglong,
            row_num,
            0,
            row_num,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncCount,
            vec![mysql::TypeDuration, mysql::TypeDuration],
            mysql::TypeLonglong,
            row_num,
            0,
            row_num,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncCount,
            vec![mysql::TypeJSON, mysql::TypeJSON],
            mysql::TypeLonglong,
            row_num,
            0,
            row_num,
        ),
    ];
    for test in tests2 {
        benchmarkMultiArgsAggFunc(&ctx, test);
    }

    let tests3 = vec![
        buildMultiArgsAggTester(
            ast::AggFuncApproxCountDistinct,
            vec![mysql::TypeLonglong, mysql::TypeLonglong],
            mysql::TypeLonglong,
            row_num,
            0,
            row_num,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncApproxCountDistinct,
            vec![mysql::TypeFloat, mysql::TypeFloat],
            mysql::TypeLonglong,
            row_num,
            0,
            row_num,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncApproxCountDistinct,
            vec![mysql::TypeDouble, mysql::TypeDouble],
            mysql::TypeLonglong,
            row_num,
            0,
            row_num,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncApproxCountDistinct,
            vec![mysql::TypeNewDecimal, mysql::TypeNewDecimal],
            mysql::TypeLonglong,
            row_num,
            0,
            row_num,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncApproxCountDistinct,
            vec![mysql::TypeString, mysql::TypeString],
            mysql::TypeLonglong,
            row_num,
            0,
            row_num,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncApproxCountDistinct,
            vec![mysql::TypeDate, mysql::TypeDate],
            mysql::TypeLonglong,
            row_num,
            0,
            row_num,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncApproxCountDistinct,
            vec![mysql::TypeDuration, mysql::TypeDuration],
            mysql::TypeLonglong,
            row_num,
            0,
            row_num,
        ),
        buildMultiArgsAggTester(
            ast::AggFuncApproxCountDistinct,
            vec![mysql::TypeJSON, mysql::TypeJSON],
            mysql::TypeLonglong,
            row_num,
            0,
            row_num,
        ),
    ];
    for test in tests3 {
        benchmarkMultiArgsAggFunc(&ctx, test);
    }
}
*/

use crate::func_count::CountAggregator;
use crate::func_count_distinct::{CountDistinctMulti, DistinctValue};

/// 校验 COUNT 忽略 NULL、合并 partial、滑动窗口，以及多列 DISTINCT 去重语义。
#[test]
fn count_handles_null_partial_merge_slide_and_multi_distinct() {
    // 两个非空 + partial 3 => 5；slide 移出非空 1、移入 9 后仍为 5（None 移出不减）。
    let mut count = CountAggregator::default();
    count.update([Some(1), None, Some(2)]).unwrap();
    count.update_partial([Some(3), None]).unwrap();
    assert_eq!(count.value(), 5);
    count.slide([Some(1), None], [Some(9)]).unwrap();
    assert_eq!(count.value(), 5);

    // 两行相同 (1,"a") 去重为 1；含 None 的行整行丢弃。
    let mut distinct = CountDistinctMulti::default();
    distinct
        .update([
            vec![
                Some(DistinctValue::Int(1)),
                Some(DistinctValue::String(b"a".to_vec())),
            ],
            vec![
                Some(DistinctValue::Int(1)),
                Some(DistinctValue::String(b"a".to_vec())),
            ],
            vec![Some(DistinctValue::Int(2)), None],
        ])
        .unwrap();
    assert_eq!(distinct.count(), 1);
}

/// Go 的 int64 聚合计数使用补码回绕；Rust 不应在边界处引入额外错误契约。
#[test]
fn count_wraps_like_go_int64_arithmetic() {
    let mut count = CountAggregator::default();
    count.update_partial([Some(i64::MAX)]).unwrap();
    count.update([Some(())]).unwrap();
    assert_eq!(count.value(), i64::MIN);

    count.reset();
    count
        .slide([Some(())], std::iter::empty::<Option<()>>())
        .unwrap();
    assert_eq!(count.value(), -1);
}
