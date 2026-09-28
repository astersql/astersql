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

// LIKE 内建函数的向量化求值单元测试。
//
// 覆盖通配符匹配、转义符、模式缓存（pattern cache：同一表达式跨行复用已编译通配符）
// 以及参数个数校验，对齐 Go `builtin_like_vec_test.go` 的行为。

use crate::expression_group_15::*;
use crate::legacy_vectorized_runtime::{Chunk, Column, EvalContext, LiteralExpression, Row};

/// 验证按行向量化 LIKE：通配符 `%`/`_`、转义、以及输入/模式一侧为 NULL 时输出 NULL。
#[test]
fn test_vectorized_builtin_like_func() {
    let signature = builtinLikeSig::new(vec![
        LiteralExpression::strings(vec![Some("abc"), Some("a_c"), Some("bb"), None]),
        LiteralExpression::strings(vec![Some("a%"), Some(r"a\_c"), Some("b_%b"), Some("%")]),
        LiteralExpression::ints(vec![Some(i64::from(b'\\')); 4]),
    ])
    .unwrap();

    let mut result = Column::default();
    // vecEvalInt：对 Chunk 中每一行批量求值，结果写入列缓冲。
    signature
        .vecEvalInt(&EvalContext::default(), &Chunk::new(4), &mut result)
        .unwrap();

    // 前两行匹配；第三行不匹配但非 NULL；第四行模式为 NULL → 结果 NULL。
    assert_eq!(result.Int64s(), &[1, 1, 0, 0]);
    assert!(!result.IsNull(0));
    assert!(!result.IsNull(1));
    assert!(!result.IsNull(2));
    assert!(result.IsNull(3));
    assert!(signature.vectorized());
}

/// 标量路径：转义匹配后应初始化运行时模式缓存；Clone 不得拷贝该缓存；错误参数个数须失败。
#[test]
fn test_like_escape_cache_and_invalid_arity() {
    let signature = likeFunctionClass::new("like")
        .getFunction(vec![
            LiteralExpression::constant_string(Some("a_b")),
            LiteralExpression::constant_string(Some(r"a\_b")),
            LiteralExpression::constant_int(Some(i64::from(b'\\'))),
        ])
        .unwrap();

    assert_eq!(
        signature.evalInt(&EvalContext::default(), Row(0)).unwrap(),
        Some(1)
    );
    // Go 把编译后的通配符缓存在签名对象上；Clone 刻意不复制，避免会话间共享脏缓存。
    assert!(signature.cache_initialized());
    assert!(!signature.Clone().cache_initialized());
    assert!(
        likeFunctionClass::new("like")
            .getFunction(Vec::new())
            .is_err()
    );
}
