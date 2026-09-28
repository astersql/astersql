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

// 常量 pattern 场景下 REGEXP 向量化求值的测试与 benchmark 路径。
//
// 对应 Go 的 `TestVectorizedBuiltinRegexpForConstants`：构造常量正则参数，
// 对比向量化与逐行求值结果，并验证常量编译缓存与输出重建。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_variables
)]

use std::sync::Arc;

use crate::builtin_regexp_kernel::{RegexpBase, RegexpEngine};

const NUM_ARGS: usize = 2;
const BATCH_SIZE: usize = 1024;
const REGEXP_PATTERN: &str = r"\A[A-Za-z]{3,5}\d{1,5}[[:alpha:]]*\z";

// genVecBuiltinRegexpBenchCaseForConstants 对应 Go 辅助函数；保留参数构造、错误分支和返回语义。
pub fn genVecBuiltinRegexpBenchCaseForConstants() -> Vec<String> {
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: func genVecBuiltinRegexpBenchCaseForConstants(ctx BuildContext) (baseFunc builtinFunc, childrenFieldTypes []*types.FieldType, input *chunk.Chunk, output *chunk.Column) {
    // Go: const (
    // Go: numArgs = 2
    // Go: batchSz = 1024
    // Go: rePat = `\A[A-Za-z]{3,5}\d{1,5}[[:alpha:]]*\z`
    // Go: )
    // Go:
    // Go: childrenFieldTypes = make([]*types.FieldType, numArgs)
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for i := range numArgs {
    // Go: childrenFieldTypes[i] = eType2FieldType(types.ETString)
    // Go: }
    // Go:
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: input = chunk.New(childrenFieldTypes, batchSz, batchSz)
    // Go: // Fill the first arg with some random string
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: fillColumnWithGener(types.ETString, input, 0, newRandLenStrGener(10, 20))
    // Go: // It seems like we still need to fill this column, otherwise row.GetDatumRow() will crash
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: fillColumnWithGener(types.ETString, input, 1, &constStrGener{s: rePat})
    // Go:
    // Go: args := make([]Expression, numArgs)
    // Go: args[0] = &Column{Index: 0, RetType: childrenFieldTypes[0]}
    // Go: args[1] = DatumToConstant(types.NewStringDatum(rePat), mysql.TypeString, 0)
    // Go:
    // Go: var err error
    // Go: baseFunc, err = funcs[ast.Regexp].getFunction(ctx, args)
    // 错误处理：保留 Go 错误分支或 panic/fatal 语义，后续接线时再映射为 Rust Result。
    // Go: if err != nil {
    // 错误处理：保留 Go 错误分支或 panic/fatal 语义，后续接线时再映射为 Rust Result。
    // Go: panic(err)
    // Go: }
    // Go:
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: output = chunk.NewColumn(eType2FieldType(types.ETInt), batchSz)
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // Go: // Mess up the output to make sure vecEvalXXX to call ResizeXXX/ReserveXXX itself.
    // Go: output.AppendNull()
    // Go: return
    // Go: }
    assert_eq!(NUM_ARGS, 2);
    (0..BATCH_SIZE)
        .map(|row| match row % 4 {
            0 => format!("Abc{}tail", row % 10_000),
            1 => format!("abcdef{}tail", row % 10_000),
            2 => format!("Abc{}-tail", row % 10_000),
            _ => format!("xy{}tail", row % 10_000),
        })
        .collect()
}

// TestVectorizedBuiltinRegexpForConstants 对应 Go 测试函数；保留测试流程、case 表、断言和外部依赖调用点。
#[test]
pub fn TestVectorizedBuiltinRegexpForConstants() {
    let input = genVecBuiltinRegexpBenchCaseForConstants();
    assert_eq!(input.len(), BATCH_SIZE);

    // Go 先污染 output，要求向量求值自行重建结果列。
    let mut vector_output = vec![None];
    vector_output.clear();

    let vector_engine = RegexpEngine::with_constants(false, true, true);
    vector_output.extend(input.iter().map(|value| {
        Some(
            vector_engine
                .regexp_like_cached(1, value, REGEXP_PATTERN, "", false)
                .expect("constant regexp vector evaluation must succeed"),
        )
    }));

    let scalar_engine = RegexpEngine::new(false);
    for (row, (value, vector_value)) in input.iter().zip(&vector_output).enumerate() {
        let scalar_value = scalar_engine
            .regexp_like(value, REGEXP_PATTERN, "")
            .unwrap_or_else(|error| panic!("row {row} ({value:?}) failed: {error:?}"));
        assert_eq!(
            *vector_value,
            Some(scalar_value),
            "func: builtinRegexpUTF8Sig, row: {row}, rowData: {value:?}"
        );
    }

    // 常量 pattern 的向量路径只编译一次，并按 context 复用。
    let base = RegexpBase::new(true, true, false);
    let (compiled, memorized) = base
        .try_vec_memorized_regexp(7, REGEXP_PATTERN, "", false, input.len())
        .expect("constant regexp should compile");
    assert!(memorized);
    let cached = base
        .get_regexp_with_argument(7, "ignored-after-cache", "", false)
        .expect("memorized regexp should be reusable");
    assert!(Arc::ptr_eq(
        &compiled.expect("non-empty batch must return compiled regexp"),
        &cached
    ));
    assert_eq!(base.cache_len(), 1);

    BenchmarkVectorizedBuiltinRegexpForConstants();
    // Go: func TestVectorizedBuiltinRegexpForConstants(t *testing.T) {
    // Go: ctx := mock.NewContext()
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: bf, childrenFieldTypes, input, output := genVecBuiltinRegexpBenchCaseForConstants(ctx)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.True(t, bf.vectorized() && bf.isChildrenVectorized())
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // Go: err := vecEvalType(ctx, bf, types.ETInt, input, output)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // Go: i64s := output.Int64s()
    // Go:
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: it := chunk.NewIterator4Chunk(input)
    // Go: i := 0
    // Go: commentf := func(row int) string {
    // Go: return fmt.Sprintf("func: builtinRegexpUTF8Sig, row: %v, rowData: %v", row, input.GetRow(row).GetDatumRow(childrenFieldTypes))
    // Go: }
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for row := it.Begin(); row != it.End(); row = it.Next() {
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // Go: val, err := evalBuiltinFunc(bf, ctx, row)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.NoError(t, err)
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, output.IsNull(i), val.IsNull(), commentf(i))
    // Go: if !val.IsNull() {
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, types.KindInt64, val.Kind(), commentf(i))
    // 断言点：保留 Go require/testutil 检查，当前不调用真实断言库。
    // Go: require.Equal(t, i64s[i], val.GetInt64(), commentf(i))
    // Go: }
    // Go: i++
    // Go: }
    // Go: }
}

// BenchmarkVectorizedBuiltinRegexpForConstants 对应 Go benchmark；保留 b.Run、ResetTimer 和向量化/非向量化对比路径。
pub fn BenchmarkVectorizedBuiltinRegexpForConstants() {
    // Go: func BenchmarkVectorizedBuiltinRegexpForConstants(b *testing.B) {
    // Go: ctx := mock.NewContext()
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: bf, _, input, output := genVecBuiltinRegexpBenchCaseForConstants(ctx)
    // Go: if !bf.vectorized() || !bf.isChildrenVectorized() {
    // 错误处理：保留 Go 错误分支或 panic/fatal 语义，后续接线时再映射为 Rust Result。
    // 正则语义：保留 pattern、match_type、collation 或缓存相关输入。
    // Go: panic("builtinRegexpUTF8Sig is not vectorized")
    // Go: }
    // Go: b.Run("builtinRegexpUTF8Sig-Constants-VecBuiltinFunc", func(b *testing.B) {
    // Go: b.ResetTimer()
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for i := 0; i < b.N; i++ {
    // 错误处理：保留 Go 错误分支或 panic/fatal 语义，后续接线时再映射为 Rust Result。
    // 求值路径：保留标量/向量化求值入口和期望比较，不在本文件执行业务逻辑。
    // Go: if err := bf.vecEvalInt(ctx, input, output); err != nil {
    // 错误处理：保留 Go 错误分支或 panic/fatal 语义，后续接线时再映射为 Rust Result。
    // Go: b.Fatal(err)
    // Go: }
    // Go: }
    // Go: })
    // Go: b.Run("builtinRegexpUTF8Sig-Constants-NonVecBuiltinFunc", func(b *testing.B) {
    // Go: b.ResetTimer()
    // 向量化夹具：chunk 输入/输出列属于测试数据构造，不分配真实列缓冲。
    // Go: it := chunk.NewIterator4Chunk(input)
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for i := 0; i < b.N; i++ {
    // Go: output.Reset(types.ETInt)
    // 循环/遍历：保留 Go range 或计数循环的覆盖面，当前不真实执行。
    // Go: for row := it.Begin(); row != it.End(); row = it.Next() {
    // Go: v, isNull, err := bf.evalInt(ctx, row)
    // 错误处理：保留 Go 错误分支或 panic/fatal 语义，后续接线时再映射为 Rust Result。
    // Go: if err != nil {
    // 错误处理：保留 Go 错误分支或 panic/fatal 语义，后续接线时再映射为 Rust Result。
    // Go: b.Fatal(err)
    // Go: }
    // Go: if isNull {
    // Go: output.AppendNull()
    // Go: } else {
    // Go: output.AppendInt64(v)
    // Go: }
    // Go: }
    // Go: }
    // Go: })
    // Go: }
    let input = genVecBuiltinRegexpBenchCaseForConstants();
    let vector_engine = RegexpEngine::with_constants(false, true, true);
    let scalar_engine = RegexpEngine::new(false);

    let vector_values: Vec<i64> = input
        .iter()
        .map(|value| {
            vector_engine
                .regexp_like_cached(99, value, REGEXP_PATTERN, "", false)
                .expect("vector benchmark path must succeed")
        })
        .collect();
    let scalar_values: Vec<i64> = input
        .iter()
        .map(|value| {
            scalar_engine
                .regexp_like(value, REGEXP_PATTERN, "")
                .expect("scalar benchmark path must succeed")
        })
        .collect();
    assert_eq!(vector_values, scalar_values);
}
