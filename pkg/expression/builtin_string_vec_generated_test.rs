// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 自动生成的字符串内建函数向量化测试。
//
// 对应 Go `builtin_string_vec_generated_test.go`：保留 FIELD 等生成函数的
// vecExprBenchCase 表、EvalOneVec / BuiltinFunc 测试与 benchmark 入口形状，
// 并对三种 FIELD 签名执行真实 Rust 向量内核。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_variables
)]

// 这段逻辑覆盖自动生成的字符串内建函数向量化 eval-one-vec、builtin-func 与 benchmark case 表。
// 主要类型、函数、方法前说明对应 Go 语义；关键分支、参数解析、资源收尾、错误处理、并发、异步、IO 和外部依赖在原调用附近补中文注释。
// Go imports（保留依赖边界，待 Rust crate 接线）：
// - "testing"
// - "github.com/pingcap/tidb/pkg/parser/ast"
// - "github.com/pingcap/tidb/pkg/types"

use crate::builtin_string_vec_generated_kernel::{
    FieldVectorError, field_int, field_int_vectorized, field_real, field_real_vectorized,
    field_string, field_string_vectorized,
};

// VecExprBenchCase 对应 Go 的 vecExprBenchCase；字段保存生成器输入的精确文本。
/// 向量化表达式 benchmark/case 配置。
pub struct VecExprBenchCase {
    pub label: &'static str,
    pub ret_eval_type: &'static str,
    pub children_types: &'static str,
    pub children_field_types: &'static str,
    pub geners: &'static str,
    pub constants: &'static str,
    pub aes_modes: &'static str,
    pub chunk_size: &'static str,
    pub go_literal: &'static str,
}

// VecCaseGroup 对应 Go map[string][]vecExprBenchCase 的单个内建函数分组。
/// 单个内建函数的向量化 case 分组。
pub struct VecCaseGroup {
    pub builtin: &'static str,
    pub cases: &'static [VecExprBenchCase],
}

// vec_case 保留 case 的返回类型、子参数类型、字段类型、数据生成器、常量参数和额外生成器参数。
/// 构造一条 vecExprBenchCase 配置记录。
pub const fn vec_case(
    label: &'static str,
    ret_eval_type: &'static str,
    children_types: &'static str,
    children_field_types: &'static str,
    geners: &'static str,
    constants: &'static str,
    aes_modes: &'static str,
    chunk_size: &'static str,
    go_literal: &'static str,
) -> VecExprBenchCase {
    VecExprBenchCase {
        label,
        ret_eval_type,
        children_types,
        children_field_types,
        geners,
        constants,
        aes_modes,
        chunk_size,
        go_literal,
    }
}

// VECGENERATEDBUILTINSTRINGCASES 对应 Go 的向量化 case map；分组顺序和每组 case 顺序与源文件一致。
/// 生成字符串内建的向量化 case 总表（对齐 Go map 顺序）。
pub const VECGENERATEDBUILTINSTRINGCASES: &[VecCaseGroup] = &[VecCaseGroup {
    builtin: r#"ast.Field"#,
    cases: &FIELD_VECGENERATEDBUILTINSTRINGCASES_CASES,
}];

// ast.Field 的 vecExprBenchCase 列表，保留 Go 生成器/常量/字段类型配置。
/// ast.Field 的向量化 case 列表。
pub const FIELD_VECGENERATEDBUILTINSTRINGCASES_CASES: &[VecExprBenchCase] = &[
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETInt, types.ETInt, types.ETInt, types.ETInt]"#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETInt, types.ETInt, types.ETInt, types.ETInt}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETReal, types.ETReal, types.ETReal, types.ETReal]"#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETReal, types.ETReal, types.ETReal, types.ETReal}}"#,
    ),
    vec_case(
        r#""#,
        r#"types.ETInt"#,
        r#"[types.ETString, types.ETString, types.ETString, types.ETString]"#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#""#,
        r#"{retEvalType: types.ETInt, childrenTypes: []types.EvalType{types.ETString, types.ETString, types.ETString, types.ETString}}"#,
    ),
];

fn run_generated_field_parity_suite() {
    assert_eq!(VECGENERATEDBUILTINSTRINGCASES.len(), 1);
    assert_eq!(FIELD_VECGENERATEDBUILTINSTRINGCASES_CASES.len(), 3);
    assert!(field_int_vectorized());
    assert!(field_real_vectorized());
    assert!(field_string_vectorized());

    assert_eq!(
        field_int(
            &[Some(2), Some(3), None, Some(9)],
            &[
                vec![Some(1), Some(3), Some(3), None],
                vec![Some(2), Some(3), None, Some(8)],
                vec![Some(2), Some(4), Some(3), Some(9)],
            ],
        )
        .unwrap(),
        vec![2, 1, 0, 3],
    );
    assert_eq!(
        field_real(
            &[Some(1.5), Some(f64::NAN), Some(-0.0), None],
            &[vec![Some(1.5), Some(f64::NAN), Some(0.0), Some(0.0)]],
        )
        .unwrap(),
        vec![1, 0, 1, 0],
    );
    assert_eq!(
        field_string(
            &[Some("A"), Some("x"), None],
            &[
                vec![Some("a"), Some("y"), Some("x")],
                vec![Some("A"), Some("X"), None],
            ],
            |left, right| left.eq_ignore_ascii_case(right),
        )
        .unwrap(),
        vec![1, 2, 0],
    );

    assert_eq!(field_int(&[], &[]).unwrap(), Vec::<i64>::new());
    assert_eq!(field_int(&[Some(1), None], &[]).unwrap(), vec![0, 0]);
    assert_eq!(
        field_int(&[Some(1)], &[vec![Some(1), Some(2)]]),
        Err(FieldVectorError::RowCountMismatch),
    );
}

// TestVectorizedGeneratedBuiltinStringEvalOneVec 对应 Go 测试函数；保留测试流程、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
/// EvalOneVec 路径测试入口。
pub fn test_vectorized_generated_builtin_string_eval_one_vec() {
    run_generated_field_parity_suite();
    // Go 签名（源文件第 37 行）：func TestVectorizedGeneratedBuiltinStringEvalOneVec(t *testing.T) {
    // Go: testVectorizedEvalOneVec(t, vecGeneratedBuiltinStringCases)
    // Go: }
}

// TestVectorizedGeneratedBuiltinStringFunc 对应 Go 测试函数；保留测试流程、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
/// BuiltinFunc 路径测试入口。
pub fn test_vectorized_generated_builtin_string_func() {
    run_generated_field_parity_suite();
    // Go 签名（源文件第 41 行）：func TestVectorizedGeneratedBuiltinStringFunc(t *testing.T) {
    // Go: testVectorizedBuiltinFunc(t, vecGeneratedBuiltinStringCases)
    // Go: }
}

// BenchmarkVectorizedGeneratedBuiltinStringEvalOneVec 对应 Go benchmark；保留 benchmark 入口和调用目标，当前不执行性能测试。
/// EvalOneVec benchmark 入口形状。
pub fn benchmark_vectorized_generated_builtin_string_eval_one_vec() {
    // Go 签名（源文件第 45 行）：func BenchmarkVectorizedGeneratedBuiltinStringEvalOneVec(b *testing.B) {
    // Go: benchmarkVectorizedEvalOneVec(b, vecGeneratedBuiltinStringCases)
    // Go: }
}

// BenchmarkVectorizedGeneratedBuiltinStringFunc 对应 Go benchmark；保留 benchmark 入口和调用目标，当前不执行性能测试。
/// BuiltinFunc benchmark 入口形状。
pub fn benchmark_vectorized_generated_builtin_string_func() {
    // Go 签名（源文件第 49 行）：func BenchmarkVectorizedGeneratedBuiltinStringFunc(b *testing.B) {
    // Go: benchmarkVectorizedBuiltinFunc(b, vecGeneratedBuiltinStringCases)
    // Go: }
}
