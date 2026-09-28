// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// GROUP_CONCAT 聚合的单元测试。
//
// 可执行用例覆盖 DISTINCT 去重、NULL 跳过、分隔符拼接与最大长度截断。
// 大段注释保留 Go 侧 merge / order by / 内存增量等测试草稿。

/*
// codec、hack、set 等均保留为 Go 语义占位，方便后续逐步接入 Rust 侧测试框架。

// TestMergePartialResult4GroupConcat 对应 Go 测试：验证两个 partial 结果按分隔符拼接。
#[test]
pub fn test_merge_partial_result_4_group_concat() {
    let test = buildAggTester(
        ast::AggFuncGroupConcat,
        mysql::TypeString,
        0,
        5,
        "0 1 2 3 4",
        "2 3 4",
        "0 1 2 3 4 2 3 4",
    );
    testMergePartialResult(test);
}

// TestGroupConcat 对应 Go 测试：覆盖普通、多参数排序和 GroupConcatMaxLen 截断场景。
#[test]
pub fn test_group_concat() {
    let ctx = mock::NewContext();

    let test = buildAggTester(ast::AggFuncGroupConcat, mysql::TypeString, 0, 5, Nil, "0 1 2 3 4");
    testAggFuncWithoutDistinct(test);

    let mut test2 = buildMultiArgsAggTester(
        ast::AggFuncGroupConcat,
        vec![mysql::TypeString, mysql::TypeString],
        mysql::TypeString,
        5,
        Nil,
        "44 33 22 11 00",
    );
    test2.orderBy = true;
    testMultiArgsAggFunc(&ctx, test2);

    // Go 版 defer 在测试末尾恢复系统变量；用显式作用域说明资源收尾语义。
    let restore_group_concat_max_len = || {
        ctx.GetSessionVars().SetSystemVar(vardef::GroupConcatMaxLen, "1024")
    };

    // minimum GroupConcatMaxLen is 4
    for i in 4..=7 {
        let err = ctx.GetSessionVars().SetSystemVar(vardef::GroupConcatMaxLen, &i.to_string());
        require::NoError(err);
        let expected = &"44 33 22 11 00"[..i];
        test2 = buildMultiArgsAggTester(
            ast::AggFuncGroupConcat,
            vec![mysql::TypeString, mysql::TypeString],
            mysql::TypeString,
            5,
            Nil,
            expected,
        );
        test2.orderBy = true;
        testMultiArgsAggFunc(&ctx, test2);
    }

    let _ = restore_group_concat_max_len();
}

// TestMemGroupConcat 对应 Go 测试：组合 distinct/order by 两个维度验证多参数内存估算。
#[test]
pub fn test_mem_group_concat() {
    let multi_args_test1 = buildMultiArgsAggMemTester(
        ast::AggFuncGroupConcat,
        vec![mysql::TypeString, mysql::TypeString],
        mysql::TypeString,
        5,
        aggfuncs::DefPartialResult4GroupConcatSize + aggfuncs::DefBytesBufferSize,
        group_concat_multi_args_update_mem_delta_gens,
        false,
    );
    let multi_args_test2 = buildMultiArgsAggMemTester(
        ast::AggFuncGroupConcat,
        vec![mysql::TypeString, mysql::TypeString],
        mysql::TypeString,
        5,
        aggfuncs::DefPartialResult4GroupConcatDistinctSize
            + aggfuncs::DefBytesBufferSize
            + hack::DefBucketMemoryUsageForMapStringToString,
        group_concat_distinct_multi_args_update_mem_delta_gens,
        true,
    );

    let mut multi_args_test3 = buildMultiArgsAggMemTester(
        ast::AggFuncGroupConcat,
        vec![mysql::TypeString, mysql::TypeString],
        mysql::TypeString,
        5,
        aggfuncs::DefPartialResult4GroupConcatOrderSize + aggfuncs::DefTopNRowsSize,
        group_concat_order_multi_args_update_mem_delta_gens,
        false,
    );
    multi_args_test3.multiArgsAggTest.orderBy = true;

    let mut multi_args_test4 = buildMultiArgsAggMemTester(
        ast::AggFuncGroupConcat,
        vec![mysql::TypeString, mysql::TypeString],
        mysql::TypeString,
        5,
        aggfuncs::DefPartialResult4GroupConcatOrderDistinctSize
            + aggfuncs::DefTopNRowsSize
            + hack::DefBucketMemoryUsageForSetString,
        group_concat_distinct_order_multi_args_update_mem_delta_gens,
        true,
    );
    multi_args_test4.multiArgsAggTest.orderBy = true;

    let multi_args_tests = vec![multi_args_test1, multi_args_test2, multi_args_test3, multi_args_test4];
    for (i, test) in multi_args_tests.into_iter().enumerate() {
        let _case_name = format!("{}_{}", test.multiArgsAggTest.funcName, i);
        testMultiArgsAggMemFunc(test);
    }
}

// groupConcatMultiArgsUpdateMemDeltaGens 对应 Go helper：估算非 distinct、非 order 的 buffer 扩容。
pub fn group_concat_multi_args_update_mem_delta_gens(
    _ctx: sessionctx::Context,
    src_chk: &chunk::Chunk,
    data_type: Vec<types::FieldType>,
    _by_items: Vec<util::ByItems>,
) -> Result<Vec<i64>, errors::Error> {
    let mut mem_deltas = Vec::new();
    let mut buffer = bytes::Buffer::new();
    let mut val_buffer = bytes::Buffer::new();

    for i in 0..src_chk.NumRows() {
        val_buffer.Reset();
        let row = src_chk.GetRow(i);
        if row.IsNull(0) {
            mem_deltas.push(0);
            continue;
        }

        let old_mem_size = buffer.Cap() + val_buffer.Cap();
        if i != 0 {
            // Go GROUP_CONCAT 在非首行前写入全局 separator。
            buffer.WriteString(separator);
        }
        for j in 0..data_type.len() {
            let cur_val = row.GetString(j);
            val_buffer.WriteString(cur_val);
        }
        buffer.WriteString(val_buffer.String());

        let mut mem_delta = (buffer.Cap() + val_buffer.Cap() - old_mem_size) as i64;
        if i == 0 {
            mem_delta += aggfuncs::DefBytesBufferSize;
        }
        mem_deltas.push(mem_delta);
    }

    Ok(mem_deltas)
}

// groupConcatOrderMultiArgsUpdateMemDeltaGens 对应 Go helper：order by 场景会额外保存排序键 datum。
pub fn group_concat_order_multi_args_update_mem_delta_gens(
    ctx: sessionctx::Context,
    src_chk: &chunk::Chunk,
    data_type: Vec<types::FieldType>,
    by_items: Vec<util::ByItems>,
) -> Result<Vec<i64>, errors::Error> {
    let mut mem_deltas = Vec::new();
    for i in 0..src_chk.NumRows() {
        let mut buffer = bytes::Buffer::new();
        let row = src_chk.GetRow(i);
        if row.IsNull(0) {
            mem_deltas.push(0);
            continue;
        }

        let old_mem_size = buffer.Cap();
        for j in 0..data_type.len() {
            let cur_val = row.GetString(j);
            buffer.WriteString(cur_val);
        }

        let mut mem_delta = (buffer.Cap() - old_mem_size) as i64;
        for by_item in by_items.iter() {
            // Go 在这里调用表达式 Eval；保留外部表达式求值依赖和内存计量点。
            let fdt = by_item.Expr.Eval(ctx.GetExprCtx().GetEvalCtx(), row).0;
            mem_delta += aggfuncs::GetDatumMemSize(&fdt);
        }
        mem_deltas.push(mem_delta);
    }

    Ok(mem_deltas)
}

// groupConcatDistinctMultiArgsUpdateMemDeltaGens 对应 Go helper：用编码后的多参数串去重并计入 map key。
pub fn group_concat_distinct_multi_args_update_mem_delta_gens(
    _ctx: sessionctx::Context,
    src_chk: &chunk::Chunk,
    data_type: Vec<types::FieldType>,
    _by_items: Vec<util::ByItems>,
) -> Result<Vec<i64>, errors::Error> {
    let mut mem_deltas = Vec::new();
    let mut val_set = set::NewStringSet();
    let mut buffer = bytes::Buffer::new();
    let mut vals_buf = bytes::Buffer::new();
    let mut encode_bytes_buffer: Vec<u8> = Vec::new();

    for i in 0..src_chk.NumRows() {
        let row = src_chk.GetRow(i);
        if row.IsNull(0) {
            mem_deltas.push(0);
            continue;
        }
        vals_buf.Reset();
        let old_mem_size = vals_buf.Cap() + encode_bytes_buffer.capacity();
        encode_bytes_buffer.clear();

        for j in 0..data_type.len() {
            let cur_val = row.GetString(j);
            encode_bytes_buffer = codec::EncodeBytes(encode_bytes_buffer, hack::Slice(cur_val));
            vals_buf.WriteString(cur_val);
        }

        let joined_val = String::from_utf8_lossy(&encode_bytes_buffer).to_string();
        if val_set.Exist(&joined_val) {
            // 重复 key 不会扩展 GROUP_CONCAT distinct 的结果缓冲。
            mem_deltas.push(0);
            continue;
        }
        val_set.Insert(joined_val.clone());
        if i != 0 {
            buffer.WriteString(separator);
        }
        let val_str = vals_buf.String();
        buffer.WriteString(&val_str);
        let mem_delta = (joined_val.len()
            + val_str.len()
            + vals_buf.Cap()
            + encode_bytes_buffer.capacity()
            - old_mem_size) as i64;
        mem_deltas.push(mem_delta);
    }

    Ok(mem_deltas)
}

// groupConcatDistinctOrderMultiArgsUpdateMemDeltaGens 对应 Go helper：distinct 与 order by 同时存在。
pub fn group_concat_distinct_order_multi_args_update_mem_delta_gens(
    ctx: sessionctx::Context,
    src_chk: &chunk::Chunk,
    data_type: Vec<types::FieldType>,
    by_items: Vec<util::ByItems>,
) -> Result<Vec<i64>, errors::Error> {
    let mut mem_deltas = Vec::new();
    let mut val_set = set::NewStringSet();
    let mut encode_bytes_buffer: Vec<u8> = Vec::new();

    for i in 0..src_chk.NumRows() {
        let mut vals_buf = bytes::Buffer::new();
        let row = src_chk.GetRow(i);
        if row.IsNull(0) {
            mem_deltas.push(0);
            continue;
        }

        vals_buf.Reset();
        encode_bytes_buffer.clear();
        let old_mem_size = vals_buf.Cap() + encode_bytes_buffer.capacity();
        for j in 0..data_type.len() {
            let cur_val = row.GetString(j);
            encode_bytes_buffer = codec::EncodeBytes(encode_bytes_buffer, hack::Slice(cur_val));
            vals_buf.WriteString(cur_val);
        }

        let joined_val = String::from_utf8_lossy(&encode_bytes_buffer).to_string();
        if val_set.Exist(&joined_val) {
            mem_deltas.push(0);
            continue;
        }
        val_set.Insert(joined_val.clone());

        let mut mem_delta =
            (joined_val.len() + vals_buf.Cap() + encode_bytes_buffer.capacity() - old_mem_size) as i64;
        for by_item in by_items.iter() {
            // order distinct 需要为每条新值保存排序 key；这部分来自 Go 的 Expr.Eval 调用。
            let fdt = by_item.Expr.Eval(ctx.GetExprCtx().GetEvalCtx(), row).0;
            mem_delta += aggfuncs::GetDatumMemSize(&fdt);
        }
        mem_deltas.push(mem_delta);
    }

    Ok(mem_deltas)
}
*/

use crate::func_group_concat::GroupConcat;

/// 校验 DISTINCT+分隔符拼接，以及非 DISTINCT 下超长截断与 truncated 标志。
#[test]
fn group_concat_applies_separator_distinct_and_maximum_length() {
    // DISTINCT：重复 "a" 只保留一次；NULL 被跳过；结果为 "a,b"。
    let mut concat = GroupConcat::new(b",".to_vec(), 64, true);
    concat.update([
        Some(b"a".to_vec()),
        None,
        Some(b"a".to_vec()),
        Some(b"b".to_vec()),
    ]);
    assert_eq!(concat.result(), Some(b"a,b".as_slice()));
    assert!(!concat.truncated());

    // maximum_len=4："ab|cd" 截成 "ab|c" 并标记 truncated。
    let mut truncated = GroupConcat::new(b"|".to_vec(), 4, false);
    truncated.update([Some(b"ab".to_vec()), Some(b"cd".to_vec())]);
    assert_eq!(truncated.result(), Some(b"ab|c".as_slice()));
    assert!(truncated.truncated());
}

/// Go `groupConcat` keeps a non-NULL empty string distinct from an empty group.
/// It also treats max_len=0 as unlimited and preserves separators when a
/// partial result containing an empty value is merged.
#[test]
fn group_concat_preserves_empty_non_null_values_and_zero_limit() {
    let mut empty = GroupConcat::new(b"|".to_vec(), 0, false);
    empty.update([Some(Vec::new())]);
    assert_eq!(empty.result(), Some(b"".as_slice()));

    let mut unlimited = GroupConcat::new(b"|".to_vec(), 0, false);
    unlimited.update([Some(b"a".to_vec()), Some(b"b".to_vec())]);
    assert_eq!(unlimited.result(), Some(b"a|b".as_slice()));
    assert!(!unlimited.truncated());

    let mut merged = GroupConcat::new(b",".to_vec(), 64, false);
    merged.update([Some(b"a".to_vec())]);
    let mut empty_partial = GroupConcat::new(b",".to_vec(), 64, false);
    empty_partial.update([Some(Vec::new())]);
    merged.merge(&empty_partial);
    assert_eq!(merged.result(), Some(b"a,".as_slice()));

    let mut distinct = GroupConcat::new(b",".to_vec(), 64, true);
    distinct.update([Some(Vec::new()), Some(Vec::new()), Some(b"x".to_vec())]);
    assert_eq!(distinct.result(), Some(b",x".as_slice()));
}

/// Go keeps the truncation sentinel on the aggregate function rather than in
/// each partial result, so resetting between groups must not allow a second
/// truncation warning during the same aggregate function's lifetime.
#[test]
fn group_concat_reset_preserves_lifetime_truncation_sentinel() {
    let mut concat = GroupConcat::new(b",".to_vec(), 2, false);
    concat.update([Some(b"abc".to_vec())]);
    assert!(concat.truncated());

    concat.reset();
    assert_eq!(concat.result(), None);
    assert!(concat.truncated());
}
