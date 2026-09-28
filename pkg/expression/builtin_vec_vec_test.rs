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

//! Go `vecBuiltinVecCases` 的 Rust 测试入口。
//!
//! Go 通过通用表驱动框架覆盖这些签名；Rust 的等价数值、NULL、NaN、错误与文本
//! 回归集中在 `builtin_vec_vec_31_aster_unit_test.rs`，这里同时锁定函数分组清单，防止
//! 后续迁移遗漏 Go 表中的签名。

/// 与 Go `vecBuiltinVecCases` 键集合一一对应。
const VECTORIZED_BUILTIN_VEC_FUNCTIONS: [&str; 8] = [
    "vec_dims",
    "vec_l1_distance",
    "vec_l2_distance",
    "vec_cosine_distance",
    "vec_negative_inner_product",
    "vec_l2_norm",
    "vec_as_text",
    "vec_from_text",
];

#[test]
fn test_vectorized_builtin_vec_func() {
    assert_eq!(
        VECTORIZED_BUILTIN_VEC_FUNCTIONS,
        [
            crate::ast::VecDims,
            crate::ast::VecL1Distance,
            crate::ast::VecL2Distance,
            crate::ast::VecCosineDistance,
            crate::ast::VecNegativeInnerProduct,
            crate::ast::VecL2Norm,
            crate::ast::VecAsText,
            crate::ast::VecFromText,
        ]
    );
    crate::builtin_vec_vec_aster_unit_test::run_vector_builtin_parity_suite();
}
